//! Data plugins: folders on the storage with a `plugin.ini` and data
//! files, no code. They are installed by copying a folder into a plugin
//! folder (e.g. `plugins/` on an SD card) and work on every device.
//!
//! ```text
//! plugins/large-print/
//!   plugin.ini     [plugin] id, name, description, version
//!   i18n/*.ini     translation catalogs (also whole new languages)
//!   keymap.ini     key bindings
//!   settings.ini   new defaults for any plugin's settings ([reader] font-size = 14)
//! ```
//!
//! The kernel lists them like every other plugin, so they can be switched
//! off in the plugin manager. Files are read once at start-up; only what a
//! plugin contributes stays in memory.

use alloc::borrow::Cow;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::RefCell;

use quireink_platform::Storage;
use quireink_sdk::{Id, Manifest, Plugin, RegisterError, Registrar, SDK_VERSION, Version};

/// Largest data file read (a translation catalog, a key map).
const MAX_FILE: usize = 64 * 1024;
/// Largest `plugin.ini`.
const MAX_MANIFEST: usize = 4 * 1024;

/// A plugin made of data files.
pub(crate) struct DataPlugin {
    manifest: Manifest,
    /// Taken by `register`, which runs once.
    translations: RefCell<Vec<String>>,
    keymap: RefCell<Option<String>>,
    settings: RefCell<Option<String>>,
}

impl Plugin for DataPlugin {
    fn manifest(&self) -> Manifest {
        self.manifest.clone()
    }

    fn register(&self, r: &mut dyn Registrar) -> Result<(), RegisterError> {
        for source in self.translations.take() {
            r.add_translations(Cow::Owned(source))?;
        }
        if let Some(source) = self.keymap.take() {
            r.add_keymap(Cow::Owned(source))?;
        }
        if let Some(source) = self.settings.take() {
            for entry in quireink_config::parse(&source) {
                let entry = entry.map_err(|error| RegisterError::Parse {
                    line: error.line,
                    message: Cow::Owned(error.to_string()),
                })?;
                let id = Id::parse(&format!("{}.{}", entry.section, entry.key)).map_err(|_| {
                    RegisterError::Parse {
                        line: entry.line,
                        message: Cow::Owned(format!(
                            "invalid setting `{}.{}`",
                            entry.section, entry.key
                        )),
                    }
                })?;
                r.set_setting_default(id, Cow::Owned(entry.value.as_str().into_owned()))?;
            }
        }
        Ok(())
    }
}

/// Every data plugin in the folders of `dir`, in name order. Folders
/// without a usable `plugin.ini` are reported through `warn` and skipped.
pub(crate) fn discover(
    storage: &dyn Storage,
    dir: &str,
    warn: &mut dyn FnMut(String),
) -> Vec<DataPlugin> {
    let mut folders: Vec<String> = Vec::new();
    let _ = storage.list(dir, &mut |entry| {
        if entry.is_dir && !entry.name.starts_with('.') {
            folders.push(entry.name.to_string());
        }
    });
    folders.sort_unstable();
    folders
        .iter()
        .filter_map(|name| {
            let path = format!("{dir}/{name}");
            load(storage, &path, name).map_err(&mut *warn).ok()
        })
        .collect()
}

/// The plugin in folder `path`.
fn load(storage: &dyn Storage, path: &str, folder: &str) -> Result<DataPlugin, String> {
    let manifest = read(storage, &format!("{path}/plugin.ini"), MAX_MANIFEST)
        .ok_or_else(|| format!("{path}: no plugin.ini"))?;
    let manifest =
        parse_manifest(&manifest, folder).map_err(|e| format!("{path}/plugin.ini: {e}"))?;
    let mut translations = Vec::new();
    let catalogs = format!("{path}/i18n");
    let mut files: Vec<String> = Vec::new();
    let _ = storage.list(&catalogs, &mut |entry| {
        if !entry.is_dir && entry.name.ends_with(".ini") {
            files.push(entry.name.to_string());
        }
    });
    files.sort_unstable();
    for file in files {
        let path = format!("{catalogs}/{file}");
        translations
            .push(read(storage, &path, MAX_FILE).ok_or_else(|| format!("{path}: unreadable"))?);
    }
    Ok(DataPlugin {
        manifest,
        translations: RefCell::new(translations),
        keymap: RefCell::new(read(storage, &format!("{path}/keymap.ini"), MAX_FILE)),
        settings: RefCell::new(read(storage, &format!("{path}/settings.ini"), MAX_FILE)),
    })
}

fn read(storage: &dyn Storage, path: &str, max: usize) -> Option<String> {
    let data = quireink_sdk::read_file(storage, path, max).ok()?;
    String::from_utf8(data).ok()
}

/// The `[plugin]` section of a `plugin.ini`. The id defaults to the
/// folder's name; the version to 0.1.0.
fn parse_manifest(source: &str, folder: &str) -> Result<Manifest, String> {
    let (mut id, mut name, mut description, mut version) = (None, None, None, None);
    for entry in quireink_config::parse(source) {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.section != "plugin" {
            continue;
        }
        let value = entry.value.as_str().into_owned();
        match entry.key {
            "id" => id = Some(value),
            "name" => name = Some(value),
            "description" => description = Some(value),
            "version" => version = Some(value),
            _ => {}
        }
    }
    let id = id.unwrap_or_else(|| folder.to_string());
    let id = Id::parse(&id)
        .ok()
        .filter(|id| id.as_str() == id.namespace())
        .ok_or_else(|| format!("invalid id `{id}`"))?;
    let version = match version {
        Some(text) => Version::parse(&text).ok_or_else(|| format!("invalid version `{text}`"))?,
        None => Version::new(0, 1, 0),
    };
    Ok(Manifest {
        name: Cow::Owned(name.unwrap_or_else(|| id.as_str().to_string())),
        id,
        description: Cow::Owned(description.unwrap_or_default()),
        version,
        sdk: SDK_VERSION,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use quireink_sdk::MemoryStorage;

    #[test]
    fn finds_folders_with_a_manifest() {
        let storage = MemoryStorage::new()
            .with(
                "plugins/large/plugin.ini",
                "[plugin]\nname = Large print\ndescription = Bigger text.\nversion = 1.2.0\n",
            )
            .with("plugins/large/settings.ini", "[core]\nui-size = 14\n")
            .with("plugins/large/i18n/de.ini", "[meta]\nlanguage = de\n")
            .with("plugins/broken/readme.txt", "")
            .with("plugins/bad-id/plugin.ini", "[plugin]\nid = a.b\n");
        let mut warnings = Vec::new();
        let plugins = discover(&storage, "plugins", &mut |w| warnings.push(w));
        assert_eq!(plugins.len(), 1);
        let large = &plugins[0];
        assert_eq!(large.manifest.id.as_str(), "large");
        assert_eq!(large.manifest.name, "Large print");
        assert_eq!(large.manifest.version, Version::new(1, 2, 0));
        assert_eq!(large.translations.borrow().len(), 1);
        assert!(large.settings.borrow().is_some());
        assert!(large.keymap.borrow().is_none());
        assert_eq!(warnings.len(), 2, "{warnings:?}");
    }
}
