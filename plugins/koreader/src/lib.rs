//! KOReader: the bookmarks and highlights of a book go back and forth
//! with KOReader, for books read in both on one device (or one library
//! folder synced between devices).
//!
//! KOReader keeps what it knows of a book beside it, in `Book.sdr`. When
//! QuireInk opens a book that has such a folder, and when its bookmarks or
//! highlights change, this plugin reads KOReader's (`metadata.epub.lua`),
//! takes over what is new there, and leaves what is new here in
//! `Book.epub.annotations.lua`: KOReader takes that over the next time it
//! opens the book, and removes the file. Positions are KOReader's
//! XPointers ([`Document::xpointer`](quireink_sdk::document::Document)),
//! so EPUBs only.
//!
//! What is removed on one side goes on the other too: the plugin keeps
//! which annotations both had at the last exchange. A note changed on
//! both sides keeps the later change.

#![no_std]

extern crate alloc;

pub mod lua;

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use quireink_sdk::annotations::{self, Annotation, Kind, Mark};
use quireink_sdk::builtin::{events, params};
use quireink_sdk::document::{Document, Locator};
use quireink_sdk::time::{self, LocalTime};
use quireink_sdk::{
    Command, Env, Id, Manifest, Params, Plugin, RegisterError, Registrar, SDK_VERSION, Storage,
    Version,
};

use lua::{Table, Value};

pub const PLUGIN: Id = Id::new_static("koreader");
/// Who wrote the annotations file, for KOReader (which takes over only
/// those of other devices).
const DEVICE: &str = "QuireInk";
/// Longest KOReader file read.
const MAX_FILE: usize = 4 << 20;
/// Most text taken from a book for a highlight.
const MAX_TEXT: usize = 4096;
/// The reader's setting for how highlights look.
const HIGHLIGHT_STYLE: &str = "reader.highlight-style";
const UNDERLINE: &str = "reader.highlight-style.underline";

/// KOReader's folder for the book at `book`: `Book.sdr` beside it.
pub fn sidecar(book: &str) -> String {
    let stem = book.rsplit_once('.').map_or(book, |(stem, _)| stem);
    format!("{stem}.sdr")
}

/// KOReader's own file in it: `metadata.<suffix>.lua`.
fn metadata_file(book: &str) -> String {
    let suffix = book.rsplit_once('.').map_or("_", |(_, suffix)| suffix);
    format!("{}/metadata.{suffix}.lua", sidecar(book))
}

/// Annotations for KOReader to take over: `<file name>.annotations.lua`.
fn exchange_file(book: &str) -> String {
    format!(
        "{}/{}.annotations.lua",
        sidecar(book),
        quireink_sdk::files::name(book)
    )
}

/// Where the annotations both sides had at the last exchange are kept.
fn known_file(book: &str) -> String {
    let hash = book.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    });
    format!("{}/{hash:016x}.txt", Env::plugin_dir(PLUGIN.as_str()))
}

/// What an annotation is known by: its kind and place.
type Key = (Kind, Locator, Locator);

fn key_of(mark: &Mark) -> Key {
    match mark.kind {
        Kind::Bookmark => (Kind::Bookmark, mark.start, mark.start),
        Kind::Highlight => (Kind::Highlight, mark.start, mark.end),
    }
}

fn load_known(storage: &dyn Storage, book: &str) -> Vec<Key> {
    let Ok(data) = quireink_sdk::read_file(storage, &known_file(book), 1 << 20) else {
        return Vec::new();
    };
    String::from_utf8_lossy(&data)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split(' ');
            let kind = match fields.next()? {
                "b" => Kind::Bookmark,
                "h" => Kind::Highlight,
                _ => return None,
            };
            let mut number = || fields.next()?.parse::<u32>().ok();
            let start = Locator::new(number()?, number()?);
            let end = Locator::new(number()?, number()?);
            Some((kind, start, end))
        })
        .collect()
}

fn save_known(storage: &dyn Storage, book: &str, known: &[Key]) {
    let mut out = String::new();
    for (kind, start, end) in known {
        let kind = if *kind == Kind::Bookmark { 'b' } else { 'h' };
        out.push_str(&format!(
            "{kind} {} {} {} {}\n",
            start.section, start.offset, end.section, end.offset
        ));
    }
    let _ = storage.write(&known_file(book), out.as_bytes());
}

