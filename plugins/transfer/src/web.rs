//! The requests of the web page: the page itself, and listing,
//! downloading, uploading, making folders and deleting.
//!
//! Every request carries the key shown on the device (`k=`); without it
//! the answer is *403*. Paths are relative to the top of the storage;
//! hidden ones (a part starting with `.`, e.g. QuireInk's own data) are
//! out of reach.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::files;
use quireink_sdk::{ServerResponse, Storage, StorageError};

/// What a request changed, to tell on the device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Done {
    Received(String),
    Sent(String),
    Folder(String),
    Deleted(String),
}

/// A request to answer.
#[derive(Debug, Clone, Copy)]
pub struct Request<'a> {
    pub number: u32,
    pub method: &'a str,
    pub target: &'a str,
    /// Where its body was saved, if it had one.
    pub body_file: Option<&'a str>,
}

/// Answers `request`; `page` is the web page to serve.
pub fn answer(
    storage: &dyn Storage,
    key: &str,
    page: &str,
    request: Request<'_>,
) -> (ServerResponse, Option<Done>) {
    let (path, query) = request
        .target
        .split_once('?')
        .unwrap_or((request.target, ""));
    let number = request.number;
    let text = |status, body: &str| {
        (
            ServerResponse::new(number, status, "text/plain; charset=utf-8", body.into()),
            None,
        )
    };
    if path == "/favicon.ico" {
        return text(404, "not found");
    }
    if param(query, "k").as_deref() != Some(key) {
        return text(
            403,
            "The key is missing or wrong: open the address shown on the device.",
        );
    }
    let Some(target) = param(query, "path").map_or(Some(String::new()), |p| safe(&p)) else {
        return text(400, "bad path");
    };
    let failed = |e: StorageError| {
        let status = match e {
            StorageError::NotFound => 404,
            StorageError::InvalidPath => 400,
            _ => 500,
        };
        text(status, &format!("{e:?}"))
    };
    let ok = |done: Option<Done>| {
        (
            ServerResponse::new(number, 200, "application/json", b"{\"ok\":true}".to_vec()),
            done,
        )
    };
    match (request.method, path) {
        ("GET", "/") => (
            ServerResponse::new(
                number,
                200,
                "text/html; charset=utf-8",
                page.as_bytes().to_vec(),
            ),
            None,
        ),
        ("GET", "/api/list") => match files::list(storage, &target, false) {
            Ok(entries) => {
                let mut json = String::from("[");
                for (i, entry) in entries.iter().enumerate() {
                    if i > 0 {
                        json.push(',');
                    }
                    json.push_str(&format!(
                        "{{\"name\":{},\"dir\":{},\"size\":{}}}",
                        json_string(&entry.name),
                        entry.is_dir,
                        entry.size
                    ));
                }
                json.push(']');
                (
                    ServerResponse::new(number, 200, "application/json", json.into_bytes()),
                    None,
                )
            }
            Err(e) => failed(e),
        },
        ("GET", "/api/file") => {
            if target.is_empty() || files::is_dir(storage, &target) || !storage.exists(&target) {
                return text(404, "not found");
            }
            let mut response =
                ServerResponse::new(number, 200, "application/octet-stream", Vec::new());
            response.body_from = Some(target.clone());
            (response, Some(Done::Sent(files::name(&target).to_string())))
        }
        ("PUT", "/api/file") => {
            let (dir, name) = (files::parent(&target), files::name(&target));
            if name.is_empty() || !(dir.is_empty() || files::is_dir(storage, dir)) {
                return text(400, "bad path");
            }
            let at = files::free_path(storage, dir, name);
            let result = match request.body_file {
                Some(body) => files::move_to(storage, body, &at),
                None => storage.write(&at, &[]),
            };
            match result {
                Ok(()) => ok(Some(Done::Received(files::name(&at).to_string()))),
                Err(e) => failed(e),
            }
        }
        ("POST", "/api/mkdir") if !target.is_empty() => {
            let at = files::free_path(storage, files::parent(&target), files::name(&target));
            match storage.create_dir(&at) {
                Ok(()) => ok(Some(Done::Folder(files::name(&at).to_string()))),
                Err(e) => failed(e),
            }
        }
        ("POST", "/api/delete") if !target.is_empty() => match files::delete(storage, &target) {
            Ok(()) => ok(Some(Done::Deleted(files::name(&target).to_string()))),
            Err(e) => failed(e),
        },
        _ => text(404, "not found"),
    }
}

/// A path a request may touch, without leading or trailing `/`: no
/// hidden parts, no `..`, nothing a file system could take otherwise.
pub fn safe(path: &str) -> Option<String> {
    let path = path.trim_matches('/');
    if path.is_empty() {
        return Some(String::new());
    }
    let fine = path.split('/').all(|part| {
        !part.is_empty()
            && !part.starts_with('.')
            && part.trim() == part
            && !part.contains(['\\', ':', '\0'])
            && !part.chars().any(char::is_control)
    });
    fine.then(|| path.to_string())
}

/// Parameter `name` of a query (`a=1&b=x%20y`), decoded.
pub fn param(query: &str, name: &str) -> Option<String> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('=').or(Some((pair, ""))))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| decode(value))
}

/// Decodes `%XX` and `+` of a query value.
fn decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let hex = |b: u8| char::from(b).to_digit(16).map(|d| d as u8);
    let mut i = 0;
    while i < bytes.len() {
        let escaped = (bytes[i] == b'%')
            .then(|| Some((hex(*bytes.get(i + 1)?)?, hex(*bytes.get(i + 2)?)?)))
            .flatten();
        match (bytes[i], escaped) {
            (_, Some((high, low))) => {
                out.push(high << 4 | low);
                i += 3;
                continue;
            }
            (b'+', _) => out.push(b' '),
            (byte, _) => out.push(byte),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `text` as a JSON string.
pub fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '<' => out.push_str("\\u003c"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
