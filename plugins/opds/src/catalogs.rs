//! The catalogs the user added: a list in the plugin's folder, one
//! catalog a line with its fields separated by tabs. Until the user
//! changes it, the list holds a free catalog to start with. Their
//! passwords are kept with the other passwords and keys (see
//! `quireink_sdk::secrets`), by the catalog's address.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::secrets::SecretChange;
use quireink_sdk::web::{clean_name, host};
use quireink_sdk::{Arg, Command, Env};

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

/// The prefix of the catalogs' secrets.
const SECRETS: &str = "opds.";

/// The id of the password of the catalog at `url`.
fn secret_id(url: &str) -> String {
    let digest = quireink_sdk::md5::of(url);
    format!("{SECRETS}{}", &digest[..16])
}

/// The catalogs; until the user changes them, one to start with.
pub(crate) fn load(env: &Env<'_>) -> Vec<Catalog> {
    let Ok(data) = quireink_sdk::read_file(env.storage, &path(), 64 * 1024) else {
        return Vec::from([Catalog {
            name: String::from("Project Gutenberg"),
            url: String::from("https://www.gutenberg.org/ebooks.opds/"),
            ..Catalog::default()
        }]);
    };
    let mut catalogs = parse(&String::from_utf8_lossy(&data));
    for c in &mut catalogs {
        // Earlier versions kept it in the list (see `move_passwords`).
        if c.password.is_empty() {
            c.password = env
                .secrets
                .get(&secret_id(&c.url))
                .unwrap_or_default()
                .to_string();
        }
    }
    catalogs
}

/// Moves passwords kept in the list by earlier versions to the other
/// passwords and keys.
pub(crate) fn move_passwords(env: &Env<'_>, out: &mut Vec<Command>) {
    let Ok(data) = quireink_sdk::read_file(env.storage, &path(), 64 * 1024) else {
        return;
    };
    let catalogs = parse(&String::from_utf8_lossy(&data));
    if catalogs.iter().any(|c| !c.password.is_empty()) {
        save(env, &catalogs, out);
    }
}

/// The list as the file has it.
fn parse(text: &str) -> Vec<Catalog> {
    text.lines()
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

/// The file's text: the password's place stays, empty.
fn encode(catalogs: &[Catalog]) -> String {
    let clean = |s: &str| s.replace(['\t', '\n', '\r'], " ");
    let mut text = String::new();
    for c in catalogs {
        text.push_str(&format!(
            "{}\t{}\t{}\t\t{}\t{}\t{}\n",
            clean(&c.name),
            clean(&c.url),
            clean(&c.user),
            clean(&c.folder),
            if c.sync { "sync" } else { "" },
            if c.webdav { "webdav" } else { "" },
        ));
    }
    text
}

/// Saves the list, and the passwords with the other passwords and keys:
/// their changes go to `out`.
pub(crate) fn save(env: &Env<'_>, catalogs: &[Catalog], out: &mut Vec<Command>) {
    for c in catalogs {
        let id = secret_id(&c.url);
        let kept = env.secrets.get(&id);
        if c.password.is_empty() {
            if kept.is_some() {
                out.push(Command::SetSecret(SecretChange::remove(&id)));
            }
        } else if kept != Some(c.password.as_str()) {
            let label = env
                .i18n
                .format("opds.secret", &[("name", Arg::Str(&c.name))]);
            out.push(Command::SetSecret(SecretChange::keep(
                &id,
                &label,
                &c.password,
            )));
        }
    }
    // Those of catalogs removed or moved to another address go.
    let ids: Vec<String> = catalogs.iter().map(|c| secret_id(&c.url)).collect();
    for secret in env.secrets.iter() {
        if secret.id.starts_with(SECRETS) && !ids.contains(&secret.id) {
            out.push(Command::SetSecret(SecretChange::remove(&secret.id)));
        }
    }
    let _ = env.storage.write(&path(), encode(catalogs).as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogs_are_kept_without_passwords() {
        let mut catalog = Catalog::new("https://www.example.org/opds");
        assert_eq!(catalog.name, "example.org");
        catalog.user = String::from("me");
        catalog.password = String::from("s3cret");
        catalog.folder = String::from("Books/Example");
        catalog.sync = true;
        let text = encode(&[catalog.clone()]);
        assert!(!text.contains("s3cret"));
        catalog.password.clear();
        assert_eq!(parse(&text), [catalog]);
        // Earlier versions kept the password in the list.
        let old = parse("Old\thttps://old.example\tme\tpw\t\tsync\t\n");
        assert_eq!(old[0].password, "pw");
        assert!(old[0].sync);
        assert!(secret_id("https://a").starts_with(SECRETS));
        assert_ne!(secret_id("https://a"), secret_id("https://b"));
    }
}
