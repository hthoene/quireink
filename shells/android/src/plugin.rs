//! The Android shell's own plugin: what only the platform can do, offered
//! through the same SDK as every other plugin.

use std::borrow::Cow;

use android_activity::AndroidApp;
use quireink_sdk::builtin::{events, groups, menus, params};
use quireink_sdk::document::Document;
use quireink_sdk::{
    ActionSpec, Canvas, Command, Cx, Env, FormatSpec, Handled, Id, Manifest, MenuEntry, MenuNeed,
    Params, Plugin, Rect, RegisterError, Registrar, SDK_VERSION, Screen, ScreenSpec, SettingKind,
    SettingSpec, SettingValue, Version,
};

use crate::lights::Lights;

pub const PLUGIN: Id = Id::new_static("android");
/// Asks for access to the shared storage, where the books are.
pub const ALLOW_STORAGE: Id = Id::new_static("android.allow-storage");
/// PDF books, rendered by the system (see `pdf`).
pub const PDF: Id = Id::new_static("android.pdf");
/// The voice that reads aloud: opens the system's speech settings.
pub const VOICE: Id = Id::new_static("android.voice");
const VOICE_SCREEN: Id = Id::new_static("android.voice-screen");
/// The reader's settings for reading aloud (see the `reader` plugin).
const ALOUD_SECTION: &str = "reader.section.aloud";
/// Frontlight brightness (0 switches it off).
pub const FRONTLIGHT: Id = Id::new_static("android.frontlight");
/// Frontlight colour temperature (higher is warmer).
pub const WARMTH: Id = Id::new_static("android.warmth");
/// The home screen's shortcuts (see the `home` plugin).
const HOME_ACTIONS: Id = Id::new_static("home.actions");
/// The reader's menu (see the `reader` plugin).
/// The reader's quick panel (font, layout and light).
const READER_PANEL: Id = Id::new_static("reader.layout");

/// The device's frontlight and its levels when QuireInk started.
#[derive(Clone, Copy)]
pub struct Frontlight {
    pub lights: Lights,
    pub brightness: i32,
    pub warmth: i32,
}

/// The Android plugin.
pub struct AndroidPlugin {
    pub app: AndroidApp,
    /// QuireInk can already read the shared storage.
    pub shared_storage: bool,
    pub frontlight: Option<Frontlight>,
    /// The screen's size in pixels, which PDF pages are rendered to fit.
    pub screen: (u32, u32),
}

impl Plugin for AndroidPlugin {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: PLUGIN,
            name: Cow::Borrowed("Android"),
            description: Cow::Borrowed("Connects QuireInk to Android, e.g. to your book folders."),
            version: Version::new(0, 1, 0),
            sdk: SDK_VERSION,
        }
    }

    fn register(&self, r: &mut dyn Registrar) -> Result<(), RegisterError> {
        let app = self.app.clone();
        r.add_action(ActionSpec {
            id: ALLOW_STORAGE,
            title: Cow::Borrowed("android.allow-storage"),
            handler: Some(Box::new(move |_: &Env<'_>, _: &mut Vec<Command>| {
                crate::access::request(&app);
            })),
        })?;
        if !self.shared_storage {
            // First on the home screen until access is granted.
            r.add_menu_entry(MenuEntry {
                menu: HOME_ACTIONS,
                item: ALLOW_STORAGE,
                order: 0,
            })?;
        }
        if let Some(frontlight) = self.frontlight {
            self.register_frontlight(r, frontlight)?;
        }
        self.register_voice(r)?;
        let (app, screen) = (self.app.clone(), self.screen);
        r.add_format(FormatSpec {
            id: PDF,
            title: Cow::Borrowed("android.pdf"),
            extensions: vec![Cow::Borrowed("pdf")],
            open: Box::new(move |storage, path, _| {
                crate::pdf::PdfDocument::open(&app, storage, path, screen)
                    .map(|d| Box::new(d) as Box<dyn Document>)
            }),
        })?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/en.ini")))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/de.ini")))?;
        Ok(())
    }
}

/// Opens the system's speech settings and closes at once: the editor of
/// [`VOICE`].
struct VoiceSettings {
    app: AndroidApp,
}

impl Screen for VoiceSettings {
    fn draw(&mut self, _: &mut Canvas<'_>, _: &Env<'_>) {}

    fn region(&self, _: &Env<'_>) -> Option<Rect> {
        Some(Rect::new(0, 0, 0, 0))
    }

