//! Kernel behaviour tests: slots, priorities, key map layers, commands,
//! settings and transactional plugin registration.

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::RefCell;

use quireink_platform::{
    ButtonState, Color, DeviceInfo, DisplayInfo, InputCaps, InputEvent, Instant, PixelFormat,
    Point, PointerEvent, PointerPhase, PointerTool, Rect, RefreshMode, Rotation,
};
use quireink_sdk::builtin::{actions, screens, settings, slots};
use quireink_sdk::{
    ActionSpec, Canvas, Command, Cx, Env, Handled, Id, Manifest, Plugin, Priority, RegisterError,
    Registrar, SDK_VERSION, Screen, ScreenSpec, SettingKind, SettingSpec, SettingValue, Version,
};

use crate::{Kernel, SpeechCall, WarningSource};

const DEVICE: DeviceInfo = DeviceInfo {
    id: "test",
    name: "Test device",
    display: DisplayInfo {
        width: 90,
        height: 90,
        dpi: 100,
        format: PixelFormat::Gray4,
        rotation: Rotation::Deg0,
        color: None,
    },
    input: InputCaps {
        buttons: true,
        touch: true,
        pen: false,
        keys: &[],
    },
    memory: quireink_platform::MemoryClass::Large,
};

type Log = Rc<RefCell<Vec<String>>>;

/// A configurable plugin providing one screen bound to the home slot.
struct TestPlugin {
    id: &'static str,
    priority: Priority,
    log: Log,
    sdk: Version,
    /// Registers an id outside its namespace after its other contributions.
    misbehave: bool,
    keymap: Option<&'static str>,
}

impl TestPlugin {
    fn new(id: &'static str, log: &Log) -> Self {
        Self {
            id,
            priority: Priority::DEFAULT,
            log: log.clone(),
            sdk: SDK_VERSION,
            misbehave: false,
            keymap: None,
        }
    }

    fn screen_id(&self) -> Id {
        Id::parse(&alloc::format!("{}.screen", self.id)).unwrap()
    }
}

impl Plugin for TestPlugin {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: Id::parse(self.id).unwrap(),
            name: Cow::Borrowed("Test"),
            description: Cow::Borrowed(""),
            version: Version::new(1, 0, 0),
            sdk: self.sdk,
        }
    }

    fn register(&self, r: &mut dyn Registrar) -> Result<(), RegisterError> {
        let log = self.log.clone();
        let name = self.id;
        r.add_screen(ScreenSpec {
            id: self.screen_id(),
            title: Cow::Borrowed("test.screen"),
            factory: Box::new(move |_, _| {
                Box::new(TestScreen {
                    name,
                    log: log.clone(),
                })
            }),
        })?;
        r.bind_slot(slots::HOME, self.screen_id(), self.priority)?;
        r.add_action(ActionSpec {
            id: Id::parse(&alloc::format!("{}.special", self.id)).unwrap(),
            title: Cow::Borrowed("test.special"),
            handler: None,
        })?;
        r.add_setting(SettingSpec::new(
            Id::parse(&alloc::format!("{}.level", self.id)).unwrap(),
            "test.level",
            SettingKind::Int { min: 1, max: 5 },
            SettingValue::Int(2),
        ))?;
        if let Some(keymap) = self.keymap {
            r.add_keymap(Cow::Borrowed(keymap))?;
        }
        if self.misbehave {
            r.add_action(ActionSpec {
                id: Id::new_static("core.stolen"),
                title: Cow::Borrowed(""),
                handler: None,
            })?;
        }
        Ok(())
    }
}

struct TestScreen {
    name: &'static str,
    log: Log,
}

impl Screen for TestScreen {
    fn draw(&mut self, canvas: &mut Canvas<'_>, _: &Env<'_>) {
        canvas.fill_rect(0, 0, 10, 10, Color::BLACK);
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        self.log
            .borrow_mut()
            .push(alloc::format!("{}:{}", self.name, action));
        if *action == actions::NEXT_PAGE {
            cx.request_redraw_area(Rect::new(20, 20, 5, 5));
            cx.request_refresh(RefreshMode::Partial);
            return Handled::Yes;
        }
        if action.namespace() == self.name || *action == actions::CONFIRM {
            return Handled::Yes;
        }
        Handled::No
    }
}

/// Draws what changed, like a shell after each call.
fn render(kernel: &mut Kernel) {
    kernel.render(&mut |_, _| {});
}

fn press(kernel: &mut Kernel, at: u64, name: &'static str) -> crate::Effects {
    kernel.handle(
        Instant(at),
        InputEvent::Button {
            name,
            state: ButtonState::Pressed,
        },
    );
    let effects = kernel.handle(
        Instant(at + 50),
        InputEvent::Button {
            name,
            state: ButtonState::Released,
        },
    );
    render(kernel);
    effects
}

fn tap(kernel: &mut Kernel, at: u64, x: i32, y: i32) -> crate::Effects {
    let event = |phase| {
        InputEvent::Pointer(PointerEvent {
            id: 0,
            phase,
            point: Point::new(x, y),
            tool: PointerTool::Finger,
            pressure: 1000,
        })
    };
    kernel.handle(Instant(at), event(PointerPhase::Down));
    let effects = kernel.handle(Instant(at + 60), event(PointerPhase::Up));
    render(kernel);
    effects
}

fn log() -> Log {
    Rc::new(RefCell::new(Vec::new()))
}

#[test]
fn empty_home_shows_placeholder() {
    let mut kernel = Kernel::builder(DEVICE).build().unwrap();
    let effects = kernel.start(Instant(0));
    assert_eq!(kernel.active_screen(), Some(&screens::PLACEHOLDER));
    assert_eq!(
        effects.refresh(),
        Some((Rect::new(0, 0, 90, 90), RefreshMode::Full))
    );
    assert!(kernel.take_warnings().is_empty());
}

#[test]
fn highest_priority_wins_and_user_config_overrides() {
    let log = log();
    let mut low = TestPlugin::new("low", &log);
    low.priority = Priority::FALLBACK;
    let high = TestPlugin::new("high", &log);

    let mut kernel = Kernel::builder(DEVICE)
        .plugin(high)
        .plugin(low)
        .build()
        .unwrap();
    kernel.start(Instant(0));
    assert_eq!(kernel.active_screen().map(Id::as_str), Some("high.screen"));

    let mut kernel = Kernel::builder(DEVICE)
        .plugin(TestPlugin::new("high", &log))
        .plugin(TestPlugin::new("low", &log))
        .user_config("[slots]\ncore.home = low.screen\n")
        .build()
        .unwrap();
    kernel.start(Instant(0));
    assert_eq!(kernel.active_screen().map(Id::as_str), Some("low.screen"));
}

#[test]
fn buttons_route_through_key_map_to_screen_and_handlers() {
    let log = log();
    let mut kernel = Kernel::builder(DEVICE)
        .plugin(TestPlugin::new("a", &log))
        .build()
        .unwrap();
    kernel.start(Instant(0));

    let effects = press(&mut kernel, 100, "page-forward");
    assert_eq!(
        effects.refresh(),
        Some((Rect::new(20, 20, 5, 5), RefreshMode::Partial))
    );
    press(&mut kernel, 300, "confirm");
    // `back` is not handled by the screen; the global handler pops, which
    // is a no-op on the root screen.
    let effects = press(&mut kernel, 500, "back");
    assert_eq!(effects.refresh(), None);
    assert_eq!(
        *log.borrow(),
        ["a:core.next-page", "a:core.confirm", "a:core.back"]
    );
    assert!(kernel.take_warnings().is_empty());
}