/// Whether a folder has anything in it.
fn has_entries(storage: &dyn Storage, dir: &str) -> bool {
    let mut any = false;
    let _ = storage.list(dir, &mut |_| any = true);
    any
}

/// The annotations of KOReader's file at `path` (empty if there is none).
fn read_items(storage: &dyn Storage, path: &str) -> Vec<Table> {
    let Ok(data) = quireink_sdk::read_file(storage, path, MAX_FILE) else {
        return Vec::new();
    };
    let Some(Value::Table(root)) = lua::parse(&String::from_utf8_lossy(&data)) else {
        return Vec::new();
    };
    let Some(Value::Table(items)) = root.get("annotations") else {
        return Vec::new();
    };
    items
        .list()
        .filter_map(|item| match item {
            Value::Table(t) => Some(t.clone()),
            _ => None,
        })
        .collect()
}

/// What an exchange works with.
pub struct Exchange<'a> {
    pub storage: &'a dyn Storage,
    /// The book, opened.
    pub doc: &'a dyn Document,
    /// Seconds since 1970 (UTC) now, 0 if not known.
    pub now: i64,
    /// Local seconds since 1970 of seconds since 1970 (UTC).
    pub local: &'a dyn Fn(i64) -> i64,
    /// How new highlights look in KOReader: `lighten` or `underscore`.
    pub drawer: &'a str,
}

impl core::fmt::Debug for Exchange<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Exchange")
            .field("now", &self.now)
            .field("drawer", &self.drawer)
            .finish_non_exhaustive()
    }
}

/// KOReader's times: local, `2026-10-16 14:05:00`.
fn datetime(x: &Exchange<'_>, unix: i64) -> String {
    let t = LocalTime::from_seconds((x.local)(unix));
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        t.year, t.month, t.day, t.hour, t.minute, t.second
    )
}

/// Seconds since 1970 (UTC) of one of KOReader's times; 0 if none.
fn unix_of(x: &Exchange<'_>, text: &str) -> i64 {
    let number = |range: core::ops::Range<usize>| text.get(range)?.parse::<i64>().ok();
    let (Some(year), Some(month), Some(day)) = (number(0..4), number(5..7), number(8..10)) else {
        return 0;
    };
    let (hour, minute, second) = (
        number(11..13).unwrap_or(0),
        number(14..16).unwrap_or(0),
        number(17..19).unwrap_or(0),
    );
    // Days since 1970 of the date (proleptic Gregorian).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let local = days * 86_400 + hour * 3600 + minute * 60 + second;
    // The zone's offset then, near enough.
    local - ((x.local)(local) - local)
}

/// The text of a book from `start` to `end`, paragraphs on lines of
/// their own.
fn text_between(doc: &dyn Document, storage: &dyn Storage, start: Locator, end: Locator) -> String {
    let mut out = String::new();
    let mut at = start;
    for _ in 0..64 {
        if at >= end || out.len() >= MAX_TEXT {
            break;
        }
        let Ok(Some(block)) = doc.block_at(storage, at) else {
            if at.section + 1 >= doc.sections() {
                break;
            }
            at = Locator::new(at.section + 1, 0);
            continue;
        };
        let from = block.text_index(at.offset.max(block.start));
        let to = if at.section == end.section && end.offset < block.end {
            block.text_index(end.offset)
        } else {
            block.text.len()
        };
        if let Some(text) = block.text.get(from..to.max(from)) {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(text);
        }
        at = Locator::new(at.section, block.end.max(at.offset + 1));
    }
    out
}

/// The title of the chapter `at` is in.
fn chapter(doc: &dyn Document, storage: &dyn Storage, at: Locator) -> Option<String> {
    let mut title = None;
    for i in 0..doc.toc_len(storage) {
        let entry = doc.toc_entry(storage, i)?;
        if entry.at > at {
            break;
        }
        title = Some(entry.title);
    }
    title
}

/// Where KOReader's annotation `item` is here; `None` where it cannot be
/// placed (it then stays as it is).
fn locate(x: &Exchange<'_>, item: &Table) -> Option<Key> {
    let (doc, storage) = (x.doc, x.storage);
    if item.get("drawer").is_some() {
        let start = doc.locate_xpointer(storage, item.str("pos0")?)?;
        let end = doc.locate_xpointer(storage, item.str("pos1")?)?;
        (start < end).then_some((Kind::Highlight, start, end))
    } else {
        let at = doc.locate_xpointer(storage, item.str("page")?)?;
        Some((Kind::Bookmark, at, at))
    }
}

