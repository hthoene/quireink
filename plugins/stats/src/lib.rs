//! Reading statistics: how long, how much and on which days one reads,
//! with an overview, a calendar and every book's numbers.
//!
//! Recorded as KOReader records its statistics, so the two can exchange
//! them: every stay on a page with its start, duration and the book's
//! pages then; stays shorter than *Shortest page* (5 s) are left out and
//! longer ones count as *Longest page* (2 min), so a book left open does
//! not count as read. Books are recognised by KOReader's fingerprint of
//! the file (a partial MD5). Reading pauses while the app is in the
//! background. See `store` for the files.
//!
//! The statistics are shared with KOReader's: through the cloud folder
//! KOReader syncs them to (WebDAV), and by taking over those of KOReader
//! on the same device (see `sync`); its database format is in
//! `koreader`, the merge in `merge`.

#![no_std]

extern crate alloc;

mod book;
mod books;
mod calendar;
mod chart;
mod extsort;
mod fmt;
mod koreader;
mod left;
mod merge;
mod overview;
mod store;
mod sync;
mod totals;
mod widget;

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::RefCell;

use quireink_sdk::builtin::{events, menus, params};
use quireink_sdk::{
    ActionSpec, Command, Env, Id, Manifest, MenuEntry, Params, Plugin, RegisterError, Registrar,
    SDK_VERSION, ScreenSpec, SettingGroupSpec, SettingKind, SettingSpec, SettingValue, Version,
    md5, time,
};

use store::{Book, PageStat};

pub const PLUGIN: Id = Id::new_static("stats");
/// Records reading.
pub const ENABLED: Id = Id::new_static("stats.enabled");
/// Stays on a page shorter than this many seconds are not counted.
pub const MIN_SECONDS: Id = Id::new_static("stats.min-seconds");
/// Stays on a page count at most this many seconds.
pub const MAX_SECONDS: Id = Id::new_static("stats.max-seconds");
/// Opens the overview.
pub const OPEN: Id = Id::new_static("stats.open");

pub(crate) const GROUP: Id = Id::new_static("stats.group");
const OVERVIEW: Id = Id::new_static("stats.overview");
const CALENDAR: Id = Id::new_static("stats.calendar");
const BOOKS: Id = Id::new_static("stats.books");
const BOOK: Id = Id::new_static("stats.book");

/// The book being read and the page it is on.
#[derive(Debug, Clone)]
struct Session {
    book: u16,
    path: String,
    page: u32,
    pages: u32,
    /// When reading the page began (Unix seconds); `None` while paused.
    since: Option<i64>,
}

#[derive(Debug, Default)]
struct State {
    session: Option<Session>,
}

type Shared = Rc<RefCell<State>>;

/// The page and the number of pages of a reading event: as the reader
/// counted them, else the progress in tenths of a percent of 1000 pages
/// (KOReader rescales pages to the book's count when it sums them).
fn page_of(params: &Params) -> Option<(u32, u32)> {
    let number = |key| params.get(key).and_then(|v| v.parse::<u32>().ok());
    if let (Some(page), Some(pages)) = (number(params::PAGE), number(params::PAGES))
        && pages > 0
    {
        return Some((page.clamp(1, pages), pages));
    }
    let progress: f32 = params.get(params::PROGRESS)?.parse().ok()?;
    let page = (progress * 10.0) as u32 + 1;
    Some((page.clamp(1, 1000), 1000))
}

/// How long a stay counts: nothing if shorter than the minimum, at most
/// the maximum (KOReader's rule).
fn counted(seconds: i64, min: i64, max: i64) -> u16 {
    if seconds < min {
        0
    } else {
        seconds.min(max).clamp(0, i64::from(u16::MAX)) as u16
    }
}

impl State {
    /// Records the stay on the current page until `now`.
    fn finish_page(&mut self, env: &Env<'_>, now: i64) {
        let Some(session) = &mut self.session else {
            return;
        };
        let Some(since) = session.since.take() else {
            return;
        };
        let min = env.settings.int(MIN_SECONDS.as_str());
        let max = env.settings.int(MAX_SECONDS.as_str());
        let duration = counted(now - since, min, max);
        if duration > 0 && since > 0 {
            store::append(
                env.storage,
                PageStat {
                    book: session.book,
                    page: session.page,
                    start: since as u32,
                    duration,
                    pages: session.pages,
                },
            );
        }
    }

    /// The reader shows `page` of `pages`.
    fn turn(&mut self, env: &Env<'_>, now: i64, (page, pages): (u32, u32)) {
        let changed = self
            .session
            .as_ref()
            .is_some_and(|s| (s.page, s.pages) != (page, pages) || s.since.is_none());
        if !changed {
            return;
        }
        self.finish_page(env, now);
        let Some(session) = &mut self.session else {
            return;
        };
        // The reader has counted the pages (or counted anew).
        if pages != session.pages && pages != 1000 {
            let mut books = store::books(env.storage);
            if let Some(book) = books.iter_mut().find(|b| b.id == session.book) {
                book.pages = pages;
                store::save_books(env.storage, &books);
            }
        }
        session.page = page;
        session.pages = pages;
        session.since = Some(now);
    }