#[test]
fn screen_layer_overrides_global_layer_and_user_overrides_plugin() {
    let log = log();
    let mut plugin = TestPlugin::new("a", &log);
    plugin.keymap = Some("[keymap.a.screen]\nbutton.confirm = a.special\n");

    let mut kernel = Kernel::builder(DEVICE).plugin(plugin).build().unwrap();
    kernel.start(Instant(0));
    press(&mut kernel, 100, "confirm");
    assert_eq!(*log.borrow(), ["a:a.special"]);

    log.borrow_mut().clear();
    let mut plugin = TestPlugin::new("a", &log);
    plugin.keymap = Some("[keymap.a.screen]\nbutton.confirm = a.special\n");
    let mut kernel = Kernel::builder(DEVICE)
        .plugin(plugin)
        .user_config("[keymap.a.screen]\nbutton.confirm = none\n")
        .build()
        .unwrap();
    kernel.start(Instant(0));
    press(&mut kernel, 100, "confirm");
    assert!(log.borrow().is_empty());
}

#[test]
fn failing_plugin_leaves_no_traces() {
    let log = log();
    let mut plugin = TestPlugin::new("bad", &log);
    plugin.misbehave = true;

    let mut kernel = Kernel::builder(DEVICE).plugin(plugin).build().unwrap();
    kernel.start(Instant(0));
    assert_eq!(kernel.active_screen(), Some(&screens::PLACEHOLDER));
    assert!(kernel.settings().get("bad.level").is_none());
    let bad = kernel.plugins().get("bad").unwrap();
    assert_eq!(bad.state, quireink_sdk::PluginState::Failed);

    let warnings = kernel.take_warnings();
    assert_eq!(warnings.len(), 1);
    assert_eq!(
        warnings[0].source,
        WarningSource::Plugin(Id::new_static("bad"))
    );
}

#[test]
fn incompatible_and_disabled_plugins_are_skipped() {
    let log = log();
    let mut old = TestPlugin::new("old", &log);
    old.sdk = Version::new(9, 0, 0);

    let mut kernel = Kernel::builder(DEVICE)
        .plugin(old)
        .plugin(TestPlugin::new("off", &log))
        .user_config("[plugins]\noff = no\n")
        .build()
        .unwrap();
    kernel.start(Instant(0));
    assert_eq!(kernel.active_screen(), Some(&screens::PLACEHOLDER));
    let warnings = kernel.take_warnings();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].message.contains("requires SDK"));
}

#[test]
fn plugins_can_ship_switched_off() {
    let log = log();
    let enabled = |config: &str| {
        let kernel = Kernel::builder(DEVICE)
            .plugin_off(TestPlugin::new("web", &log))
            .user_config(config)
            .build()
            .unwrap();
        kernel.shared.plugins.get("web").map(|info| info.enabled)
    };
    assert_eq!(enabled(""), Some(false), "listed, but off");
    assert_eq!(
        enabled(
            "[plugins]
web = on
"
        ),
        Some(true)
    );
    assert_eq!(
        enabled(
            "[plugins]
web = off
"
        ),
        Some(false)
    );
}

#[test]
fn user_settings_are_applied_and_validated() {
    let log = log();
    let mut kernel = Kernel::builder(DEVICE)
        .plugin(TestPlugin::new("a", &log))
        .user_config(
            "[core]\nlanguage = de\nfull-refresh-interval = 2\n\
             [a]\nlevel = 4\n[b]\nlevel = 1\n[a]\nlevel = 99\n",
        )
        .build()
        .unwrap();
    assert_eq!(kernel.translator().language(), "de");
    assert_eq!(kernel.translator().tr("core.action.home"), "Startseite");
    assert_eq!(kernel.settings().int("a.level"), 4);
    let warnings: Vec<String> = kernel
        .take_warnings()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    assert!(warnings[0].contains("`b.level`: unknown setting"));
    assert!(warnings[1].contains("`a.level`: invalid value"));

    // Every second partial refresh becomes a full refresh of the screen.
    kernel.start(Instant(0));
    let first = press(&mut kernel, 100, "page-forward");
    let second = press(&mut kernel, 300, "page-forward");
    assert_eq!(first.refresh().map(|r| r.1), Some(RefreshMode::Partial));
    assert_eq!(
        second.refresh(),
        Some((Rect::new(0, 0, 90, 90), RefreshMode::Full))
    );
}

#[test]
fn long_tap_toggles_night_mode() {
    let mut kernel = Kernel::builder(DEVICE).build().unwrap();
    kernel.start(Instant(0));
    let down = InputEvent::Pointer(PointerEvent {
        id: 0,
        phase: PointerPhase::Down,
        point: Point::new(45, 45),
        tool: PointerTool::Finger,
        pressure: 1000,
    });
    let effects = kernel.handle(Instant(1000), down);
    assert_eq!(effects.wake_at(), Some(Instant(1500)));
    let effects = kernel.tick(Instant(1500));
    assert!(kernel.settings().bool(settings::NIGHT_MODE.as_str()));
    assert_eq!(effects.refresh().map(|r| r.1), Some(RefreshMode::Full));
    render(&mut kernel);
    assert_eq!(kernel.frame().pixel(0, 0), Color::BLACK);
}

#[test]
fn taps_resolve_through_zones() {
    let log = log();
    let mut kernel = Kernel::builder(DEVICE)
        .plugin(TestPlugin::new("a", &log))
        .build()
        .unwrap();
    kernel.start(Instant(0));
    tap(&mut kernel, 100, 85, 45);
    tap(&mut kernel, 300, 5, 45);
    assert_eq!(*log.borrow(), ["a:core.next-page", "a:core.prev-page"]);
}

#[test]
fn reports_unknown_references_in_config() {
    let mut kernel = Kernel::builder(DEVICE)
        .user_config("[slots]\ncore.home = nope.screen\n[keymap.global]\nbutton.x = nope.action\n")
        .build()
        .unwrap();
    let warnings = kernel.take_warnings();
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    kernel.start(Instant(0));
    assert_eq!(kernel.active_screen(), Some(&screens::PLACEHOLDER));
}

/// A plugin contributing two tiny bitmap fonts.
struct FontPlugin;

impl Plugin for FontPlugin {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: Id::new_static("fonts"),
            name: Cow::Borrowed("Fonts"),
            description: Cow::Borrowed(""),
            version: Version::new(1, 0, 0),
            sdk: SDK_VERSION,
        }
    }

    fn register(&self, r: &mut dyn Registrar) -> Result<(), RegisterError> {
        for name in ["fonts.serif", "fonts.sans"] {
            let mut pack = quireink_text::PackBuilder::new();
            let metrics = quireink_text::LineMetrics {
                ascent: 8.0,
                descent: 2.0,
                line_height: 12.0,
            };
            pack.add_strike(
                10.0,
                metrics,
                alloc::vec![('a', quireink_text::Glyph::empty(5.0))],
            );
            let font = quireink_sdk::Font::from_data(pack.build(), &[]).unwrap();
            r.add_font(quireink_sdk::FontSpec::new(Id::parse(name).unwrap(), font))?;
        }
        r.bind_font(
            quireink_sdk::builtin::fonts::UI,
            Id::new_static("fonts.sans"),
            Priority::DEFAULT,
        )?;
        Ok(())
    }
}

