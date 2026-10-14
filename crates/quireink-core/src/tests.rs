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
    ActionSpec, Canvas, Cx, Env, Handled, Id, Manifest, Plugin, Priority, RegisterError, Registrar,
    SDK_VERSION, Screen, ScreenSpec, SettingKind, SettingSpec, SettingValue, Version,
};

use crate::{Kernel, WarningSource};

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
    },
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
        r.add_setting(SettingSpec {
            id: Id::parse(&alloc::format!("{}.level", self.id)).unwrap(),
            title: Cow::Borrowed("test.level"),
            kind: SettingKind::Int { min: 1, max: 5 },
            default: SettingValue::Int(2),
        })?;
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
            cx.canvas().fill_rect(20, 20, 5, 5, Color::BLACK);
            cx.request_refresh(RefreshMode::Partial);
            return Handled::Yes;
        }
        if action.namespace() == self.name || *action == actions::CONFIRM {
            return Handled::Yes;
        }
        Handled::No
    }
}

fn press(kernel: &mut Kernel, at: u64, name: &'static str) -> crate::Effects {
    kernel.handle(
        Instant(at),
        InputEvent::Button {
            name,
            state: ButtonState::Pressed,
        },
    );
    kernel.handle(
        Instant(at + 50),
        InputEvent::Button {
            name,
            state: ButtonState::Released,
        },
    )
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
    kernel.handle(Instant(at + 60), event(PointerPhase::Up))
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
    assert_eq!(kernel.plugins().len(), 1);

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
            r.add_font(quireink_sdk::FontSpec {
                id: Id::parse(name).unwrap(),
                font,
            })?;
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
