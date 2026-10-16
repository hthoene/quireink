use super::*;
use quireink_plugin_format_epub::EpubDocument;
use quireink_sdk::MemoryStorage;
use quireink_zip::testing::ZipBuilder;

const BOOK: &str = "books/alice.epub";
const SIDECAR: &str = "books/alice.sdr/metadata.epub.lua";
const EXCHANGE: &str = "books/alice.sdr/alice.epub.annotations.lua";
const NOW: i64 = 1_792_000_000;

const CONTAINER: &str = r#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>"#;

const OPF: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Alice</dc:title></metadata>
  <manifest>
    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
    <item id="c1" href="ch1.xhtml" media-type="application/xhtml+xml"/>
  </manifest>
  <spine><itemref idref="c1"/></spine>
</package>"#;

const NAV: &str = r#"<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
<body><nav epub:type="toc"><ol><li><a href="ch1.xhtml">Down the Rabbit Hole</a></li></ol></nav></body></html>"#;

const CH1: &str = "<html xmlns=\"http://www.w3.org/1999/xhtml\"><body>\
    <p>Alice was beginning to get very tired.</p>\
    <p>So she was considering in her own mind.</p></body></html>";

/// What KOReader keeps: a highlight of "Alice" with a note, a bookmark,
/// and a highlight QuireInk cannot place.
const METADATA: &str = r#"-- we can read Lua syntax here!
return {
    ["annotations"] = {
        [1] = {
            ["datetime"] = "2026-10-01 10:00:00",
            ["drawer"] = "lighten",
            ["note"] = "Her name",
            ["page"] = "/body/DocFragment[1]/body/p[1]/text().0",
            ["pos0"] = "/body/DocFragment[1]/body/p[1]/text().0",
            ["pos1"] = "/body/DocFragment[1]/body/p[1]/text().5",
            ["text"] = "Alice",
        },
        [2] = {
            ["datetime"] = "2026-10-01 10:05:00",
            ["page"] = "/body/DocFragment[1]/body/p[2]/text().0",
        },
        [3] = {
            ["datetime"] = "2026-10-01 10:06:00",
            ["drawer"] = "lighten",
            ["page"] = "/body/DocFragment[7]/body/p[1]/text().0",
            ["pos0"] = "/body/DocFragment[7]/body/p[1]/text().0",
            ["pos1"] = "/body/DocFragment[7]/body/p[1]/text().3",
            ["text"] = "Elsewhere",
        },
    },
    ["percent_finished"] = 0.5,
}
"#;

fn storage() -> MemoryStorage {
    let zip = ZipBuilder::new()
        .stored("mimetype", "application/epub+zip")
        .deflated("META-INF/container.xml", CONTAINER)
        .deflated("OEBPS/content.opf", OPF)
        .deflated("OEBPS/nav.xhtml", NAV)
        .deflated("OEBPS/ch1.xhtml", CH1)
        .build();
    MemoryStorage::new()
        .with(BOOK, zip)
        .with(SIDECAR, METADATA.as_bytes().to_vec())
}

fn run(storage: &MemoryStorage, now: i64) -> bool {
    let doc = EpubDocument::open(storage, BOOK).unwrap();
    let local = |unix| unix;
    let x = Exchange {
        storage,
        doc: &doc,
        now,
        local: &local,
        drawer: "lighten",
    };
    exchange(&x, BOOK)
}

fn items(storage: &MemoryStorage, path: &str) -> Vec<Table> {
    read_items(storage, path)
}

/// A highlight here of the text at `pos0..pos1`.
fn highlight(storage: &MemoryStorage, pos0: &str, pos1: &str, note: &str) -> Annotation {
    let doc = EpubDocument::open(storage, BOOK).unwrap();
    let start = doc.locate_xpointer(storage, pos0).unwrap();
    let end = doc.locate_xpointer(storage, pos1).unwrap();
    Annotation {
        mark: Mark {
            kind: Kind::Highlight,
            start,
            end,
            noted: !note.is_empty(),
        },
        percent: 1,
        created: NOW - 100,
        updated: 0,
        text: String::from("tired"),
        note: String::from(note),
    }
}

#[test]
fn file_names_as_koreader_has_them() {
    assert_eq!(sidecar(BOOK), "books/alice.sdr");
    assert_eq!(
        metadata_file("a/b.Book.EPUB"),
        "a/b.Book.sdr/metadata.EPUB.lua"
    );
    assert_eq!(exchange_file(BOOK), EXCHANGE);
    let local = |unix| unix + 7200;
    let doc = EpubDocument::open(&storage(), BOOK).unwrap();
    let x = Exchange {
        storage: &MemoryStorage::new(),
        doc: &doc,
        now: 0,
        local: &local,
        drawer: "lighten",
    };
    // Local times, two hours ahead here.
    assert_eq!(datetime(&x, 1_760_623_200), "2025-10-16 16:00:00");
    assert_eq!(unix_of(&x, "2025-10-16 16:00:00"), 1_760_623_200);
    assert_eq!(unix_of(&x, "nonsense"), 0);
}

