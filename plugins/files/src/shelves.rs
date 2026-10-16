//! The library by author, by series and by tag.
//!
//! A catalog of every book under the library folder (`catalog.txt`, one
//! book a line: path, author, title, series, number, tags) is made anew in the
//! background whenever one of these views opens: from the details the
//! library keeps of each book, opening only books not seen before, a few
//! entries at a time. The view shows the catalog there was until the new
//! one is complete.
//!
//! The first page lists the authors (or series, or tags) with how many
//! books each has; choosing one lists those books, a series by its
//! numbers. Only the names are held in memory, never the whole catalog.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::{actions, params, slots};
use quireink_sdk::ui::{self, ListChange, ListView, UiStyle};
use quireink_sdk::{
    Arg, Canvas, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Params, Rect, RefreshMode,
    Screen, Storage, files,
};

use crate::ROOT;
use quireink_sdk::library;
use quireink_sdk::library::catalog::{self, Record, Walk};

/// The view of authors (`mode` = `authors`), series (`series`) or tags
/// (`tags`).
pub(crate) const MODE_AUTHORS: &str = "authors";
pub(crate) const MODE_SERIES: &str = "series";
pub(crate) const MODE_TAGS: &str = "tags";
/// The favourite books (`favourites`).
pub(crate) const MODE_FAVOURITES: &str = "favourites";
/// The books matching a search (`search`, the words as `text`).
pub(crate) const MODE_SEARCH: &str = "search";
/// What books are listed under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Shelf {
    Authors,
    Series,
    Tags,
    /// No names: the favourite books at once.
    Favourites,
    /// No names: the books a search finds.
    Search,
}

impl Shelf {
    fn of(mode: Option<&str>) -> Shelf {
        match mode {
            Some(MODE_SERIES) => Shelf::Series,
            Some(MODE_TAGS) => Shelf::Tags,
            Some(MODE_FAVOURITES) => Shelf::Favourites,
            Some(MODE_SEARCH) => Shelf::Search,
            _ => Shelf::Authors,
        }
    }
}
/// Names held at most.
const MAX_NAMES: usize = 2000;
/// Pause between steps of making the catalog, so the screen stays
/// responsive.
const STEP_DELAY_MS: u64 = 30;

/// What a book is listed under in a view: its author, its series, or
/// each of its tags.
fn keys(record: &Record, shelf: Shelf) -> Vec<&str> {
    match shelf {
        Shelf::Authors => alloc::vec![record.author.as_str()],
        Shelf::Series => alloc::vec![record.series.as_str()],
        Shelf::Tags => record.tags().collect(),
        Shelf::Favourites | Shelf::Search => Vec::new(),
    }
}

/// The names of the authors, series or tags and how many books each
/// has, alphabetically.
fn names(storage: &dyn Storage, shelf: Shelf) -> Vec<(String, usize)> {
    let mut names: Vec<(String, usize)> = Vec::new();
    catalog::each(storage, &mut |line| {
        for key in keys(&line, shelf) {
            // Books without series are no series; those without author
            // are listed under "Unknown author".
            if shelf == Shelf::Series && key.is_empty() {
                continue;
            }
            if let Some((_, count)) = names.iter_mut().find(|(n, _)| n == key) {
                *count += 1;
            } else if names.len() < MAX_NAMES {
                names.push((key.to_string(), 1));
            }
        }
    });
    names.sort_by_key(|(n, _)| n.to_lowercase());
    names
}

/// A book's format as its file says, e.g. `EPUB`; a zipped book by
/// what is inside (`FB2` for `.fb2.zip`).
fn format_of(path: &str) -> String {
    let name = files::name(path);
    let lower = name.to_ascii_lowercase();
    let name = match lower.strip_suffix(".zip") {
        Some(inner) if inner.contains('.') => &name[..inner.len()],
        _ => name,
    };
    name.rsplit_once('.')
        .map(|(_, ext)| ext.to_uppercase())
        .unwrap_or_default()
}

