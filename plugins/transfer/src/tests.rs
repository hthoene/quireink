use alloc::string::String;
use alloc::vec::Vec;

use quireink_i18n::{Catalog, check};
use quireink_sdk::{MemoryStorage, Storage, read_file};

use crate::web::{self, Done, Request};

#[test]
fn translations_are_complete() {
    let en = Catalog::parse(include_str!("../i18n/en.ini")).unwrap();
    let de = Catalog::parse(include_str!("../i18n/de.ini")).unwrap();
    let issues = check::compare(&en, &de);
    assert!(issues.is_empty(), "{issues:?}");
}

#[test]
fn the_page_takes_its_texts() {
    assert!(crate::PAGE.contains("/*STRINGS*/{}"));
}

fn ask(
    storage: &MemoryStorage,
    method: &str,
    target: &str,
    body_file: Option<&str>,
) -> (u16, Vec<u8>, Option<String>, Option<Done>) {
    let request = Request {
        number: 1,
        method,
        target,
        body_file,
    };
    let (response, done) = web::answer(storage, "abc", "<page>", request);
    (response.status, response.body, response.body_from, done)
}

#[test]
fn the_key_is_needed() {
    let storage = MemoryStorage::new();
    assert_eq!(ask(&storage, "GET", "/", None).0, 403);
    assert_eq!(ask(&storage, "GET", "/?k=abd", None).0, 403);
    let (status, body, ..) = ask(&storage, "GET", "/?k=abc", None);
    assert_eq!((status, body.as_slice()), (200, &b"<page>"[..]));
}

#[test]
fn files_are_listed_and_fetched() {
    let storage = MemoryStorage::new()
        .with("Books/a b.epub", b"book".to_vec())
        .with(".quireink/x", b"own".to_vec());
    let (status, body, ..) = ask(&storage, "GET", "/api/list?k=abc", None);
    assert_eq!(status, 200);
    assert_eq!(
        String::from_utf8(body).unwrap(),
        r#"[{"name":"Books","dir":true,"size":0}]"#
    );
    let (_, body, ..) = ask(&storage, "GET", "/api/list?k=abc&path=Books", None);
    assert_eq!(
        String::from_utf8(body).unwrap(),
        r#"[{"name":"a b.epub","dir":false,"size":4}]"#
    );
    let (status, _, from, done) = ask(
        &storage,
        "GET",
        "/api/file?k=abc&path=Books%2Fa%20b.epub",
        None,
    );
    assert_eq!(status, 200);
    assert_eq!(from.as_deref(), Some("Books/a b.epub"));
    assert_eq!(done, Some(Done::Sent(String::from("a b.epub"))));
    // QuireInk's own data stays out of reach.
    let hidden = "/api/file?k=abc&path=.quireink/x";
    assert_eq!(ask(&storage, "GET", hidden, None).0, 400);
    let around = "/api/list?k=abc&path=Books/../.quireink";
    assert_eq!(ask(&storage, "GET", around, None).0, 400);
    assert_eq!(
        ask(&storage, "GET", "/api/file?k=abc&path=nope", None).0,
        404
    );
}

#[test]
fn files_come_and_go() {
    let storage = MemoryStorage::new().with("Books/a.epub", b"old".to_vec());
    storage.write(".quireink/tmp/request-1", b"new").unwrap();
    let (status, .., done) = ask(
        &storage,
        "PUT",
        "/api/file?k=abc&path=Books/a.epub",
        Some(".quireink/tmp/request-1"),
    );
    assert_eq!(status, 200);
    // What is there is kept.
    assert_eq!(done, Some(Done::Received(String::from("a (2).epub"))));
    assert_eq!(read_file(&storage, "Books/a (2).epub", 16).unwrap(), b"new");
    assert_eq!(read_file(&storage, "Books/a.epub", 16).unwrap(), b"old");
    assert!(!storage.exists(".quireink/tmp/request-1"));
    // Not into a folder that is not there.
    let nowhere = "/api/file?k=abc&path=No/a.epub";
    assert_eq!(ask(&storage, "PUT", nowhere, None).0, 400);
    let (status, .., done) = ask(&storage, "POST", "/api/mkdir?k=abc&path=Books/Neu", None);
    assert_eq!(
        (status, done),
        (200, Some(Done::Folder(String::from("Neu"))))
    );
    let delete = "/api/delete?k=abc&path=Books/a.epub";
    let (status, .., done) = ask(&storage, "POST", delete, None);
    assert_eq!(
        (status, done),
        (200, Some(Done::Deleted(String::from("a.epub"))))
    );
    assert!(!storage.exists("Books/a.epub"));
    // Not everything at once.
    assert_eq!(ask(&storage, "POST", "/api/delete?k=abc", None).0, 404);
}

#[test]
fn queries_are_decoded() {
    let query = "k=1&path=a%2Fb+c%C3%A4";
    assert_eq!(web::param(query, "path").as_deref(), Some("a/b cä"));
    assert_eq!(web::param("k", "k").as_deref(), Some(""));
    assert_eq!(web::param("path=%zz%4", "path").as_deref(), Some("%zz%4"));
    assert_eq!(web::param("k=1", "path"), None);
}

#[test]
fn paths_are_checked() {
    assert_eq!(web::safe("/Books/x.epub/").as_deref(), Some("Books/x.epub"));
    assert_eq!(web::safe("").as_deref(), Some(""));
    for bad in ["a//b", "../x", ".hidden", "a/.b", "C:/x", "a\\b", " a"] {
        assert_eq!(web::safe(bad), None, "{bad}");
    }
}

#[test]
fn json_is_escaped() {
    assert_eq!(web::json_string("a\"b\\c</\n"), r#""a\"b\\c\u003c/\u000a""#);
}