#[test]
fn annotations_go_back_and_forth() {
    let storage = storage();
    let tired = highlight(
        &storage,
        "/body/DocFragment[1]/body/p[1]/text().32",
        "/body/DocFragment[1]/body/p[1]/text().37",
        "",
    );
    annotations::add(&storage, BOOK, tired.clone());

    // KOReader's come over; ours go to the exchange file with all of
    // KOReader's, also the one not placed here.
    assert!(run(&storage, NOW));
    let ours = annotations::load(&storage, BOOK);
    assert_eq!(ours.len(), 3);
    let alice = ours.iter().find(|a| a.note == "Her name").unwrap();
    assert_eq!(alice.text, "Alice");
    assert_eq!(alice.created, unix_of_utc("2026-10-01 10:00:00"));
    let bookmark = ours.iter().find(|a| a.mark.kind == Kind::Bookmark).unwrap();
    assert!(bookmark.text.starts_with("So she was"));
    let passed = items(&storage, EXCHANGE);
    assert_eq!(passed.len(), 4);
    assert_eq!(passed[2].str("text"), Some("Elsewhere"));
    let new = &passed[3];
    assert_eq!(new.str("text"), Some("tired"));
    assert_eq!(new.str("chapter"), Some("Down the Rabbit Hole"));
    assert_eq!(
        new.str("pos0"),
        Some("/body/DocFragment[1]/body/p[1]/text()[1].32")
    );
    assert_eq!(
        new.str("pos1"),
        Some("/body/DocFragment[1]/body/p[1]/text()[1].37")
    );
    let Some(Value::Table(root)) = lua::parse(
        &String::from_utf8(quireink_sdk::read_file(&storage, EXCHANGE, 1 << 20).unwrap()).unwrap(),
    ) else {
        panic!("not Lua");
    };
    assert_eq!(root.str("device_id"), Some("QuireInk"));

    // Until KOReader has taken it over, nothing more to do.
    assert!(!run(&storage, NOW + 10));

    // KOReader took it over, then removed "Alice" and noted on "tired".
    storage.remove(EXCHANGE).unwrap();
    let mut theirs = passed.clone();
    theirs.remove(0);
    theirs[2].set("note", lua::string("Very"));
    theirs[2].set("datetime_updated", lua::string("2026-10-20 08:00:00"));
    let file = lua::table([(
        "annotations",
        Value::Table(Table::from_list(theirs.into_iter().map(Value::Table))),
    )]);
    storage
        .write(SIDECAR, lua::write(&file).as_bytes())
        .unwrap();
    assert!(run(&storage, NOW + 20));
    let ours = annotations::load(&storage, BOOK);
    assert_eq!(ours.len(), 2);
    assert!(ours.iter().all(|a| a.note != "Her name"));
    assert!(ours.iter().any(|a| a.note == "Very" && a.mark.noted));
    assert!(!storage.exists(EXCHANGE), "nothing new for KOReader");

    // Removed here: KOReader is told by leaving it out.
    assert!(annotations::remove(&storage, BOOK, |m| m.kind == Kind::Bookmark));
    assert!(!run(&storage, NOW + 30));
    let passed = items(&storage, EXCHANGE);
    assert_eq!(passed.len(), 2);
    assert!(passed.iter().all(|i| i.get("drawer").is_some()));
}

#[test]
fn later_notes_win() {
    let storage = storage();
    // The same highlight as KOReader's, with a note made later here.
    let mut alice = highlight(
        &storage,
        "/body/DocFragment[1]/body/p[1]/text().0",
        "/body/DocFragment[1]/body/p[1]/text().5",
        "Later note",
    );
    alice.updated = unix_of_utc("2026-10-05 12:00:00");
    annotations::add(&storage, BOOK, alice);
    run(&storage, NOW);
    let ours = annotations::load(&storage, BOOK);
    assert!(ours.iter().any(|a| a.note == "Later note"));
    let passed = items(&storage, EXCHANGE);
    assert_eq!(passed[0].str("note"), Some("Later note"));
    assert_eq!(
        passed[0].str("datetime_updated"),
        Some("2026-10-05 12:00:00")
    );
}

fn unix_of_utc(text: &str) -> i64 {
    let doc = EpubDocument::open(&storage(), BOOK).unwrap();
    let local = |unix| unix;
    let x = Exchange {
        storage: &MemoryStorage::new(),
        doc: &doc,
        now: 0,
        local: &local,
        drawer: "lighten",
    };
    unix_of(&x, text)
}

#[test]
fn translations_are_complete() {
    use quireink_i18n::{Catalog, check};
    let en = Catalog::parse(include_str!("../i18n/en.ini")).unwrap();
    let de = Catalog::parse(include_str!("../i18n/de.ini")).unwrap();
    let issues = check::compare(&en, &de);
    assert!(issues.is_empty(), "{issues:?}");
}