    fn on_show(&mut self, cx: &mut Cx<'_, '_>) {
        if !crate::speech::open_settings(&self.app) {
            let text = cx.env().i18n.tr("android.voice.unavailable").to_string();
            cx.command(Command::Notify(text));
        }
        cx.command(Command::Back);
    }

    fn on_action(&mut self, _: &Id, _: &mut Cx<'_, '_>) -> Handled {
        Handled::No
    }
}

impl AndroidPlugin {
    /// The voice for reading aloud, with its other settings; it is
    /// chosen in the system's speech settings.
    fn register_voice(&self, r: &mut dyn Registrar) -> Result<(), RegisterError> {
        r.add_setting(
            SettingSpec::new(
                VOICE,
                "android.voice",
                SettingKind::Text,
                SettingValue::Text(Cow::Borrowed("")),
            )
            .group(groups::READING)
            .section(ALOUD_SECTION)
            .editor(VOICE_SCREEN)
            .description("android.voice.help"),
        )?;
        r.set_menu_need(VOICE, MenuNeed::Speech)?;
        let app = self.app.clone();
        r.add_screen(ScreenSpec {
            id: VOICE_SCREEN,
            title: Cow::Borrowed("android.voice"),
            factory: Box::new(move |_, _| Box::new(VoiceSettings { app: app.clone() })),
        })
    }

    /// Brightness and warmth as settings, also in the reader's and the
    /// main menu. They follow the device: its levels at start, and every
    /// change goes to the device.
    fn register_frontlight(
        &self,
        r: &mut dyn Registrar,
        frontlight: Frontlight,
    ) -> Result<(), RegisterError> {
        let lights = frontlight.lights;
        r.add_setting(
            SettingSpec::new(
                FRONTLIGHT,
                "android.frontlight",
                SettingKind::Int {
                    min: 0,
                    max: i64::from(lights.max_brightness),
                },
                SettingValue::Int(i64::from(frontlight.brightness)),
            )
            .group(groups::DISPLAY)
            .description("android.frontlight.help")
            .slider(),
        )?;
        let mut items = vec![FRONTLIGHT];
        if lights.has_warmth() {
            r.add_setting(
                SettingSpec::new(
                    WARMTH,
                    "android.warmth",
                    SettingKind::Int {
                        min: 0,
                        max: i64::from(lights.max_warmth),
                    },
                    SettingValue::Int(i64::from(frontlight.warmth)),
                )
                .group(groups::DISPLAY)
                .description("android.warmth.help")
                .slider(),
            )?;
            items.push(WARMTH);
        }
        // In the reader's quick panel, first, and with the device's
        // settings in the main menu.
        for (n, item) in items.into_iter().enumerate() {
            for (menu, order) in [(READER_PANEL, -10), (menus::MAIN, 82), (menus::QUICK, 20)] {
                r.add_menu_entry(MenuEntry {
                    menu,
                    item: item.clone(),
                    order: order + n as i16,
                })?;
            }
        }
        r.add_icon(FRONTLIGHT, Cow::Borrowed("sun"))?;
        r.add_icon(WARMTH, Cow::Borrowed("temperature"))?;
        let app = self.app.clone();
        r.subscribe(
            events::SETTING_CHANGED,
            Box::new(
                move |env: &Env<'_>, params: &Params, _: &mut Vec<Command>| {
                    let id = params.get(params::ID).unwrap_or_default();
                    let level = env.settings.int(id) as i32;
                    // Only real changes reach the device: taking over its
                    // own levels on start never writes back.
                    if id == FRONTLIGHT.as_str() && lights.brightness(&app) != level {
                        lights.set_brightness(&app, level);
                    } else if id == WARMTH.as_str() && lights.warmth(&app) != level {
                        lights.set_warmth(&app, level);
                    }
                },
            ),
        )?;
        // The device's levels are the truth (they also change through
        // the system's own controls): take them over on start.
        let app = self.app.clone();
        r.subscribe(
            events::STARTED,
            Box::new(move |env: &Env<'_>, _: &Params, out: &mut Vec<Command>| {
                let levels = [
                    (FRONTLIGHT, lights.brightness(&app)),
                    (WARMTH, lights.warmth(&app)),
                ];
                for (id, level) in levels {
                    let level = i64::from(level);
                    let known = env.settings.get(id.as_str()).is_some();
                    if known && env.settings.int(id.as_str()) != level {
                        out.push(Command::SetSetting(id, SettingValue::Int(level)));
                    }
                }
            }),
        )?;
        Ok(())
    }
}