/// KOReader's annotation `item` as one here, at `key`.
fn annotation_of(x: &Exchange<'_>, item: &Table, (kind, start, end): Key) -> Annotation {
    let (doc, storage) = (x.doc, x.storage);
    let note = annotations::one_line(item.str("note").unwrap_or_default());
    let text = match kind {
        Kind::Highlight => item
            .str("text")
            .map(String::from)
            .unwrap_or_else(|| text_between(doc, storage, start, end)),
        // Bookmarks show the beginning of their page.
        Kind::Bookmark => {
            let mut text = text_between(doc, storage, start, Locator::new(start.section, u32::MAX));
            text.truncate(text.char_indices().nth(120).map_or(text.len(), |(i, _)| i));
            text
        }
    };
    Annotation {
        mark: Mark {
            kind,
            start,
            end,
            noted: !note.is_empty(),
        },
        percent: (doc.progress(start) * 100.0 + 0.5) as u8,
        created: unix_of(x, item.str("datetime").unwrap_or_default()),
        updated: unix_of(x, item.str("datetime_updated").unwrap_or_default()),
        text: annotations::snippet(&text),
        note,
    }
}

/// Annotation `a` as KOReader keeps it.
fn item_of(x: &Exchange<'_>, a: &Annotation) -> Option<Value> {
    let (doc, storage) = (x.doc, x.storage);
    let page = doc.xpointer(storage, a.mark.start)?;
    let created = if a.created > 0 { a.created } else { x.now };
    let optional = |v: Option<String>| v.map_or(Value::Nil, |v| lua::string(&v));
    let note = optional((!a.note.is_empty()).then(|| a.note.clone()));
    let updated = optional((a.updated > 0).then(|| datetime(x, a.updated)));
    let chapter = optional(chapter(doc, storage, a.mark.start));
    Some(match a.mark.kind {
        Kind::Bookmark => lua::table([
            ("chapter", chapter),
            ("datetime", lua::string(&datetime(x, created))),
            ("datetime_updated", updated),
            ("note", note),
            ("page", lua::string(&page)),
        ]),
        Kind::Highlight => {
            let end = doc.xpointer(storage, a.mark.end)?;
            let text = text_between(doc, storage, a.mark.start, a.mark.end);
            let text = if text.is_empty() {
                a.text.clone()
            } else {
                text
            };
            lua::table([
                ("chapter", chapter),
                ("datetime", lua::string(&datetime(x, created))),
                ("datetime_updated", updated),
                ("drawer", lua::string(x.drawer)),
                ("note", note),
                ("page", lua::string(&page)),
                ("pos0", lua::string(&page)),
                ("pos1", lua::string(&end)),
                ("text", lua::string(&text)),
            ])
        }
    })
}