    /// A book is shown: the same one again (after a menu), or another.
    fn open(&mut self, env: &Env<'_>, now: i64, params: &Params) {
        let (Some(path), Some(at)) = (params.get(params::PATH), page_of(params)) else {
            return;
        };
        if self.session.as_ref().is_some_and(|s| s.path == path) {
            return self.turn(env, now, at);
        }
        self.finish_page(env, now);
        self.session = register_book(env, path, now, at.1).map(|book| Session {
            book,
            path: path.to_string(),
            page: at.0,
            pages: at.1,
            since: Some(now),
        });
    }
}

/// The book at `path` in `books.ini`, added if new: recognised by its
/// fingerprint, with title and authors as KOReader takes them.
fn register_book(env: &Env<'_>, path: &str, now: i64, pages: u32) -> Option<u16> {
    let md5 = md5::partial(env.storage, path)?;
    let mut books = store::books(env.storage);
    let at = match books.iter().position(|b| b.md5 == md5) {
        Some(at) => at,
        None => {
            let id = books.iter().map(|b| b.id).max().unwrap_or(0) + 1;
            let document = env.formats.open(env.storage, path).ok();
            let metadata = document.as_ref().map(|d| d.metadata().clone());
            let file = path.rsplit('/').next().unwrap_or(path);
            let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
            let (title, authors, language) = match metadata {
                Some(m) => (
                    m.title.unwrap_or_else(|| stem.to_string()),
                    m.authors.join("\n"),
                    m.language.unwrap_or_default(),
                ),
                None => (stem.to_string(), String::new(), String::new()),
            };
            let na = |text: String| {
                if text.is_empty() {
                    String::from("N/A")
                } else {
                    text
                }
            };
            books.push(Book {
                id,
                title,
                authors: na(authors),
                md5,
                pages: 0,
                language: na(language),
                series: String::from("N/A"),
                last_open: 0,
                path: String::new(),
            });
            books.len() - 1
        }
    };
    let book = &mut books[at];
    book.last_open = now;
    book.path = path.to_string();
    // Counted pages, not the per-mille stand-in.
    if pages != 1000 || book.pages == 0 {
        book.pages = pages;
    }
    let id = book.id;
    store::save_books(env.storage, &books);
    Some(id)
}

/// The statistics plugin.
#[derive(Debug, Default)]
pub struct Stats;