#[test]
fn font_roles_follow_proposals_and_user_config() {
    let kernel = Kernel::builder(DEVICE).plugin(FontPlugin).build().unwrap();
    let fonts = kernel.fonts();
    assert_eq!(
        fonts.role("core.font-ui").unwrap().id().as_str(),
        "fonts.sans"
    );
    // No proposal for reading: the first registered font is used.
    assert_eq!(
        fonts.role("core.font-reading").unwrap().id().as_str(),
        "fonts.serif"
    );

    let mut kernel = Kernel::builder(DEVICE)
        .plugin(FontPlugin)
        .user_config(
            "[fonts]
core.font-ui = fonts.serif
core.font-reading = nope.font
",
        )
        .build()
        .unwrap();
    assert_eq!(
        kernel.fonts().role("core.font-ui").unwrap().id().as_str(),
        "fonts.serif"
    );
    let warnings = kernel.take_warnings();
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].message.contains("unknown font `nope.font`"));
}

/// Subscribes to `core.started`, opens its screen with parameters and runs
/// a timer started in `on_show`.
struct LifecyclePlugin {
    log: Log,
}

struct LifecycleScreen {
    log: Log,
    ticks: u32,
}

impl Screen for LifecycleScreen {
    fn draw(&mut self, _: &mut Canvas<'_>, _: &Env<'_>) {}

    fn on_action(&mut self, _: &Id, _: &mut Cx<'_, '_>) -> Handled {
        Handled::No
    }

    fn on_show(&mut self, cx: &mut Cx<'_, '_>) {
        self.log.borrow_mut().push(String::from("show"));
        let at = cx.env().now.add_millis(10);
        cx.wake_at(at);
    }

    fn on_timer(&mut self, cx: &mut Cx<'_, '_>) {
        self.ticks += 1;
        self.log.borrow_mut().push(alloc::format!(
            "timer {} at {}",
            self.ticks,
            cx.env().now.millis()
        ));
        if self.ticks < 2 {
            let at = cx.env().now.add_millis(10);
            cx.wake_at(at);
        }
    }
}

impl Plugin for LifecyclePlugin {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: Id::new_static("life"),
            name: Cow::Borrowed("Lifecycle"),
            description: Cow::Borrowed(""),
            version: Version::new(1, 0, 0),
            sdk: SDK_VERSION,
        }
    }

    fn register(&self, r: &mut dyn Registrar) -> Result<(), RegisterError> {
        let log = self.log.clone();
        r.add_screen(ScreenSpec {
            id: Id::new_static("life.screen"),
            title: Cow::Borrowed("life"),
            factory: Box::new(move |_, params| {
                log.borrow_mut()
                    .push(alloc::format!("open {}", params.get("path").unwrap_or("-")));
                Box::new(LifecycleScreen {
                    log: log.clone(),
                    ticks: 0,
                })
            }),
        })?;
        r.subscribe(
            quireink_sdk::builtin::events::STARTED,
            Box::new(|_, _, out| {
                out.push(quireink_sdk::Command::OpenScreen(
                    Id::new_static("life.screen"),
                    quireink_sdk::Params::new().with("path", "books/a.txt"),
                ));
            }),
        )?;
        Ok(())
    }
}

#[test]
fn events_open_screens_with_params_and_timers_run() {
    let log = log();
    let mut kernel = Kernel::builder(DEVICE)
        .plugin(LifecyclePlugin { log: log.clone() })
        .build()
        .unwrap();
    let effects = kernel.start(Instant(100));
    assert_eq!(kernel.active_screen().map(Id::as_str), Some("life.screen"));
    assert_eq!(effects.wake_at(), Some(Instant(110)));

    // Too early: nothing happens.
    kernel.tick(Instant(105));
    let effects = kernel.tick(Instant(110));
    assert_eq!(effects.wake_at(), Some(Instant(120)));
    let effects = kernel.tick(Instant(120));
    assert_eq!(effects.wake_at(), None);
    assert_eq!(
        *log.borrow(),
        [
            "open books/a.txt",
            "show",
            "timer 1 at 110",
            "timer 2 at 120"
        ]
    );
}

/// A full screen that logs events, plus an overlay covering the lower half.
struct OverlayPlugin {
    log: Log,
}

struct BaseScreen {
    log: Log,
}

impl Screen for BaseScreen {
    fn draw(&mut self, canvas: &mut Canvas<'_>, _: &Env<'_>) {
        let clip = canvas.clip();
        self.log
            .borrow_mut()
            .push(alloc::format!("draw base {}x{}", clip.width, clip.height));
    }

    fn on_action(&mut self, _: &Id, _: &mut Cx<'_, '_>) -> Handled {
        Handled::No
    }

    fn on_event(&mut self, event: &Id, params: &quireink_sdk::Params, cx: &mut Cx<'_, '_>) {
        self.log.borrow_mut().push(alloc::format!(
            "event {event} {}",
            params.get("id").unwrap_or_default()
        ));
        if event.as_str() == "ov.changed" {
            cx.request_redraw();
        }
    }
}

struct HalfOverlay;

impl Screen for HalfOverlay {
    fn region(&self, _: &Env<'_>) -> Option<Rect> {
        Some(Rect::new(0, 45, 90, 45))
    }

    fn draw(&mut self, canvas: &mut Canvas<'_>, _: &Env<'_>) {
        canvas.fill_rect(0, 45, 90, 45, Color::BLACK);
    }

    fn on_action(&mut self, _: &Id, _: &mut Cx<'_, '_>) -> Handled {
        Handled::No
    }
}

impl Plugin for OverlayPlugin {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: Id::new_static("ov"),
            name: Cow::Borrowed("Overlay"),
            description: Cow::Borrowed(""),
            version: Version::new(1, 0, 0),
            sdk: SDK_VERSION,
        }
    }

    fn register(&self, r: &mut dyn Registrar) -> Result<(), RegisterError> {
        let log = self.log.clone();
        r.add_screen(ScreenSpec {
            id: Id::new_static("ov.base"),
            title: Cow::Borrowed("base"),
            factory: Box::new(move |_, _| Box::new(BaseScreen { log: log.clone() })),
        })?;
        r.bind_slot(slots::HOME, Id::new_static("ov.base"), Priority::DEFAULT)?;
        r.add_screen(ScreenSpec {
            id: Id::new_static("ov.half"),
            title: Cow::Borrowed("half"),
            factory: Box::new(|_, _| Box::new(HalfOverlay)),
        })?;
        r.bind_slot(slots::MENU, Id::new_static("ov.half"), Priority::DEFAULT)?;
        for (action, order) in [(actions::TOGGLE_NIGHT_MODE, 20), (actions::HOME, 10)] {
            r.add_menu_entry(quireink_sdk::MenuEntry {
                menu: quireink_sdk::builtin::menus::MAIN,
                item: action,
                order,
            })?;
        }
        Ok(())
    }
}

