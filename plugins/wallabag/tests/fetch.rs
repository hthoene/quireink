//! A fetch against answers as Wallabag gives them: the token, the list of
//! unread articles, their books; then archiving one read to the end.

// Tests panic on purpose when something is wrong.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::rc::Rc;

use quireink_core::{HttpCall, HttpResponse, Kernel};
use quireink_platform::{ButtonState, InputEvent, Rotation};
use quireink_plugin_wallabag::Wallabag;
use quireink_sdk::secrets::{FILE, SecretChange, Secrets};
use quireink_sdk::{
    DeviceInfo, DirEntry, DisplayInfo, InputCaps, Instant, MemoryClass, MemoryStorage, PixelFormat,
    Storage, StorageError,
};

const DEVICE: DeviceInfo = DeviceInfo {
    id: "test",
    name: "Test device",
    display: DisplayInfo {
        width: 200,
        height: 300,
        dpi: 100,
        format: PixelFormat::Gray4,
        rotation: Rotation::Deg0,
        color: None,
    },
    input: InputCaps {
        buttons: true,
        touch: false,
        pen: false,
        keys: &[],
    },
    memory: MemoryClass::Large,
};

/// A storage the test looks into while the kernel has it.
#[derive(Clone)]
struct Shared(Rc<MemoryStorage>);

impl Storage for Shared {
    fn read_at(&self, path: &str, offset: u64, buf: &mut [u8]) -> Result<usize, StorageError> {
        self.0.read_at(path, offset, buf)
    }
    fn size(&self, path: &str) -> Result<u64, StorageError> {
        self.0.size(path)
    }
    fn write(&self, path: &str, data: &[u8]) -> Result<(), StorageError> {
        self.0.write(path, data)
    }
    fn append(&self, path: &str, data: &[u8]) -> Result<(), StorageError> {
        self.0.append(path, data)
    }
    fn list(&self, dir: &str, visit: &mut dyn FnMut(DirEntry<'_>)) -> Result<(), StorageError> {
        self.0.list(dir, visit)
    }
    fn remove(&self, path: &str) -> Result<(), StorageError> {
        self.0.remove(path)
    }
    fn exists(&self, path: &str) -> bool {
        self.0.exists(path)
    }
    fn rename(&self, from: &str, to: &str) -> Result<(), StorageError> {
        self.0.rename(from, to)
    }
    fn create_dir(&self, path: &str) -> Result<(), StorageError> {
        self.0.create_dir(path)
    }
}

const CONFIG: &str = "[wallabag]\nserver = wb.example\nuser = me\nclient-id = cid\n\n\
    [keymap.global]\nbutton.fetch = wallabag.fetch\n";

const LIST: &str = r#"{"_embedded": {"items": [
    {"id": 9, "title": "Newer"},
    {"id": 7, "title": "Older"}
]}}"#;

fn fetch(kernel: &mut Kernel, at: u64) -> Vec<HttpCall> {
    for state in [ButtonState::Pressed, ButtonState::Released] {
        kernel.handle(
            Instant(at),
            InputEvent::Button {
                name: "fetch",
                state,
            },
        );
    }
    kernel.take_requests()
}

fn answer(kernel: &mut Kernel, call: &HttpCall, status: u16, body: &str) -> Vec<HttpCall> {
    kernel.http_response(
        Instant(10),
        HttpResponse {
            id: call.id,
            status,
            body: body.as_bytes().to_vec(),
            ..HttpResponse::default()
        },
    );
    kernel.take_requests()
}

