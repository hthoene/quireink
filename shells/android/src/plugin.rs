//! The Android shell's own plugin: what only the platform can do, offered
//! through the same SDK as every other plugin.

use std::borrow::Cow;

use android_activity::AndroidApp;
use quireink_sdk::builtin::{events, groups, menus, params};
use quireink_sdk::{
    ActionSpec, Command, Env, Id, Manifest, MenuEntry, Params, Plugin, RegisterError, Registrar,
    SDK_VERSION, SettingKind, SettingSpec, SettingValue, Version,
};

use crate::lights::Lights;

pub const PLUGIN: Id = Id::new_static("android");
/// Asks for access to the shared storage, where the books are.
pub const ALLOW_STORAGE: Id = Id::new_static("android.allow-storage");
/// Frontlight brightness (0 switches it off).
pub const FRONTLIGHT: Id = Id::new_static("android.frontlight");
/// Frontlight colour temperature (higher is warmer).
pub const WARMTH: Id = Id::new_static("android.warmth");
/// The home screen's shortcuts (see the `home` plugin).
const HOME_ACTIONS: Id = Id::new_static("home.actions");
/// The reader's menu (see the `reader` plugin).
const READER_MENU: Id = Id::new_static("reader.menu");

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
        r.add_translations(Cow::Borrowed(include_str!("../i18n/en.ini")))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/de.ini")))?;
        Ok(())
    }
}

impl AndroidPlugin {
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
            .description("android.frontlight.help"),
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
                .description("android.warmth.help"),
            )?;
            items.push(WARMTH);
        }
        for (n, item) in items.into_iter().enumerate() {
            for (menu, order) in [(READER_MENU, 32), (menus::MAIN, 35)] {
                r.add_menu_entry(MenuEntry {
                    menu,
                    item: item.clone(),
                    order: order + n as i16,
                })?;
            }
        }
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
