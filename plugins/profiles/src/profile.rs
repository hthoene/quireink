//! Profiles: named sets of setting values, kept in the plugin's folder as
//! sections of an ini file (`[Night]`, then `id = value` lines), with the
//! name of the profile applied last.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::{groups, settings};
use quireink_sdk::{Command, Env, Id, SettingKind, Storage};

use crate::PLUGIN;

/// The settings a profile keeps: everything about reading, the display
/// and the status lines.
const GROUPS: [Id; 3] = [
    groups::READING,
    groups::DISPLAY,
    Id::new_static("reader.group.status"),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Profile {
    pub name: String,
    /// Setting ids and values as in `quireink.ini`.
    pub values: Vec<(Id, String)>,
}

fn path() -> String {
    format!("{}/profiles.ini", Env::plugin_dir(PLUGIN.as_str()))
}

fn last_path() -> String {
    format!("{}/last.txt", Env::plugin_dir(PLUGIN.as_str()))
}

pub(crate) fn load(storage: &dyn Storage) -> Vec<Profile> {
    let data = quireink_sdk::read_file(storage, &path(), 64 * 1024).unwrap_or_default();
    let mut profiles: Vec<Profile> = Vec::new();
    for line in String::from_utf8_lossy(&data).lines() {
        let line = line.trim();
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            profiles.push(Profile {
                name: name.trim().to_string(),
                values: Vec::new(),
            });
        } else if let (Some(profile), Some((id, value))) =
            (profiles.last_mut(), line.split_once('='))
            && let Ok(id) = Id::parse(id.trim())
        {
            profile.values.push((id, value.trim().to_string()));
        }
    }
    profiles
}

pub(crate) fn save(storage: &dyn Storage, profiles: &[Profile]) {
    let mut text = String::new();
    for profile in profiles {
        let name = profile.name.replace(['[', ']'], " ");
        let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
        text.push_str(&format!("[{name}]\n"));
        for (id, value) in &profile.values {
            text.push_str(&format!("{id} = {}\n", value.replace(['\n', '\r'], " ")));
        }
        text.push('\n');
    }
    let _ = storage.write(&path(), text.as_bytes());
}

/// The name of the profile applied last.
pub(crate) fn last(storage: &dyn Storage) -> String {
    let data = quireink_sdk::read_file(storage, &last_path(), 1024).unwrap_or_default();
    String::from_utf8_lossy(&data).trim().to_string()
}

pub(crate) fn set_last(storage: &dyn Storage, name: &str) {
    let _ = storage.write(&last_path(), name.as_bytes());
}

/// The current values of the settings a profile keeps.
pub(crate) fn capture(env: &Env<'_>) -> Vec<(Id, String)> {
    env.settings
        .specs()
        .filter(|spec| GROUPS.contains(&env.settings.group_of(spec)))
        // Folders and languages belong to the device, not to a way of
        // reading.
        .filter(|spec| !matches!(spec.kind, SettingKind::Text | SettingKind::Language))
        // Neither is a way of reading.
        .filter(|spec| spec.id != settings::SCOPE_ONLY && spec.id != settings::NOTICE_SECONDS)
        .filter_map(|spec| {
            let value = env.settings.get(spec.id.as_str())?;
            Some((spec.id.clone(), value.to_config_text()))
        })
        .collect()
}

/// The commands that apply `profile`: only settings that differ, and
/// only values their settings accept.
pub(crate) fn apply(env: &Env<'_>, profile: &Profile) -> Vec<Command> {
    profile
        .values
        .iter()
        .filter_map(|(id, text)| {
            let spec = env.settings.specs().find(|s| s.id == *id)?;
            let value = spec.kind.parse(text).filter(|v| spec.kind.accepts(v))?;
            (env.settings.get(id.as_str()) != Some(&value))
                .then(|| Command::SetSetting(id.clone(), value))
        })
        .collect()
}

/// Whether the settings are as `profile` has them.
pub(crate) fn in_use(env: &Env<'_>, profile: &Profile) -> bool {
    !profile.values.is_empty() && apply(env, profile).is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use quireink_sdk::MemoryStorage;

    #[test]
    fn profiles_are_kept() {
        let storage = MemoryStorage::new();
        assert!(load(&storage).is_empty());
        let profiles = [
            Profile {
                name: String::from("Night"),
                values: Vec::from([
                    (Id::new_static("core.night-mode"), String::from("on")),
                    (Id::new_static("reader.font-size"), String::from("12")),
                ]),
            },
            Profile {
                name: String::from("Large [print]"),
                values: Vec::from([(Id::new_static("reader.font-size"), String::from("18"))]),
            },
        ];
        save(&storage, &profiles);
        let loaded = load(&storage);
        assert_eq!(loaded[0], profiles[0]);
        assert_eq!(loaded[1].name, "Large print", "brackets would end the name");
        set_last(&storage, "Night");
        assert_eq!(last(&storage), "Night");
    }
}