#[test]
fn overlays_redraw_only_their_region_and_close_on_outside_taps() {
    let log = log();
    let mut kernel = Kernel::builder(DEVICE)
        .plugin(OverlayPlugin { log: log.clone() })
        .build()
        .unwrap();
    kernel.start(Instant(0));
    render(&mut kernel);
    log.borrow_mut().clear();

    // The menu action opens the overlay bound to core.menu.
    let effects = press(&mut kernel, 100, "menu");
    assert_eq!(kernel.active_screen().map(Id::as_str), Some("ov.half"));
    assert_eq!(
        effects.refresh(),
        Some((Rect::new(0, 45, 90, 45), RefreshMode::Partial))
    );
    assert_eq!(*log.borrow(), ["draw base 90x45"]);
    assert_eq!(kernel.frame().pixel(0, 60), Color::BLACK);
    assert_eq!(kernel.frame().pixel(0, 10), Color::WHITE);

    // A tap above the overlay closes it and restores the base screen.
    log.borrow_mut().clear();
    let effects = tap(&mut kernel, 300, 45, 10);
    assert_eq!(kernel.active_screen().map(Id::as_str), Some("ov.base"));
    assert_eq!(
        effects.refresh(),
        Some((Rect::new(0, 45, 90, 45), RefreshMode::Partial))
    );
    assert_eq!(*log.borrow(), ["draw base 90x45"]);
    assert_eq!(kernel.frame().pixel(0, 60), Color::WHITE);
}

/// The base screen and the half overlay as the notice.
struct NoticePlugin {
    log: Log,
}

impl Plugin for NoticePlugin {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: Id::new_static("nt"),
            name: Cow::Borrowed("Notice"),
            description: Cow::Borrowed(""),
            version: Version::new(1, 0, 0),
            sdk: SDK_VERSION,
        }
    }

    fn register(&self, r: &mut dyn Registrar) -> Result<(), RegisterError> {
        let log = self.log.clone();
        r.add_screen(ScreenSpec {
            id: Id::new_static("nt.base"),
            title: Cow::Borrowed("base"),
            factory: Box::new(move |_, _| Box::new(BaseScreen { log: log.clone() })),
        })?;
        r.bind_slot(slots::HOME, Id::new_static("nt.base"), Priority::DEFAULT)?;
        r.add_screen(ScreenSpec {
            id: Id::new_static("nt.notice"),
            title: Cow::Borrowed("notice"),
            factory: Box::new(|_, _| Box::new(HalfOverlay)),
        })?;
        r.bind_slot(
            slots::NOTICE,
            Id::new_static("nt.notice"),
            Priority::DEFAULT,
        )?;
        Ok(())
    }
}

#[test]
fn notices_show_over_everything_and_go_by_themselves() {
    let log = log();
    let mut kernel = Kernel::builder(DEVICE)
        .plugin(NoticePlugin { log: log.clone() })
        .build()
        .unwrap();
    kernel.start(Instant(0));
    render(&mut kernel);

    kernel.apply(Command::Notify(String::from("Done")));
    let effects = kernel.finish();
    assert_eq!(
        effects.refresh().map(|(r, _)| r),
        Some(Rect::new(0, 45, 90, 45))
    );
    assert_eq!(
        effects.wake_at(),
        Some(Instant(4000)),
        "four seconds by default"
    );
    render(&mut kernel);
    assert_eq!(kernel.frame().pixel(0, 60), Color::BLACK);
    // Input still goes to the screen below.
    assert_eq!(kernel.active_screen().map(Id::as_str), Some("nt.base"));

    // Time is up: gone.
    let effects = kernel.tick(Instant(4000));
    assert_eq!(
        effects.refresh().map(|(r, _)| r),
        Some(Rect::new(0, 45, 90, 45))
    );
    render(&mut kernel);
    assert_eq!(kernel.frame().pixel(0, 60), Color::WHITE);
    assert_eq!(effects.wake_at(), None);
}

#[test]
fn scheduled_events_come_at_their_time() {
    let log = log();
    let mut kernel = Kernel::builder(DEVICE)
        .plugin(OverlayPlugin { log: log.clone() })
        .build()
        .unwrap();
    kernel.start(Instant(0));
    let event = Id::new_static("ov.later");
    let params = quireink_sdk::Params::new().with("id", "x");
    kernel.apply(Command::Schedule(
        event.clone(),
        params.clone(),
        Instant(9000),
    ));
    // Scheduled again: moved, not doubled.
    kernel.apply(Command::Schedule(event, params, Instant(5000)));
    assert_eq!(kernel.finish().wake_at(), Some(Instant(5000)));
    log.borrow_mut().clear();
    kernel.tick(Instant(4999));
    assert!(log.borrow().is_empty());
    let effects = kernel.tick(Instant(5000));
    assert_eq!(*log.borrow(), ["event ov.later x"]);
    assert_eq!(effects.wake_at(), None);
}

#[test]
fn bindings_change_at_once_and_are_saved() {
    let log = log();
    let mut kernel = Kernel::builder(DEVICE)
        .plugin(OverlayPlugin { log: log.clone() })
        .storage(quireink_sdk::MemoryStorage::new())
        .build()
        .unwrap();
    kernel.start(Instant(0));
    kernel.apply(Command::Bind {
        layer: String::from("global"),
        input: String::from("button.menu"),
        action: Some(actions::TOGGLE_NIGHT_MODE),
    });
    kernel.finish();
    press(&mut kernel, 100, "menu");
    assert_eq!(
        kernel.active_screen().map(Id::as_str),
        Some("ov.base"),
        "no menu"
    );
    assert!(kernel.settings().bool("core.night-mode"));
    let saved = quireink_sdk::read_file(
        kernel.shared.storage.as_ref(),
        quireink_sdk::builtin::CONFIG_FILE,
        4096,
    )
    .unwrap();
    assert!(String::from_utf8_lossy(&saved).contains("menu = core.toggle-night-mode"));

    // Back to what the plugins bind: the menu again.
    kernel.apply(Command::Bind {
        layer: String::from("global"),
        input: String::from("button.menu"),
        action: None,
    });
    kernel.finish();
    press(&mut kernel, 300, "menu");
    assert_eq!(kernel.active_screen().map(Id::as_str), Some("ov.half"));
}

#[test]
fn screens_below_overlays_redraw_themselves() {
    let log = log();
    let mut kernel = Kernel::builder(DEVICE)
        .plugin(OverlayPlugin { log: log.clone() })
        .build()
        .unwrap();
    kernel.start(Instant(0));
    render(&mut kernel);
    press(&mut kernel, 100, "menu");
    render(&mut kernel);
    log.borrow_mut().clear();

    // The base screen changes while the overlay is open: the whole screen
    // is drawn again, the overlay on top.
    kernel.apply(Command::Emit(
        Id::new_static("ov.changed"),
        quireink_sdk::Params::new(),
    ));
    let effects = kernel.finish();
    assert_eq!(
        effects.refresh().map(|(r, _)| r),
        Some(Rect::new(0, 0, 90, 90))
    );
    render(&mut kernel);
    assert_eq!(*log.borrow(), ["event ov.changed ", "draw base 90x90"]);
    assert_eq!(kernel.frame().pixel(0, 60), Color::BLACK);
}

#[test]
fn open_slots_reopen_for_other_params() {
    let log = log();
    let mut kernel = Kernel::builder(DEVICE)
        .plugin(OverlayPlugin { log: log.clone() })
        .build()
        .unwrap();
    kernel.start(Instant(0));
    let open = |kernel: &mut Kernel, menu: &str| {
        kernel.apply(Command::OpenSlot(
            slots::MENU,
            quireink_sdk::Params::new().with("menu", menu.to_string()),
        ));
        kernel.finish();
    };
    let shown = |kernel: &Kernel| -> Vec<String> {
        kernel
            .stack
            .iter()
            .map(|s| alloc::format!("{} {}", s.id, s.params.get("menu").unwrap_or("-")))
            .collect()
    };
    open(&mut kernel, "a");
    assert_eq!(shown(&kernel), ["ov.base -", "ov.half a"]);
    // The same slot with the same params is just brought to the front.
    open(&mut kernel, "a");
    assert_eq!(shown(&kernel), ["ov.base -", "ov.half a"]);
    // Other params (e.g. another book) open it anew in its place.
    open(&mut kernel, "b");
    assert_eq!(shown(&kernel), ["ov.base -", "ov.half b"]);
}