/// The favourite books still there, as catalog lines from the details
/// the library keeps.
fn favourites(env: &Env<'_>) -> Vec<Record> {
    library::favourites(env.storage)
        .into_iter()
        .filter(|path| env.storage.size(path).is_ok())
        .map(|path| {
            Record::of(
                &path,
                &library::book(env.storage, &path).unwrap_or_default(),
            )
        })
        .collect()
}

/// The books of author, series or tag `name`: a series by its numbers,
/// the others by title.
fn books_of(storage: &dyn Storage, shelf: Shelf, name: &str) -> Vec<Record> {
    let series = shelf == Shelf::Series;
    let mut books = Vec::new();
    catalog::each(storage, &mut |line| {
        if keys(&line, shelf).contains(&name) {
            books.push(line);
        }
    });
    let number = |l: &Record| l.number.parse::<f32>().unwrap_or(f32::MAX);
    books.sort_by(|a, b| {
        let by_number = if series {
            number(a).total_cmp(&number(b))
        } else {
            core::cmp::Ordering::Equal
        };
        by_number.then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
    });
    books
}

pub(crate) struct ShelfScreen {
    style: UiStyle,
    header: ui::Header,
    list: ListView,
    shelf: Shelf,
    names: Vec<(String, usize)>,
    /// The author or series chosen, with its books.
    chosen: Option<(String, Vec<Record>)>,
    /// Where the list of names was, to return there.
    names_focus: usize,
    /// The catalog being made anew, until it is complete.
    walk: Option<Walk>,
    walked: bool,
}

impl ShelfScreen {
    pub(crate) fn new(env: &Env<'_>, params: &Params) -> Self {
        let style = UiStyle::new(env);
        let header = ui::Header::new(env, &style, true);
        let display = env.device.display;
        let top = header.area.height + style.gap as u16;
        let area = Rect::new(0, top, display.width, display.height.saturating_sub(top));
        let shelf = Shelf::of(params.get(params::MODE));
        let chosen = match shelf {
            Shelf::Favourites => Some((String::new(), favourites(env))),
            Shelf::Search => {
                let query = params
                    .get(params::TEXT)
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                let found = catalog::search(env.storage, &query);
                Some((query, found))
            }
            _ => None,
        };
        Self {
            list: ListView::new(area, style.noted_row()).focus_on_keys(env),
            names: names(env.storage, shelf),
            chosen,
            names_focus: 0,
            walk: None,
            walked: false,
            shelf,
            style,
            header,
        }
    }

    fn len(&self) -> usize {
        match &self.chosen {
            Some((_, books)) => books.len(),
            None => self.names.len(),
        }
    }

    fn redraw(&self, cx: &mut Cx<'_, '_>) {
        cx.request_redraw();
        cx.request_refresh(RefreshMode::Partial);
    }

    fn activate(&mut self, index: usize, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        match &self.chosen {
            Some((_, books)) => {
                if let Some(book) = books.get(index) {
                    cx.command(Command::OpenSlot(
                        slots::READER,
                        Params::new().with(params::PATH, book.path.clone()),
                    ));
                }
            }
            None => {
                if let Some((name, _)) = self.names.get(index) {
                    let books = books_of(env.storage, self.shelf, name);
                    self.chosen = Some((name.clone(), books));
                    self.names_focus = index;
                    self.list.focus = 0;
                    self.redraw(cx);
                }
            }
        }
    }

    fn title(&self, env: &Env<'_>) -> String {
        match &self.chosen {
            _ if self.shelf == Shelf::Favourites => env.i18n.tr("files.favourites").to_string(),
            Some((query, _)) if self.shelf == Shelf::Search => env
                .i18n
                .format("files.search.title", &[("text", Arg::Str(query))]),
            Some((name, _)) if name.is_empty() => env.i18n.tr("files.unknown-author").to_string(),
            Some((name, _)) => name.clone(),
            None => env
                .i18n
                .tr(match self.shelf {
                    Shelf::Authors => "files.by-author",
                    Shelf::Series => "files.by-series",
                    Shelf::Tags => "files.by-tag",
                    Shelf::Favourites => "files.favourites",
                    Shelf::Search => "files.search",
                })
                .to_string(),
        }
    }
}

