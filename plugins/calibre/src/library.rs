//! The books Calibre sent, so it knows them when it connects again: each
//! one's metadata as Calibre gave it, in a file of its own in the
//! plugin's folder, and what Calibre said about itself
//! (`SET_CALIBRE_DEVICE_INFO`).

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::md5::Md5;
use quireink_sdk::{Env, Storage};

use crate::PLUGIN;
use crate::protocol;

/// Metadata kept at most per book (Calibre's descriptions can be long).
const MAX_METADATA: usize = 256 * 1024;

fn folder() -> String {
    format!("{}/books", Env::plugin_dir(PLUGIN.as_str()))
}

fn device_file() -> String {
    format!("{}/device.json", Env::plugin_dir(PLUGIN.as_str()))
}

/// The file of the book at `lpath`.
fn record(lpath: &str) -> String {
    let mut md5 = Md5::new();
    md5.update(lpath.as_bytes());
    format!("{}/{}.json", folder(), md5.hex())
}

/// Keeps the metadata of the book at `lpath`.
pub fn remember(storage: &dyn Storage, lpath: &str, metadata: &str) {
    if metadata.len() > MAX_METADATA {
        return;
    }
    let _ = storage.create_dir(&folder());
    let _ = storage.write(&record(lpath), metadata.as_bytes());
}

/// Forgets the book at `lpath`; its `uuid`, if Calibre gave one.
pub fn forget(storage: &dyn Storage, lpath: &str) -> Option<String> {
    let path = record(lpath);
    let uuid = read(storage, &path).and_then(|m| protocol::text(&m, "uuid"));
    let _ = storage.remove(&path);
    uuid
}

fn read(storage: &dyn Storage, path: &str) -> Option<String> {
    let data = quireink_sdk::read_file(storage, path, MAX_METADATA).ok()?;
    String::from_utf8(data).ok()
}

/// The metadata of the books Calibre sent that are still in `folder`
/// (the library), each as Calibre gave it.
pub fn books(storage: &dyn Storage, library: &str) -> Vec<String> {
    let mut names = Vec::new();
    let _ = storage.list(&folder(), &mut |entry| {
        if !entry.is_dir && entry.name.ends_with(".json") {
            names.push(entry.name.to_string());
        }
    });
    names.sort();
    names
        .into_iter()
        .filter_map(|name| read(storage, &format!("{}/{name}", folder())))
        .filter(|metadata| {
            protocol::text(metadata, "lpath")
                .and_then(|lpath| protocol::safe_path(&lpath))
                .is_some_and(|lpath| storage.exists(&quireink_sdk::files::join(library, &lpath)))
        })
        .collect()
}

/// What Calibre said about itself last.
pub fn device_info(storage: &dyn Storage) -> Option<String> {
    read(storage, &device_file())
}

pub fn set_device_info(storage: &dyn Storage, info: &str) {
    let _ = storage.create_dir(Env::plugin_dir(PLUGIN.as_str()).as_str());
    let _ = storage.write(&device_file(), info.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use quireink_sdk::MemoryStorage;

    #[test]
    fn books_kept_while_their_files_are_there() {
        let storage = MemoryStorage::new().with("books/Ann/A.epub", b"x".to_vec());
        let a = r#"{"lpath": "Ann/A.epub", "uuid": "u-a", "title": "A"}"#;
        let b = r#"{"lpath": "Bo/B.epub", "uuid": "u-b", "title": "B"}"#;
        remember(&storage, "Ann/A.epub", a);
        remember(&storage, "Bo/B.epub", b);
        // B's file is not there (deleted on the device).
        assert_eq!(books(&storage, "books"), [a]);
        assert_eq!(forget(&storage, "Ann/A.epub").as_deref(), Some("u-a"));
        assert!(books(&storage, "books").is_empty());
        assert_eq!(forget(&storage, "Ann/A.epub"), None);
    }

    #[test]
    fn device_info_is_kept() {
        let storage = MemoryStorage::new();
        assert_eq!(device_info(&storage), None);
        set_device_info(&storage, r#"{"device_store_uuid": "x"}"#);
        assert_eq!(
            device_info(&storage).as_deref(),
            Some(r#"{"device_store_uuid": "x"}"#)
        );
    }
}