#[test]
fn menus_follow_order_and_user_definitions() {
    let log = log();
    let kernel = Kernel::builder(DEVICE)
        .plugin(OverlayPlugin { log: log.clone() })
        .build()
        .unwrap();
    let env_items = |kernel: &Kernel| -> Vec<String> {
        kernel
            .shared
            .menus
            .items("core.main")
            .iter()
            .map(|i| i.id.to_string())
            .collect()
    };
    let items = env_items(&kernel);
    // Entries of the core plugin and this plugin, sorted by order: a
    // short list, the tools in a menu of their own.
    assert_eq!(
        items.iter().take(3).map(String::as_str).collect::<Vec<_>>(),
        ["core.home", "core.home", "core.library"]
    );
    assert!(items.iter().any(|i| i == "core.tools"));
    assert!(!kernel.shared.menus.items("core.main")[0].section);
    assert_eq!(kernel.shared.menus.icon("core.library"), Some("books"));
    assert_eq!(kernel.shared.menus.icon("core.tools"), Some("apps"));
    assert!(items.iter().filter(|a| *a == "core.home").count() == 2);

    let kernel = Kernel::builder(DEVICE)
        .plugin(OverlayPlugin { log })
        .user_config("[menus]\ncore.main = core.settings, core.toggle-night-mode\n")
        .build()
        .unwrap();
    assert_eq!(
        env_items(&kernel),
        ["core.settings", "core.toggle-night-mode"]
    );
}

#[test]
fn menus_are_redefined_saved_and_reset() {
    let log = log();
    let storage = quireink_sdk::MemoryStorage::new().with(
        "quireink.ini",
        "[core]
ui-size = 12
",
    );
    let mut kernel = Kernel::builder(DEVICE)
        .storage(storage)
        .plugin(OverlayPlugin { log })
        .build()
        .unwrap();
    let items = |kernel: &Kernel| -> Vec<String> {
        kernel
            .shared
            .menus
            .items("core.main")
            .iter()
            .map(|i| i.id.to_string())
            .collect()
    };
    let file = |kernel: &Kernel| -> String {
        let mut buf = [0u8; 256];
        let n = kernel
            .shared
            .storage
            .read_at("quireink.ini", 0, &mut buf)
            .unwrap();
        String::from(core::str::from_utf8(&buf[..n]).unwrap())
    };
    let defaults = items(&kernel);
    kernel.set_menu(
        Id::new_static("core.main"),
        Some(alloc::vec![
            Id::new_static("core.settings"),
            Id::new_static("core.home")
        ]),
    );
    assert_eq!(items(&kernel), ["core.settings", "core.home"]);
    assert!(kernel.shared.menus.is_customized("core.main"));
    assert_eq!(
        file(&kernel),
        "[core]
ui-size = 12

[menus]
core.main = core.settings, core.home
"
    );
    kernel.set_menu(Id::new_static("core.main"), None);
    assert_eq!(items(&kernel), defaults);
    assert_eq!(
        file(&kernel),
        "[core]
ui-size = 12

[menus]
"
    );
}

#[test]
fn http_requests_go_to_the_shell_and_answers_come_back() {
    let request = |id: &str| quireink_sdk::HttpRequest {
        headers: alloc::vec![(String::from("accept"), String::from("text/plain"))],
        params: quireink_sdk::Params::new().with("id", String::from(id)),
        ..quireink_sdk::HttpRequest::new("GET", "https://example.org/x", Id::new_static("ov.reply"))
    };
    // With network: the shell gets it and answers.
    let log = log();
    let mut kernel = Kernel::builder(DEVICE)
        .plugin(OverlayPlugin { log: log.clone() })
        .network(true)
        .build()
        .unwrap();
    kernel.start(Instant(0));
    kernel.request(request("a"));
    let calls = kernel.take_requests();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        (calls[0].method.as_str(), calls[0].url.as_str()),
        ("GET", "https://example.org/x")
    );
    assert!(kernel.take_requests().is_empty());
    kernel.http_response(
        Instant(5),
        crate::HttpResponse {
            id: calls[0].id,
            status: 200,
            body: b"ok".to_vec(),
            etag: None,
            ..Default::default()
        },
    );
    assert!(log.borrow().contains(&String::from("event ov.reply a")));
    // Without: it fails at once.
    let log = self::log();
    let mut kernel = Kernel::builder(DEVICE)
        .plugin(OverlayPlugin { log: log.clone() })
        .build()
        .unwrap();
    kernel.start(Instant(0));
    kernel.request(request("b"));
    kernel.run_commands();
    assert!(kernel.take_requests().is_empty());
    assert!(log.borrow().contains(&String::from("event ov.reply b")));
}

#[test]
fn plugins_serve_through_the_shell() {
    use quireink_sdk::server_params as p;
    let log = log();
    let mut kernel = Kernel::builder(DEVICE)
        .plugin(OverlayPlugin { log: log.clone() })
        .storage(quireink_sdk::MemoryStorage::new())
        .network(true)
        .build()
        .unwrap();
    kernel.start(Instant(0));
    let events = Id::new_static("ov.reply");
    kernel.output.commands.push(Command::Serve {
        server: Id::new_static("ov.server"),
        port: 8080,
        events: events.clone(),
    });
    kernel.run_commands();
    assert!(kernel.serving());
    let calls = kernel.take_server_calls();
    let crate::ServerCall::Listen { server, port } = calls[0] else {
        panic!("{calls:?}");
    };
    assert_eq!(port, 8080);
    kernel.server_event(
        Instant(1),
        crate::ServerEvent::Request {
            server,
            request: 7,
            method: String::from("PUT"),
            target: String::from("/up?name=a"),
            body: true,
        },
    );
    // The plugin hears of it once the body is complete.
    let told = |log: &Log| log.borrow().iter().any(|l| l.starts_with("event ov.reply"));
    assert!(!told(&log));
    for (data, last) in [(&b"da"[..], false), (&b"ta"[..], true)] {
        kernel.server_event(
            Instant(1),
            crate::ServerEvent::Body {
                request: 7,
                data: data.to_vec(),
                last,
            },
        );
    }
    assert!(told(&log));
    let path = String::from(".quireink/tmp/request-7");
    let storage = kernel.shared.storage.as_ref();
    assert_eq!(
        quireink_sdk::read_file(storage, &path, 64).unwrap(),
        b"data"
    );
    kernel
        .output
        .commands
        .push(Command::Respond(quireink_sdk::ServerResponse::new(
            7,
            200,
            "text/plain",
            b"ok".to_vec(),
        )));
    // A file goes in parts, read by the shell.
    let mut response = quireink_sdk::ServerResponse::new(8, 200, "text/plain", Vec::new());
    response.body_from = Some(String::from("served.txt"));
    kernel
        .shared
        .storage
        .write("served.txt", b"0123456789")
        .unwrap();
    kernel.output.commands.push(Command::Respond(response));
    kernel.run_commands();
    let calls = kernel.take_server_calls();
    assert!(
        matches!(&calls[0], crate::ServerCall::Respond { request: 7, status: 200, body, .. } if body == b"ok")
    );
    let crate::ServerCall::Respond {
        file: Some(file), ..
    } = &calls[1]
    else {
        panic!("{calls:?}");
    };
    assert_eq!(file.size, 10);
    let mut part = [0u8; 4];
    assert_eq!(kernel.read_served(&file.path, 8, &mut part).unwrap(), 2);
    assert_eq!(&part[..2], b"89");
    assert!(
        !kernel.shared.storage.exists(&path),
        "the body is cleaned up"
    );
    kernel
        .output
        .commands
        .push(Command::StopServing(Id::new_static("ov.server")));
    kernel.run_commands();
    assert!(!kernel.serving());
    assert!(matches!(
        kernel.take_server_calls()[0],
        crate::ServerCall::Stop { .. }
    ));
    // A server stops with the screen it was started on.
    kernel.output.commands.push(Command::OpenScreen(
        Id::new_static("ov.base"),
        quireink_sdk::Params::new(),
    ));
    kernel.run_commands();
    kernel.output.commands.push(Command::Serve {
        server: Id::new_static("ov.server"),
        port: 8080,
        events: events.clone(),
    });
    kernel.run_commands();
    assert!(kernel.serving());
    kernel.take_server_calls();
    kernel.output.commands.push(Command::Back);
    kernel.run_commands();
    assert!(!kernel.serving());
    assert!(matches!(
        kernel.take_server_calls()[0],
        crate::ServerCall::Stop { .. }
    ));
    let _ = p::KIND;
}

