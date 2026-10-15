//! The catalogs the user added: a list in the plugin's folder, one
//! catalog a line with its fields separated by tabs. Until the user
//! changes it, the list holds a free catalog to start with.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::web::{clean_name, host};
use quireink_sdk::{Env, Storage};

use crate::PLUGIN;

/// The library folder setting of the standard library.
const LIBRARY: &str = "files.root";

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Catalog {
    pub name: String,
    pub url: String,
    pub user: String,
    pub password: String,
    /// Where syncing puts its books; empty for a folder named after the
    /// catalog in the library.
    pub folder: String,
    /// Synced with the others by *Sync catalogs*.
    pub sync: bool,
    /// A WebDAV folder (e.g. in Nextcloud) rather than an OPDS catalog.
    pub webdav: bool,
}

impl Catalog {
    /// A new catalog at `url`, named after its host.
    pub(crate) fn new(url: &str) -> Self {
        Self {
            name: host(url).trim_start_matches("www.").to_string(),
            url: url.to_string(),
            // Nextcloud's and ownCloud's addresses say so.
            webdav: url.contains("/remote.php/") || url.contains("/webdav"),
            ..Self::default()
        }
    }

    /// The folder its books are synced to.
    pub(crate) fn sync_folder(&self, env: &Env<'_>) -> String {
        if !self.folder.trim().is_empty() {
            return self.folder.trim().trim_matches('/').to_string();
        }
        let library = env.settings.text(LIBRARY).trim_matches('/');
        let name = clean_name(&self.name, 60).unwrap_or_else(|| String::from("OPDS"));
        if library.is_empty() {
            name
        } else {
            format!("{library}/{name}")
        }
    }

    /// The `Authorization` header for it, if it needs one.
    pub(crate) fn authorization(&self) -> Option<String> {
        (!self.user.is_empty()).then(|| quireink_sdk::web::basic_auth(&self.user, &self.password))
    }
}

fn path() -> String {
    format!("{}/catalogs.txt", Env::plugin_dir(PLUGIN.as_str()))
}

/// The catalogs; until the user changes them, one to start with.
pub(crate) fn load(storage: &dyn Storage) -> Vec<Catalog> {
    let Ok(data) = quireink_sdk::read_file(storage, &path(), 64 * 1024) else {
        return Vec::from([Catalog {
            name: String::from("Project Gutenberg"),
            url: String::from("https://www.gutenberg.org/ebooks.opds/"),
            ..Catalog::default()
        }]);
    };
    String::from_utf8_lossy(&data)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let name = fields.next()?.to_string();
            let url = fields.next().filter(|url| !url.is_empty())?.to_string();
            let mut field = || fields.next().unwrap_or("").to_string();
            Some(Catalog {
                name,
                url,
                user: field(),
                password: field(),
                folder: field(),
                sync: field() == "sync",
                webdav: field() == "webdav",
            })
        })
        .collect()
}

pub(crate) fn save(storage: &dyn Storage, catalogs: &[Catalog]) {
    let clean = |s: &str| s.replace(['\t', '\n', '\r'], " ");
    let mut text = String::new();
    for c in catalogs {
        text.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            clean(&c.name),
            clean(&c.url),
            clean(&c.user),
            clean(&c.password),
            clean(&c.folder),
            if c.sync { "sync" } else { "" },
            if c.webdav { "webdav" } else { "" },
        ));
    }
    let _ = storage.write(&path(), text.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use quireink_sdk::MemoryStorage;

    #[test]
    fn catalogs_are_kept() {
        let storage = MemoryStorage::new();
        assert_eq!(load(&storage)[0].name, "Project Gutenberg");
        let mut catalog = Catalog::new("https://www.example.org/opds");
        assert_eq!(catalog.name, "example.org");
        catalog.user = String::from("me");
        catalog.password = String::from("a\tb");
        catalog.sync = true;
        save(&storage, &[catalog.clone()]);
        let loaded = load(&storage);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].password, "a b");
        assert!(loaded[0].sync);
        assert_eq!(loaded[0].user, "me");
        save(&storage, &[]);
        assert!(load(&storage).is_empty(), "removed for good");
    }
}
