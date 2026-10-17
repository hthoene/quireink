//! The X4's own plugin: what only this device has to set (for
//! developers, in their group of settings).

use alloc::borrow::Cow;

use quireink_sdk::builtin::groups;
use quireink_sdk::{
    Id, Manifest, Plugin, RegisterError, Registrar, SDK_VERSION, SettingKind, SettingSpec,
    SettingValue, Version,
};

pub const PLUGIN: Id = Id::new_static("x4");
/// The processor sleeps lightly while idle on battery (see `power`).
pub const LIGHT_SLEEP: Id = Id::new_static("x4.light-sleep");

#[derive(Debug, Default)]
pub struct X4;

impl Plugin for X4 {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: PLUGIN,
            name: Cow::Borrowed("Xteink X4"),
            description: Cow::Borrowed("What only the Xteink X4 has to set."),
            version: Version::new(0, 1, 0),
            sdk: SDK_VERSION,
        }
    }

    fn register(&self, r: &mut dyn Registrar) -> Result<(), RegisterError> {
        r.add_setting(
            SettingSpec::new(
                LIGHT_SLEEP,
                "x4.light-sleep",
                SettingKind::Bool,
                SettingValue::Bool(true),
            )
            .group(groups::DEVELOPER)
            .description("x4.light-sleep.help"),
        )?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/en.ini")))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/de.ini")))?;
        Ok(())
    }
}