/// Exchanges the annotations of the book at `book` with KOReader's;
/// `true` if those here changed.
pub fn exchange(x: &Exchange<'_>, book: &str) -> bool {
    let (doc, storage) = (x.doc, x.storage);
    // Only where KOReader's positions are understood.
    if doc.xpointer(storage, Locator::new(0, 0)).is_none() {
        return false;
    }
    // While KOReader has not taken over the last exchange, that is what
    // it will have; else what it has.
    let pending = storage.exists(&exchange_file(book));
    let theirs = read_items(
        storage,
        &if pending {
            exchange_file(book)
        } else {
            metadata_file(book)
        },
    );
    let known = load_known(storage, book);
    let mut ours = annotations::load(storage, book);
    let mut matched = alloc::vec![false; ours.len()];
    let (mut ours_changed, mut theirs_changed) = (false, false);
    let mut items: Vec<Value> = Vec::new();
    for mut item in theirs {
        let Some(key) = locate(x, &item) else {
            items.push(Value::Table(item));
            continue;
        };
        if let Some(i) = ours.iter().position(|a| key_of(&a.mark) == key) {
            matched[i] = true;
            let a = &mut ours[i];
            let note = annotations::one_line(item.str("note").unwrap_or_default());
            if note != a.note {
                let changed = item
                    .str("datetime_updated")
                    .or(item.str("datetime"))
                    .unwrap_or_default();
                let their_time = unix_of(x, changed);
                if their_time > a.updated.max(a.created) {
                    a.mark.noted = !note.is_empty();
                    a.note = note;
                    a.updated = their_time;
                    ours_changed = true;
                } else {
                    let ours_note = (!a.note.is_empty()).then(|| lua::string(&a.note));
                    item.set("note", ours_note.unwrap_or(Value::Nil));
                    let time = if a.updated > 0 { a.updated } else { x.now };
                    item.set("datetime_updated", lua::string(&datetime(x, time)));
                    theirs_changed = true;
                }
            }
            items.push(Value::Table(item));
        } else if known.contains(&key) {
            // Removed here: not passed on.
            theirs_changed = true;
        } else {
            ours.push(annotation_of(x, &item, key));
            matched.push(true);
            ours_changed = true;
            items.push(Value::Table(item));
        }
    }
    // Ours that KOReader does not have: new here, or removed there.
    let mut kept = Vec::with_capacity(ours.len());
    for (a, matched) in ours.into_iter().zip(matched) {
        if matched {
            kept.push(a);
        } else if known.contains(&key_of(&a.mark)) && !pending {
            ours_changed = true;
        } else {
            if let Some(item) = item_of(x, &a) {
                items.push(item);
                theirs_changed = true;
            }
            kept.push(a);
        }
    }
    if ours_changed {
        annotations::save(storage, book, &kept);
    }
    if theirs_changed {
        let file = lua::table([
            ("annotations", Value::Table(Table::from_list(items))),
            ("datetime", lua::string(&datetime(x, x.now))),
            ("device_id", lua::string(DEVICE)),
        ]);
        let _ = storage.write(&exchange_file(book), lua::write(&file).as_bytes());
    }
    let known: Vec<Key> = kept.iter().map(|a| key_of(&a.mark)).collect();
    save_known(storage, book, &known);
    ours_changed
}

/// Exchanges the annotations of the book at `book` with KOReader's, if
/// KOReader knows the book.
fn exchange_book(env: &Env<'_>, book: &str, out: &mut Vec<Command>) {
    if !has_entries(env.storage, &sidecar(book)) {
        return;
    }
    let Ok(doc) = env.formats.open(env.storage, book) else {
        return;
    };
    let local = |unix| time::local_seconds(env, unix);
    let x = Exchange {
        storage: env.storage,
        doc: doc.as_ref(),
        now: time::unix_seconds(env).unwrap_or(0),
        local: &local,
        drawer: if env.settings.text(HIGHLIGHT_STYLE) == UNDERLINE {
            "underscore"
        } else {
            "lighten"
        },
    };
    if exchange(&x, book) {
        out.push(annotations::changed(book));
    }
}

/// A book moved: KOReader's folder and what is known go along.
fn moved(env: &Env<'_>, p: &Params, _: &mut Vec<Command>) {
    let (Some(from), Some(to)) = (p.get(params::FROM), p.get(params::PATH)) else {
        return;
    };
    let storage = env.storage;
    if has_entries(storage, &sidecar(from)) {
        let _ = storage.rename(&sidecar(from), &sidecar(to));
        let old = format!(
            "{}/{}.annotations.lua",
            sidecar(to),
            quireink_sdk::files::name(from)
        );
        let _ = storage.rename(&old, &exchange_file(to));
    }
    let _ = storage.rename(&known_file(from), &known_file(to));
}

/// The KOReader plugin.
#[derive(Debug, Default)]
pub struct KoReader;

impl Plugin for KoReader {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: PLUGIN,
            name: Cow::Borrowed("KOReader"),
            description: Cow::Borrowed(
                "Shares bookmarks and highlights with KOReader for books read in both.",
            ),
            version: Version::new(0, 1, 0),
            sdk: SDK_VERSION,
        }
    }

    fn register(&self, r: &mut dyn Registrar) -> Result<(), RegisterError> {
        for event in [events::BOOK_OPENED, events::ANNOTATIONS_CHANGED] {
            r.subscribe(
                event,
                Box::new(|env: &Env<'_>, p: &Params, out: &mut Vec<Command>| {
                    if let Some(book) = p.get(params::PATH) {
                        exchange_book(env, book, out);
                    }
                }),
            )?;
        }
        r.subscribe(events::BOOK_MOVED, Box::new(moved))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/en.ini")))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/de.ini")))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
