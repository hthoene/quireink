//! Passwords and keys, kept in one place for every plugin: plugins read
//! them with [`Env::secrets`](crate::Env) and keep or remove them with
//! [`Command::SetSecret`](crate::Command::SetSecret); the settings list
//! them all under *Passwords and keys* to change or remove.
//!
//! They are kept in a file of their own (`.quireink/secrets.txt`), out of
//! `quireink.ini` and not readable at a glance. That is no encryption:
//! the devices have no safe place for a key, so whoever has the files
//! can read them.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Where the secrets are kept.
pub const FILE: &str = ".quireink/secrets.txt";

/// Mixed into the stored values, so they are not read at a glance.
const VEIL: &[u8] = b"QuireInk keeps this out of sight, not safe";

/// A password or key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Secret {
    /// Who keeps it and for what, e.g. `translate.deepl-key`.
    pub id: String,
    /// What it is, shown in the list: a translation key or the text.
    pub label: String,
    pub value: String,
    /// It can be typed anew in the list (not, e.g., a hash of one).
    pub changeable: bool,
}

/// A change: `value` `None` removes the secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretChange {
    pub id: String,
    pub label: String,
    pub value: Option<String>,
    pub changeable: bool,
}

impl SecretChange {
    /// Keeps `value` as secret `id`, shown as `label`.
    pub fn keep(id: &str, label: &str, value: &str) -> Self {
        Self {
            id: id.to_string(),
            label: label.to_string(),
            value: Some(value.to_string()),
            changeable: true,
        }
    }

    /// Removes secret `id`.
    pub fn remove(id: &str) -> Self {
        Self {
            id: id.to_string(),
            label: String::new(),
            value: None,
            changeable: false,
        }
    }
}

/// All secrets.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Secrets {
    list: Vec<Secret>,
}

fn veil(data: &[u8]) -> Vec<u8> {
    data.iter()
        .enumerate()
        .map(|(i, b)| b ^ VEIL[i % VEIL.len()])
        .collect()
}

fn hex(data: &[u8]) -> String {
    data.iter().map(|b| alloc::format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

impl Secrets {
    /// The value of secret `id`, if kept.
    pub fn get(&self, id: &str) -> Option<&str> {
        self.list
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.value.as_str())
    }

    pub fn iter(&self) -> impl Iterator<Item = &Secret> {
        self.list.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// Applies `change`.
    pub fn apply(&mut self, change: SecretChange) {
        let at = self.list.iter().position(|s| s.id == change.id);
        match (change.value, at) {
            (Some(value), Some(at)) => {
                let secret = &mut self.list[at];
                secret.value = value;
                if !change.label.is_empty() {
                    secret.label = change.label;
                }
                secret.changeable = change.changeable;
            }
            (Some(value), None) => self.list.push(Secret {
                id: change.id,
                label: change.label,
                value,
                changeable: change.changeable,
            }),
            (None, Some(at)) => {
                self.list.remove(at);
            }
            (None, None) => {}
        }
    }

    /// The file's text: `id<TAB>label<TAB>flags<TAB>value` lines, the
    /// value veiled.
    pub fn encode(&self) -> String {
        let clean = |s: &str| s.replace(['\t', '\n', '\r'], " ");
        self.list
            .iter()
            .map(|s| {
                alloc::format!(
                    "{}\t{}\t{}\t{}\n",
                    clean(&s.id),
                    clean(&s.label),
                    if s.changeable { "c" } else { "" },
                    hex(&veil(s.value.as_bytes()))
                )
            })
            .collect()
    }

    pub fn decode(text: &str) -> Self {
        let list = text
            .lines()
            .filter_map(|line| {
                let mut f = line.splitn(4, '\t');
                let id = f.next()?.to_string();
                let label = f.next()?.to_string();
                let changeable = f.next()? == "c";
                let value = String::from_utf8(veil(&unhex(f.next()?)?)).ok()?;
                (!id.is_empty()).then_some(Secret {
                    id,
                    label,
                    value,
                    changeable,
                })
            })
            .collect();
        Self { list }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kept_changed_removed_and_veiled() {
        let mut secrets = Secrets::default();
        secrets.apply(SecretChange::keep("a.key", "a.label", "s3cr\u{e9}t:fx"));
        secrets.apply(SecretChange::keep("b.key", "Catalog: B", "pw"));
        let text = secrets.encode();
        assert!(!text.contains("s3cr"));
        let read = Secrets::decode(&text);
        assert_eq!(read.get("a.key"), Some("s3cr\u{e9}t:fx"));
        assert_eq!(read, secrets);
        secrets.apply(SecretChange::keep("a.key", "", "new"));
        assert_eq!(secrets.get("a.key"), Some("new"));
        assert_eq!(
            secrets.iter().next().map(|s| s.label.as_str()),
            Some("a.label")
        );
        secrets.apply(SecretChange::remove("a.key"));
        assert_eq!(secrets.get("a.key"), None);
        assert_eq!(secrets.iter().count(), 1);
    }
}
