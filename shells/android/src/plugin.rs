//! The Android shell's own plugin: what only the platform can do, offered
//! through the same SDK as every other plugin.

use std::borrow::Cow;

use android_activity::AndroidApp;
use quireink_sdk::{
    ActionSpec, Command, Env, Id, Manifest, MenuEntry, Plugin, RegisterError, Registrar,
    SDK_VERSION, Version,
};

pub const PLUGIN: Id = Id::new_static("android");
/// Asks for access to the shared storage, where the books are.
pub const ALLOW_STORAGE: Id = Id::new_static("android.allow-storage");
/// The home screen's shortcuts (see the `home` plugin).
const HOME_ACTIONS: Id = Id::new_static("home.actions");

/// The Android plugin.
pub struct AndroidPlugin {
    pub app: AndroidApp,
    /// QuireInk can already read the shared storage.
    pub shared_storage: bool,
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
        r.add_translations(Cow::Borrowed(include_str!("../i18n/en.ini")))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/de.ini")))?;
        Ok(())
    }
}