#[test]
fn fetches_new_articles_and_archives_read_ones() {
    let mut secrets = Secrets::default();
    secrets.apply(SecretChange::keep("wallabag.password", "", "pw"));
    secrets.apply(SecretChange::keep("wallabag.client-secret", "", "cs"));
    let storage = Shared(Rc::new(
        MemoryStorage::new().with(FILE, secrets.encode().into_bytes()),
    ));
    let mut kernel = Kernel::builder(DEVICE)
        .storage(storage.clone())
        .user_config(CONFIG)
        .network(true)
        .plugin(Wallabag)
        .build()
        .unwrap();
    kernel.start(Instant(0));

    // A token for the account.
    let calls = fetch(&mut kernel, 1);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].method, "POST");
    assert_eq!(calls[0].url, "https://wb.example/oauth/v2/token");
    let body = String::from_utf8(calls[0].body.clone().unwrap()).unwrap();
    assert_eq!(
        body,
        "grant_type=password&client_id=cid&client_secret=cs&username=me&password=pw"
    );
    // The list, then the books, the oldest first.
    let calls = answer(
        &mut kernel,
        &calls[0],
        200,
        r#"{"access_token": "T", "x": 1}"#,
    );
    assert!(calls[0].url.contains("/api/entries.json?archive=0"));
    assert!(
        calls[0]
            .headers
            .contains(&(String::from("Authorization"), String::from("Bearer T")))
    );
    let calls = answer(&mut kernel, &calls[0], 200, LIST);
    assert_eq!(calls[0].url, "https://wb.example/api/entries/7/export.epub");
    let calls = answer(&mut kernel, &calls[0], 200, "EPUB 7");
    assert_eq!(calls[0].url, "https://wb.example/api/entries/9/export.epub");
    let calls = answer(&mut kernel, &calls[0], 200, "EPUB 9");
    assert!(calls.is_empty());
    assert_eq!(storage.0.get("Wallabag/Older.epub").unwrap(), b"EPUB 7");
    assert_eq!(storage.0.get("Wallabag/Newer.epub").unwrap(), b"EPUB 9");

    // Article 7 read to the end: archived with the next fetch, its book
    // removed; nothing new to download.
    storage
        .0
        .write(".quireink/plugins/wallabag/finished.txt", b"7\n")
        .unwrap();
    let calls = fetch(&mut kernel, 20);
    let calls = answer(&mut kernel, &calls[0], 200, r#"{"access_token": "U"}"#);
    assert_eq!(calls[0].method, "PATCH");
    assert_eq!(calls[0].url, "https://wb.example/api/entries/7.json");
    let calls = answer(&mut kernel, &calls[0], 200, "{}");
    assert!(storage.0.get("Wallabag/Older.epub").is_none());
    let calls = answer(
        &mut kernel,
        &calls[0],
        200,
        r#"{"_embedded": {"items": [{"id": 9, "title": "Newer"}]}}"#,
    );
    assert!(calls.is_empty());
    assert!(storage.0.get("Wallabag/Newer.epub").is_some());

    // Archived elsewhere: its book goes too.
    let calls = fetch(&mut kernel, 30);
    let calls = answer(&mut kernel, &calls[0], 200, r#"{"access_token": "V"}"#);
    let calls = answer(
        &mut kernel,
        &calls[0],
        200,
        r#"{"_embedded": {"items": []}}"#,
    );
    assert!(calls.is_empty());
    assert!(storage.0.get("Wallabag/Newer.epub").is_none());
}

#[test]
fn a_wrong_login_says_so() {
    let mut secrets = Secrets::default();
    secrets.apply(SecretChange::keep("wallabag.password", "", "pw"));
    secrets.apply(SecretChange::keep("wallabag.client-secret", "", "cs"));
    let storage = MemoryStorage::new().with(FILE, secrets.encode().into_bytes());
    let mut kernel = Kernel::builder(DEVICE)
        .storage(storage)
        .user_config(CONFIG)
        .network(true)
        .plugin(Wallabag)
        .build()
        .unwrap();
    kernel.start(Instant(0));
    let calls = fetch(&mut kernel, 1);
    let calls = answer(&mut kernel, &calls[0], 400, r#"{"error": "invalid_grant"}"#);
    assert!(calls.is_empty());
    // And may start again.
    assert_eq!(fetch(&mut kernel, 2).len(), 1);
}