#[test]
fn files_go_up_and_come_down() {
    let storage = quireink_sdk::MemoryStorage::new().with("up.bin", b" data".to_vec());
    let mut kernel = Kernel::builder(DEVICE)
        .storage(storage)
        .network(true)
        .build()
        .unwrap();
    kernel.start(Instant(0));
    let reply = Id::new_static("core.test-reply");
    kernel.request(quireink_sdk::HttpRequest {
        body_from: Some(String::from("up.bin")),
        save_to: Some(String::from("down.bin")),
        ..quireink_sdk::HttpRequest::new("PUT", "https://example.org/f", reply.clone())
    });
    let calls = kernel.take_requests();
    assert_eq!(calls[0].body.as_deref(), Some(&b" data"[..]));
    assert_eq!(calls[0].max_response, crate::MAX_FILE);
    kernel.http_response(
        Instant(1),
        crate::HttpResponse {
            id: calls[0].id,
            status: 200,
            body: b"\xffbinary".to_vec(),
            etag: Some(String::from("\"abc\"")),
            content_type: Some(String::from("application/epub+zip")),
            url: Some(String::from("https://example.org/g")),
        },
    );
    let storage = kernel.shared.storage.as_ref();
    assert_eq!(
        quireink_sdk::read_file(storage, "down.bin", 64).unwrap(),
        b"\xffbinary"
    );
    // A file that is not there fails at once.
    kernel.request(quireink_sdk::HttpRequest {
        body_from: Some(String::from("missing.bin")),
        ..quireink_sdk::HttpRequest::new("PUT", "https://example.org/f", reply)
    });
    assert!(kernel.take_requests().is_empty());
}

#[test]
fn copied_text_reaches_plugins_and_the_shell() {
    let log = log();
    let mut kernel = Kernel::builder(DEVICE)
        .plugin(OverlayPlugin { log: log.clone() })
        .build()
        .unwrap();
    kernel.start(Instant(0));
    assert!(!kernel.shared.env(Instant(0)).clipboard);
    let read = |id: &str| {
        Command::ReadClipboard(
            Id::new_static("ov.reply"),
            quireink_sdk::Params::new().with("id", String::from(id)),
        )
    };
    kernel
        .output
        .commands
        .extend([Command::SetClipboard(String::from("Alice")), read("a")]);
    kernel.run_commands();
    assert!(kernel.shared.env(Instant(0)).clipboard);
    assert_eq!(kernel.take_clipboard().as_deref(), Some("Alice"));
    assert_eq!(kernel.take_clipboard(), None);
    // Without the system's clipboard, the answer comes at once.
    assert!(!kernel.wants_clipboard());
    assert!(log.borrow().contains(&String::from("event ov.reply a")));
    // Long text is cut at a character boundary.
    let long = "ä".repeat(quireink_sdk::MAX_CLIPBOARD);
    assert_eq!(crate::clipped(&long).len(), quireink_sdk::MAX_CLIPBOARD);

    // With it: the shell reads it when asked.
    let log = self::log();
    let mut kernel = Kernel::builder(DEVICE)
        .plugin(OverlayPlugin { log: log.clone() })
        .system_clipboard(true)
        .build()
        .unwrap();
    kernel.start(Instant(0));
    kernel.set_clipboard_available(true);
    assert!(kernel.shared.env(Instant(0)).clipboard);
    kernel.output.commands.push(read("b"));
    kernel.run_commands();
    assert!(kernel.wants_clipboard());
    assert!(!log.borrow().contains(&String::from("event ov.reply b")));
    kernel.clipboard_text(Instant(1), Some(String::from("from another app")));
    assert!(!kernel.wants_clipboard());
    assert!(log.borrow().contains(&String::from("event ov.reply b")));
}

#[test]
fn secrets_are_kept_apart_and_read_at_start() {
    use quireink_sdk::secrets::{FILE, SecretChange};
    let mut kernel = Kernel::builder(DEVICE)
        .storage(quireink_sdk::MemoryStorage::new())
        .build()
        .unwrap();
    kernel.start(Instant(0));
    kernel
        .output
        .commands
        .push(Command::SetSecret(SecretChange::keep(
            "a.key", "a.label", "hunter2",
        )));
    kernel.run_commands();
    assert_eq!(
        kernel.shared.env(Instant(0)).secrets.get("a.key"),
        Some("hunter2")
    );
    let data = quireink_sdk::read_file(kernel.shared.storage.as_ref(), FILE, 4096).unwrap();
    assert!(!String::from_utf8_lossy(&data).contains("hunter2"));
    // Read again at the next start.
    let mut kernel = Kernel::builder(DEVICE)
        .storage(quireink_sdk::MemoryStorage::new().with(FILE, data))
        .build()
        .unwrap();
    kernel.start(Instant(0));
    assert_eq!(
        kernel.shared.env(Instant(0)).secrets.get("a.key"),
        Some("hunter2")
    );
    // The last one removed, the file goes.
    kernel
        .output
        .commands
        .push(Command::SetSecret(SecretChange::remove("a.key")));
    kernel.run_commands();
    assert!(kernel.shared.env(Instant(0)).secrets.is_empty());
    assert!(!kernel.shared.storage.exists(FILE));
}