impl Plugin for Stats {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: PLUGIN,
            name: Cow::Borrowed("Reading statistics"),
            description: Cow::Borrowed(
                "Records how long and how much you read, with a calendar and every book's numbers.",
            ),
            version: Version::new(0, 1, 0),
            sdk: SDK_VERSION,
        }
    }

    fn register(&self, r: &mut dyn Registrar) -> Result<(), RegisterError> {
        let state: Shared = Rc::new(RefCell::new(State::default()));
        widget::register(r)?;
        left::register(r)?;
        r.add_setting_group(SettingGroupSpec {
            id: GROUP,
            title: Cow::Borrowed("stats.group"),
            order: 25,
        })?;
        r.add_icon(GROUP, Cow::Borrowed("chart-bar"))?;
        let seconds = |id, title: &'static str, (min, max, default), step| {
            SettingSpec::new(
                id,
                title,
                SettingKind::Int { min, max },
                SettingValue::Int(default),
            )
            .unit("stats.unit.seconds")
            .step(step)
            .advanced()
        };
        for spec in [
            SettingSpec::new(
                ENABLED,
                "stats.setting.enabled",
                SettingKind::Bool,
                SettingValue::Bool(true),
            )
            .description("stats.setting.enabled.help"),
            seconds(MIN_SECONDS, "stats.setting.min", (1, 60, 5), 1)
                .description("stats.setting.min.help"),
            seconds(MAX_SECONDS, "stats.setting.max", (30, 600, 120), 10)
                .description("stats.setting.max.help"),
        ] {
            r.add_setting(spec.group(GROUP))?;
        }

        // Recording.
        let s = state.clone();
        r.subscribe(
            events::BOOK_OPENED,
            Box::new(move |env, params, _| {
                if let Some(now) = time::unix_seconds(env).filter(|_| enabled(env)) {
                    s.borrow_mut().open(env, now, params);
                }
            }),
        )?;
        let s = state.clone();
        r.subscribe(
            events::PAGE_TURNED,
            Box::new(move |env, params, _| {
                let mut state = s.borrow_mut();
                let same = state
                    .session
                    .as_ref()
                    .is_some_and(|session| params.get(params::PATH) == Some(&session.path));
                if let (Some(now), Some(at), true) = (
                    time::unix_seconds(env),
                    page_of(params),
                    same && enabled(env),
                ) {
                    state.turn(env, now, at);
                }
            }),
        )?;
        r.subscribe(
            events::BOOK_MOVED,
            Box::new(|env, params, _| {
                let (Some(from), Some(to)) = (params.get(params::FROM), params.get(params::PATH))
                else {
                    return;
                };
                let mut books = store::books(env.storage);
                let mut changed = false;
                for book in books.iter_mut().filter(|b| b.path == from) {
                    book.path = to.to_string();
                    changed = true;
                }
                if changed {
                    store::save_books(env.storage, &books);
                }
            }),
        )?;
        let s = state.clone();
        r.subscribe(
            events::SUSPEND,
            Box::new(move |env, _, _| {
                if let Some(now) = time::unix_seconds(env) {
                    s.borrow_mut().finish_page(env, now);
                }
            }),
        )?;
        let s = state.clone();
        r.subscribe(
            events::RESUME,
            Box::new(move |env, _, _| {
                let mut state = s.borrow_mut();
                if let (Some(session), Some(now)) = (&mut state.session, time::unix_seconds(env)) {
                    session.since.get_or_insert(now);
                }
            }),
        )?;

        // Sharing with KOReader.
        sync::register(r, Rc::new(RefCell::new(sync::Sync::default())))?;

        // The views.
        r.add_screen(ScreenSpec {
            id: OVERVIEW,
            title: Cow::Borrowed("stats.title"),
            factory: Box::new(|env, params| Box::new(overview::Overview::new(env, params))),
        })?;
        r.add_screen(ScreenSpec {
            id: CALENDAR,
            title: Cow::Borrowed("stats.calendar"),
            factory: Box::new(|env, _| Box::new(calendar::Calendar::new(env))),
        })?;
        r.add_screen(ScreenSpec {
            id: BOOKS,
            title: Cow::Borrowed("stats.books"),
            factory: Box::new(|env, _| Box::new(books::BookList::new(env))),
        })?;
        r.add_screen(ScreenSpec {
            id: BOOK,
            title: Cow::Borrowed("stats.title"),
            factory: Box::new(|env, params| Box::new(book::BookView::new(env, params))),
        })?;
        let s = state;
        r.add_action(ActionSpec {
            id: OPEN,
            title: Cow::Borrowed("stats.title"),
            handler: Some(Box::new(move |env: &Env<'_>, out: &mut Vec<Command>| {
                // Recorded so far, so the views include the current page.
                if let Some(now) = time::unix_seconds(env) {
                    let mut state = s.borrow_mut();
                    state.finish_page(env, now);
                    if let Some(session) = &mut state.session {
                        session.since = Some(now);
                    }
                }
                let current = s.borrow().session.as_ref().map(|session| session.book);
                let params = match current {
                    Some(book) => Params::new().with(params::ID, alloc::format!("{book}")),
                    None => Params::new(),
                };
                out.push(Command::OpenScreen(OVERVIEW, params));
            })),
        })?;
        for (menu, order) in [(menus::TOOLS, 35), (Id::new_static("reader.more"), 30)] {
            r.add_menu_entry(MenuEntry {
                menu,
                item: OPEN,
                order,
            })?;
        }
        r.add_icon(OPEN, Cow::Borrowed("chart-bar"))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/en.ini")))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/de.ini")))?;
        Ok(())
    }
}

fn enabled(env: &Env<'_>) -> bool {
    env.settings.bool(ENABLED.as_str())
}

#[cfg(test)]
mod tests {
    use quireink_i18n::{Catalog, check};

    use super::*;

    #[test]
    fn translations_are_complete() {
        let en = Catalog::parse(include_str!("../i18n/en.ini")).unwrap();
        let de = Catalog::parse(include_str!("../i18n/de.ini")).unwrap();
        let issues = check::compare(&en, &de);
        assert!(issues.is_empty(), "{issues:?}");
    }

    #[test]
    fn stays_count_as_koreader_counts_them() {
        assert_eq!(counted(3, 5, 120), 0);
        assert_eq!(counted(5, 5, 120), 5);
        assert_eq!(counted(45, 5, 120), 45);
        assert_eq!(counted(3600, 5, 120), 120);
    }

    #[test]
    fn pages_come_from_the_reader_or_the_progress() {
        let counted = Params::new()
            .with(params::PAGE, "12")
            .with(params::PAGES, "300")
            .with(params::PROGRESS, "4.0");
        assert_eq!(page_of(&counted), Some((12, 300)));
        let estimated = Params::new().with(params::PROGRESS, "42.5");
        assert_eq!(page_of(&estimated), Some((426, 1000)));
        assert_eq!(page_of(&Params::new()), None);
    }
}