impl Screen for ShelfScreen {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        let len = self.len();
        let title = self.title(env);
        self.header
            .draw(canvas, env, &style, &title, None, self.list.paging(len));
        if len == 0 {
            let message = match &self.walk {
                Some(walk) => env
                    .i18n
                    .plural("files.collecting", walk.found() as i64, &[]),
                None if self.shelf == Shelf::Series => env.i18n.tr("files.no-series").to_string(),
                None if self.shelf == Shelf::Tags => env.i18n.tr("files.no-tags").to_string(),
                _ if self.shelf == Shelf::Favourites => {
                    env.i18n.tr("files.no-favourites").to_string()
                }
                _ if self.shelf == Shelf::Search => env.i18n.tr("files.search.none").to_string(),
                None => env.i18n.tr("files.no-books").to_string(),
            };
            ui::draw_message(canvas, env, &style, self.list.area, &message);
            return;
        }
        let series = self.shelf == Shelf::Series;
        let tags = matches!(self.shelf, Shelf::Tags | Shelf::Favourites | Shelf::Search);
        let (names, chosen) = (&self.names, &self.chosen);
        self.list.draw(
            canvas,
            &style,
            len,
            |canvas, index, rect, focused| match chosen {
                Some((_, books)) => {
                    let book = &books[index];
                    let note = if series && !book.number.is_empty() {
                        env.i18n
                            .format("files.series-number", &[("number", Arg::Str(&book.number))])
                    } else if series || tags {
                        book.author.clone()
                    } else if book.number.is_empty() {
                        book.series.clone()
                    } else {
                        let number = env
                            .i18n
                            .format("files.series-number", &[("number", Arg::Str(&book.number))]);
                        format!("{} · {number}", book.series)
                    };
                    // The format, so editions of a book are told apart.
                    let format = format_of(&book.path);
                    let note = match (note.is_empty(), format.is_empty()) {
                        (_, true) => note,
                        (true, false) => format,
                        (false, false) => format!("{note} · {format}"),
                    };
                    ui::draw_noted_row(canvas, env, &style, rect, focused, &book.title, "", &note);
                }
                None => {
                    let (name, count) = &names[index];
                    let name = if name.is_empty() {
                        env.i18n.tr("files.unknown-author")
                    } else {
                        name.as_str()
                    };
                    let note = env.i18n.plural("files.books", *count as i64, &[]);
                    ui::draw_noted_row(canvas, env, &style, rect, focused, name, "›", &note);
                }
            },
        );
    }

    fn on_show(&mut self, cx: &mut Cx<'_, '_>) {
        if self.shelf == Shelf::Favourites {
            // Back from a book or its options: maybe one less.
            let env = *cx.env();
            self.chosen = Some((String::new(), favourites(&env)));
            self.list.focus = self.list.focus.min(self.len().saturating_sub(1));
            cx.request_redraw();
            return;
        }
        if self.walk.is_none() && !self.walked {
            let env = *cx.env();
            self.walk = Some(Walk::start(env.storage, env.settings.text(ROOT.as_str())));
            cx.wake_at(env.now);
        }
    }

    fn on_timer(&mut self, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        let Some(walk) = &mut self.walk else {
            return;
        };
        if !walk.step(&env) {
            if self.names.is_empty() {
                // "Collecting …" counts up.
                self.redraw(cx);
            }
            cx.wake_at(env.now.add_millis(STEP_DELAY_MS));
            return;
        }
        // The new catalog: the lists again, keeping the place.
        self.walk = None;
        self.walked = true;
        self.names = names(env.storage, self.shelf);
        if let Some((name, books)) = &mut self.chosen {
            *books = if self.shelf == Shelf::Search {
                catalog::search(env.storage, name)
            } else {
                books_of(env.storage, self.shelf, name)
            };
        }
        self.list.focus = self.list.focus.min(self.len().saturating_sub(1));
        self.redraw(cx);
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let a = action;
        let len = self.len();
        let rows = self.list.rows() as isize;
        let delta = if [actions::UP, actions::LEFT, actions::PREVIOUS].contains(a) {
            -1
        } else if [actions::DOWN, actions::RIGHT, actions::NEXT].contains(a) {
            1
        } else if *a == actions::NEXT_PAGE {
            rows
        } else if *a == actions::PREV_PAGE {
            -rows
        } else if *a == actions::CONFIRM {
            self.activate(self.list.focus, cx);
            return Handled::Yes;
        } else if *a == actions::BACK {
            if !matches!(self.shelf, Shelf::Favourites | Shelf::Search)
                && self.chosen.take().is_some()
            {
                self.list.focus = self.names_focus;
                self.redraw(cx);
            } else {
                cx.command(Command::Back);
            }
            return Handled::Yes;
        } else {
            return Handled::No;
        };
        let change = self.list.move_focus(delta, len);
        if change != ListChange::None {
            cx.request_redraw();
            cx.request_refresh(if change == ListChange::Page {
                RefreshMode::Partial
            } else {
                RefreshMode::Fast
            });
        }
        Handled::Yes
    }

    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        if gesture.kind != GestureKind::Tap {
            return Handled::No;
        }
        let len = self.len();
        let paging = self.list.paging(len);
        if let Some(action) = self
            .header
            .tap(cx.env(), &self.style, gesture.start, paging)
        {
            cx.command(Command::RunAction(action));
            return Handled::Yes;
        }
        match self.list.hit(gesture.start, len) {
            Some(index) => {
                self.list.focus = index;
                self.activate(index, cx);
                Handled::Yes
            }
            None => Handled::No,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quireink_sdk::MemoryStorage;

    #[test]
    fn catalog_lines_and_views() {
        let storage = MemoryStorage::new();
        let books = [
            (
                "b/2.epub",
                "Ann",
                "Second",
                "Tales",
                "2",
                &["Fantasy", "Kids"][..],
            ),
            ("b/1.epub", "Ann", "First", "Tales", "1", &["Fantasy"]),
            ("b/x.epub", "bob", "Alone", "", "", &[]),
            ("b/y.txt", "", "Nobody's", "", "", &[]),
        ];
        let records: Vec<Record> = books
            .iter()
            .map(|(path, author, title, series, number, tags)| {
                let some = |s: &str| (!s.is_empty()).then(|| String::from(s));
                let book = library::Book {
                    author: some(author),
                    title: some(title),
                    series: some(series),
                    series_index: some(number),
                    tags: tags.iter().map(|t| String::from(*t)).collect(),
                    ..library::Book::default()
                };
                Record::of(path, &book)
            })
            .collect();
        catalog::write(&storage, &records);
        let authors = names(&storage, Shelf::Authors);
        assert_eq!(
            authors,
            [
                (String::new(), 1),
                (String::from("Ann"), 2),
                (String::from("bob"), 1)
            ]
        );
        assert_eq!(names(&storage, Shelf::Series), [(String::from("Tales"), 2)]);
        assert_eq!(
            names(&storage, Shelf::Tags),
            [(String::from("Fantasy"), 2), (String::from("Kids"), 1)]
        );
        assert_eq!(books_of(&storage, Shelf::Tags, "Kids")[0].title, "Second");
        // Searching: every word, anywhere, in any case.
        let found: Vec<String> = catalog::search(&storage, "ann TALES")
            .into_iter()
            .map(|l| l.title)
            .collect();
        assert_eq!(found, ["First", "Second"]);
        assert_eq!(catalog::search(&storage, "kids")[0].title, "Second");
        assert!(catalog::search(&storage, "nothing").is_empty());
        assert_eq!(format_of("b/x.epub"), "EPUB");
        assert_eq!(format_of("b/y.FB2.zip"), "FB2");
        let tales = books_of(&storage, Shelf::Series, "Tales");
        assert_eq!(tales[0].title, "First", "by number");
        assert_eq!(tales[1].path, "b/2.epub");
    }
}