#[test]
fn changed_settings_are_saved_and_announced() {
    let log = log();
    let storage = quireink_sdk::MemoryStorage::new()
        .with("quireink.ini", "# my settings\n[core]\nui-size = 12\n");
    let mut kernel = Kernel::builder(DEVICE)
        .storage(storage)
        .plugin(OverlayPlugin { log: log.clone() })
        .build()
        .unwrap();
    assert_eq!(kernel.settings().int("core.ui-size"), 12);
    kernel.start(Instant(0));
    log.borrow_mut().clear();

    // Long tap in the centre runs core.toggle-night-mode.
    let down = InputEvent::Pointer(PointerEvent {
        id: 0,
        phase: PointerPhase::Down,
        point: Point::new(45, 45),
        tool: PointerTool::Finger,
        pressure: 1000,
    });
    kernel.handle(Instant(100), down);
    kernel.tick(Instant(600));
    assert!(kernel.settings().bool("core.night-mode"));
    assert!(
        log.borrow()
            .contains(&String::from("event core.setting-changed core.night-mode"))
    );

    let mut buf = [0u8; 256];
    let n = kernel
        .shared
        .storage
        .read_at("quireink.ini", 0, &mut buf)
        .unwrap();
    assert_eq!(
        core::str::from_utf8(&buf[..n]).unwrap(),
        "# my settings\n[core]\nui-size = 12\nnight-mode = on\n"
    );
}

#[test]
fn plugins_switch_on_and_off_for_the_next_start() {
    use quireink_sdk::PluginState::{Off, Running};

    let log = log();
    let storage = quireink_sdk::MemoryStorage::new().with(
        "quireink.ini",
        "[plugins]
b = off
",
    );
    let mut kernel = Kernel::builder(DEVICE)
        .storage(storage)
        .plugin(OverlayPlugin { log: log.clone() })
        .plugin(TestPlugin::new("a", &log))
        .plugin(TestPlugin::new("b", &log))
        .build()
        .unwrap();
    let states: Vec<(&str, quireink_sdk::PluginState)> = kernel
        .plugins()
        .iter()
        .map(|p| (p.manifest.id.as_str(), p.state))
        .collect();
    assert_eq!(
        states,
        [
            ("core", Running),
            ("ov", Running),
            ("a", Running),
            ("b", Off)
        ]
    );
    assert!(kernel.settings().get("b.level").is_none());
    // "ov" alone provides the menu; the home screen has two providers.
    let ov = kernel.plugins().get("ov").unwrap();
    assert_eq!(ov.required_for.iter().collect::<Vec<_>>(), [&slots::MENU]);
    assert!(!kernel.plugins().restart_needed());

    kernel.apply(Command::SetPluginEnabled(Id::new_static("a"), false));
    kernel.apply(Command::SetPluginEnabled(Id::new_static("b"), true));
    // The kernel and plugins that alone provide a slot stay on.
    kernel.apply(Command::SetPluginEnabled(Id::new_static("core"), false));
    kernel.apply(Command::SetPluginEnabled(Id::new_static("ov"), false));
    assert_eq!(kernel.take_warnings().len(), 2);
    assert!(kernel.plugins().get("ov").unwrap().enabled);
    assert!(kernel.plugins().get("a").unwrap().restart_needed());
    assert!(kernel.plugins().restart_needed());

    let mut buf = [0u8; 64];
    let n = kernel
        .shared
        .storage
        .read_at("quireink.ini", 0, &mut buf)
        .unwrap();
    assert_eq!(
        core::str::from_utf8(&buf[..n]).unwrap(),
        "[plugins]
b = on
a = off
"
    );

    kernel.apply(Command::Restart);
    assert!(kernel.finish().restart());
}

#[test]
fn data_plugins_load_from_storage() {
    use quireink_sdk::PluginState::{Off, Running};

    let files = |config: &str| {
        quireink_sdk::MemoryStorage::new()
            .with(
                "plugins/large/plugin.ini",
                "[plugin]
name = Large print
description = Bigger text.
",
            )
            .with(
                "plugins/large/settings.ini",
                "[core]
ui-size = 14
night-mode = on
",
            )
            .with(
                "plugins/large/i18n/de.ini",
                "[meta]
language = de
name = Deutsch
plural = one-other

[messages]
large.plugin.name = Großdruck
",
            )
            .with("quireink.ini", config)
    };

    // Its defaults apply; the user's own choice wins.
    let kernel = Kernel::builder(DEVICE)
        .storage(files(
            "[core]
night-mode = off
",
        ))
        .build()
        .unwrap();
    let large = kernel.plugins().get("large").unwrap();
    assert_eq!(large.state, Running);
    assert_eq!(large.manifest.name, "Large print");
    assert_eq!(kernel.settings().int("core.ui-size"), 14);
    assert!(!kernel.settings().bool("core.night-mode"));

    // Switched off: listed with its translated name, no defaults.
    let mut kernel = Kernel::builder(DEVICE)
        .storage(files(
            "[plugins]
large = off
",
        ))
        .build()
        .unwrap();
    kernel.shared.translator.set_language("de");
    let large = kernel.plugins().get("large").unwrap();
    assert_eq!(large.state, Off);
    assert_eq!(large.name(&kernel.shared.translator), "Großdruck");
    assert_ne!(kernel.settings().int("core.ui-size"), 14);
}

#[test]
fn keymap_presets_follow_device_defaults_and_switch_at_runtime() {
    let log = log();
    let mut kernel = Kernel::builder(DEVICE)
        .plugin(TestPlugin::new("a", &log))
        .setting_default("core.keymap-preset", "core.two-buttons")
        .build()
        .unwrap();
    assert!(kernel.take_warnings().is_empty());
    assert_eq!(
        kernel.settings().text("core.keymap-preset"),
        "core.two-buttons"
    );
    kernel.start(Instant(0));

    // Two-button preset: forward is "next", which falls back to next page;
    // a long press on forward confirms.
    press(&mut kernel, 100, "page-forward");
    kernel.handle(
        Instant(300),
        InputEvent::Button {
            name: "page-forward",
            state: ButtonState::Pressed,
        },
    );
    kernel.tick(Instant(900));
    assert_eq!(
        *log.borrow(),
        ["a:core.next", "a:core.next-page", "a:core.confirm"]
    );

    // Back to the standard layout: forward turns pages directly again.
    log.borrow_mut().clear();
    kernel.handle(
        Instant(1000),
        InputEvent::Button {
            name: "page-forward",
            state: ButtonState::Released,
        },
    );
    kernel
        .shared
        .settings
        .set(
            "core.keymap-preset",
            quireink_sdk::SettingValue::Text(Cow::Borrowed("core.standard")),
        )
        .unwrap();
    kernel.rebuild_keymap();
    press(&mut kernel, 1100, "page-forward");
    assert_eq!(*log.borrow(), ["a:core.next-page"]);

    // Unknown device defaults are reported.
    let mut kernel = Kernel::builder(DEVICE)
        .setting_default("core.keymap-preset", "nope.preset")
        .build()
        .unwrap();
    assert_eq!(kernel.take_warnings().len(), 1);
}

#[test]
fn banded_frames_match_full_frames() {
    // Renders into a copy of the whole screen, as a panel controller keeps it.
    fn screen(kernel: &mut Kernel, copy: &mut quireink_graphics::Framebuffer) -> Vec<u8> {
        let mut bands = 0;
        kernel.render(&mut |top, rows| {
            bands += 1;
            copy.write_rows(top, rows);
        });
        assert!(bands > 0);
        copy.view().data.to_vec()
    }
    let build = |rows: Option<u16>| {
        let builder = Kernel::builder(DEVICE).plugin(OverlayPlugin { log: log() });
        match rows {
            Some(rows) => builder.frame_rows(rows),
            None => builder,
        }
        .build()
        .unwrap()
    };
    let new_copy = || {
        let display = DEVICE.display;
        quireink_graphics::Framebuffer::new(display.width, display.height, display.format).unwrap()
    };
    let (mut full, mut banded) = (build(None), build(Some(16)));
    let (mut full_copy, mut banded_copy) = (new_copy(), new_copy());
    full.start(Instant(0));
    banded.start(Instant(0));
    assert_eq!(
        screen(&mut full, &mut full_copy),
        screen(&mut banded, &mut banded_copy)
    );
    // An overlay redraws only part of the screen, across band borders.
    for kernel in [&mut full, &mut banded] {
        for (at, state) in [(100, ButtonState::Pressed), (150, ButtonState::Released)] {
            kernel.handle(
                Instant(at),
                InputEvent::Button {
                    name: "menu",
                    state,
                },
            );
        }
    }
    assert_eq!(
        screen(&mut full, &mut full_copy),
        screen(&mut banded, &mut banded_copy)
    );
    assert!(banded.frame().height <= 16);
}

#[test]
fn bands_pass_whole_aligned_rows_and_again() {
    let mut kernel = Kernel::builder(DEVICE)
        .plugin(OverlayPlugin { log: log() })
        .frame_rows(16)
        .row_align(8)
        .build()
        .unwrap();
    let passed = |kernel: &mut Kernel, again: bool| {
        let mut parts = Vec::new();
        let mut out = |top: u16, rows: quireink_platform::FrameView<'_>| {
            parts.push((top, rows.height, rows.width))
        };
        if again {
            kernel.rerender(&mut out);
        } else {
            kernel.render(&mut out);
        }
        parts
    };
    kernel.start(Instant(0));
    passed(&mut kernel, false);
    // The overlay covers rows 45 to 90: passed from row 40, in bands of 16.
    for (at, state) in [(100, ButtonState::Pressed), (150, ButtonState::Released)] {
        kernel.handle(
            Instant(at),
            InputEvent::Button {
                name: "menu",
                state,
            },
        );
    }
    let parts = passed(&mut kernel, false);
    assert_eq!(
        parts,
        [(40, 8, 90), (48, 16, 90), (64, 16, 90), (80, 10, 90)]
    );
    // Drawn again the same, e.g. for the controller's memory of the frame shown.
    assert_eq!(passed(&mut kernel, true), parts);
}

#[test]
fn the_power_button_puts_the_device_to_sleep_where_the_shell_can() {
    // Where the system sleeps on its own (Android), nothing happens.
    let mut kernel = Kernel::builder(DEVICE).build().unwrap();
    kernel.start(Instant(0));
    assert!(!kernel.shared.env(Instant(0)).sleep);
    assert!(!press(&mut kernel, 100, "power").sleep());
    // Where the shell sleeps, it is told to.
    let mut kernel = Kernel::builder(DEVICE).sleep(true).build().unwrap();
    kernel.start(Instant(0));
    assert!(kernel.shared.env(Instant(0)).sleep);
    assert!(press(&mut kernel, 100, "power").sleep());
    assert!(!press(&mut kernel, 200, "confirm").sleep());
}

#[test]
fn the_device_sleeps_by_itself_after_a_while_unused() {
    let mut kernel = Kernel::builder(DEVICE)
        .sleep(true)
        .setting_default("core.sleep-after", "1")
        .build()
        .unwrap();
    let effects = kernel.start(Instant(0));
    // A wake-up is asked for when the minute is over.
    assert_eq!(effects.wake_at(), Some(Instant(60_000)));
    // Use puts it off.
    press(&mut kernel, 30_000, "confirm");
    assert!(!kernel.tick(Instant(60_000)).sleep());
    assert!(kernel.tick(Instant(90_100)).sleep());
    // Never, where the setting says so.
    let mut kernel = Kernel::builder(DEVICE)
        .sleep(true)
        .setting_default("core.sleep-after", "0")
        .build()
        .unwrap();
    assert_eq!(kernel.start(Instant(0)).wake_at(), None);
    assert!(!kernel.tick(Instant(10_000_000)).sleep());
}

#[test]
fn speech_goes_to_the_shell_and_stops_with_its_screen() {
    let speech = |id: &str| {
        let mut speech = quireink_sdk::Speech::new("Hello", Id::new_static("ov.reply"));
        speech.params = quireink_sdk::Params::new().with("id", String::from(id));
        Command::Speak(speech)
    };
    // Without speech, the reply comes at once.
    let log = log();
    let mut kernel = Kernel::builder(DEVICE)
        .plugin(OverlayPlugin { log: log.clone() })
        .build()
        .unwrap();
    kernel.start(Instant(0));
    assert!(!kernel.shared.env(Instant(0)).speech);
    kernel.output.commands.push(speech("a"));
    kernel.run_commands();
    assert!(kernel.take_speech().is_empty());
    assert!(log.borrow().contains(&String::from("event ov.reply a")));

    // With it: the shell speaks and says when it is done.
    let log = self::log();
    let mut kernel = Kernel::builder(DEVICE)
        .plugin(OverlayPlugin { log: log.clone() })
        .speech(true)
        .build()
        .unwrap();
    kernel.start(Instant(0));
    kernel.output.commands.push(speech("b"));
    kernel.run_commands();
    let calls = kernel.take_speech();
    let [SpeechCall::Say { id, text, rate, .. }] = calls.as_slice() else {
        panic!("{calls:?}");
    };
    assert_eq!((text.as_str(), *rate), ("Hello", 100));
    assert!(kernel.speaking());
    assert!(!log.borrow().contains(&String::from("event ov.reply b")));
    kernel.speech_done(Instant(5), *id, true);
    assert!(!kernel.speaking());
    assert!(log.borrow().contains(&String::from("event ov.reply b")));

    // Queued texts follow; each is answered when done.
    let queued = |id: &str| {
        let Command::Speak(mut speech) = speech(id) else {
            unreachable!()
        };
        speech.queue = true;
        Command::Speak(speech)
    };
    kernel.output.commands.extend([speech("q1"), queued("q2")]);
    kernel.run_commands();
    let calls = kernel.take_speech();
    let ids: Vec<(u32, bool)> = calls
        .iter()
        .filter_map(|c| match c {
            SpeechCall::Say { id, queue, .. } => Some((*id, *queue)),
            SpeechCall::Stop => None,
        })
        .collect();
    assert_eq!(ids.iter().map(|i| i.1).collect::<Vec<_>>(), [false, true]);
    kernel.speech_done(Instant(6), ids[0].0, true);
    assert!(kernel.speaking());
    assert!(log.borrow().contains(&String::from("event ov.reply q1")));
    kernel.speech_done(Instant(7), ids[1].0, true);
    assert!(!kernel.speaking());
    assert!(log.borrow().contains(&String::from("event ov.reply q2")));

    // Started over the menu, it stops when the menu closes.
    kernel.output.commands.push(Command::open_slot(slots::MENU));
    kernel.run_commands();
    kernel.output.commands.push(speech("c"));
    kernel.run_commands();
    let Some(SpeechCall::Say { id, .. }) = kernel.take_speech().pop() else {
        panic!("no speech");
    };
    kernel.output.commands.push(Command::Back);
    kernel.run_commands();
    assert_eq!(kernel.take_speech(), [SpeechCall::Stop]);
    assert!(!kernel.speaking());
    // An end reported late is not passed on.
    kernel.speech_done(Instant(9), id, true);
    assert!(!log.borrow().contains(&String::from("event ov.reply c")));
}
