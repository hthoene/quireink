//! The library by author and by series.
//!
//! A catalog of every book under the library folder (`catalog.txt`, one
//! book a line: path, author, title, series, number) is made anew in the
//! background whenever one of these views opens: from the details the
//! library keeps of each book, opening only books not seen before, a few
//! entries at a time. The view shows the catalog there was until the new
//! one is complete.
//!
//! The first page lists the authors (or series) with how many books each
//! has; choosing one lists those books, a series by its numbers. Only
//! the names are held in memory, never the whole catalog.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::{actions, params, slots};
use quireink_sdk::ui::{self, ListChange, ListView, UiStyle};
use quireink_sdk::{
    Arg, Canvas, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Params, Rect, RefreshMode,
    Screen, Storage, files,
};

use crate::books;
use crate::listing::{Entry, Listing};
use crate::{PLUGIN, ROOT};

/// The view of authors (`mode` = `authors`) or series (`series`).
pub(crate) const MODE_AUTHORS: &str = "authors";
pub(crate) const MODE_SERIES: &str = "series";
/// Names held at most.
const MAX_NAMES: usize = 2000;
/// Entries looked at, and books opened, in one step of making the catalog.
const STEP_ENTRIES: usize = 12;
/// Pause between steps, so the screen stays responsive.
const STEP_DELAY_MS: u64 = 30;

fn catalog_path() -> String {
    format!("{}/catalog.txt", Env::plugin_dir(PLUGIN.as_str()))
}

fn building_path() -> String {
    format!("{}/catalog.tmp", Env::plugin_dir(PLUGIN.as_str()))
}

/// A book in the catalog.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Line {
    pub path: String,
    pub author: String,
    pub title: String,
    pub series: String,
    pub number: String,
}

impl Line {
    fn parse(text: &str) -> Option<Line> {
        let mut fields = text.split('\t');
        let path = fields.next().filter(|p| !p.is_empty())?.to_string();
        let mut next = || fields.next().unwrap_or("").to_string();
        Some(Line {
            path,
            author: next(),
            title: next(),
            series: next(),
            number: next(),
        })
    }

    fn text(&self) -> String {
        let clean = |s: &str| s.replace(['\t', '\n', '\r'], " ");
        format!(
            "{}\t{}\t{}\t{}\t{}\n",
            clean(&self.path),
            clean(&self.author),
            clean(&self.title),
            clean(&self.series),
            clean(&self.number)
        )
    }

    /// What it is listed under in a view.
    fn key(&self, series: bool) -> &str {
        if series { &self.series } else { &self.author }
    }
}

/// Calls `f` with each book of the catalog, reading it in pieces.
fn each_line(storage: &dyn Storage, path: &str, f: &mut dyn FnMut(Line)) {
    let mut buf = [0u8; 2048];
    let mut rest: Vec<u8> = Vec::new();
    let mut offset = 0u64;
    while let Ok(n) = storage.read_at(path, offset, &mut buf) {
        if n == 0 {
            break;
        }
        offset += n as u64;
        rest.extend_from_slice(&buf[..n]);
        while let Some(end) = rest.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = rest.drain(..=end).collect();
            if let Some(line) = Line::parse(String::from_utf8_lossy(&line).trim_end()) {
                f(line);
            }
        }
    }
    if let Some(line) = Line::parse(String::from_utf8_lossy(&rest).trim_end()) {
        f(line);
    }
}

/// Making the catalog anew: the folders still to walk, and where the
/// current one is.
#[derive(Debug, Default)]
struct Walk {
    dirs: Vec<String>,
    dir: Option<String>,
    last: Option<Entry>,
    /// Books found so far.
    found: usize,
}

impl Walk {
    fn start(env: &Env<'_>) -> Walk {
        let root = env
            .settings
            .text(ROOT.as_str())
            .trim_matches('/')
            .to_string();
        let _ = env.storage.write(&building_path(), &[]);
        Walk {
            dirs: Vec::from([root]),
            ..Walk::default()
        }
    }

    /// One step: a few entries of a folder, at most one book opened;
    /// `true` when the catalog is complete.
    fn step(&mut self, env: &Env<'_>) -> bool {
        let Some(dir) = self.dir.clone().or_else(|| self.dirs.pop()) else {
            // Done: the new catalog replaces the old one.
            let _ = files::move_to(env.storage, &building_path(), &catalog_path());
            return true;
        };
        self.dir = Some(dir.clone());
        let listing = Listing {
            storage: env.storage,
            formats: env.formats,
            dir: &dir,
            folders_only: false,
            rank: None,
        };
        let entries = listing.after(self.last.as_ref(), STEP_ENTRIES);
        if entries.is_empty() {
            self.dir = None;
            self.last = None;
            return false;
        }
        for entry in entries {
            let path = files::join(&dir, &entry.name);
            let mut opened = false;
            if entry.is_dir {
                self.dirs.push(path);
            } else {
                let cached = books::load(env.storage, &path).filter(|b| b.indexed);
                let book = cached.unwrap_or_else(|| {
                    opened = true;
                    books::index(env, &path)
                });
                let title = book.title.unwrap_or_else(|| {
                    let name = entry.name.as_str();
                    name.rsplit_once('.').map_or(name, |(n, _)| n).to_string()
                });
                let line = Line {
                    path,
                    author: book.author.unwrap_or_default(),
                    title,
                    series: book.series.unwrap_or_default(),
                    number: book.series_index.unwrap_or_default(),
                };
                let _ = env.storage.append(&building_path(), line.text().as_bytes());
                self.found += 1;
            }
            self.last = Some(entry);
            // Opening a book takes a moment: the rest in the next step.
            if opened {
                break;
            }
        }
        false
    }
}

/// The names of the authors or series and how many books each has,
/// alphabetically.
fn names(storage: &dyn Storage, series: bool) -> Vec<(String, usize)> {
    let mut names: Vec<(String, usize)> = Vec::new();
    each_line(storage, &catalog_path(), &mut |line| {
        let key = line.key(series);
        if series && key.is_empty() {
            return;
        }
        if let Some((_, count)) = names.iter_mut().find(|(n, _)| n == key) {
            *count += 1;
        } else if names.len() < MAX_NAMES {
            names.push((key.to_string(), 1));
        }
    });
    names.sort_by_key(|(n, _)| n.to_lowercase());
    names
}

/// The books of author or series `name`: a series by its numbers, an
/// author's by title.
fn books_of(storage: &dyn Storage, series: bool, name: &str) -> Vec<Line> {
    let mut books = Vec::new();
    each_line(storage, &catalog_path(), &mut |line| {
        if line.key(series) == name {
            books.push(line);
        }
    });
    let number = |l: &Line| l.number.parse::<f32>().unwrap_or(f32::MAX);
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
    series: bool,
    names: Vec<(String, usize)>,
    /// The author or series chosen, with its books.
    chosen: Option<(String, Vec<Line>)>,
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
        let series = params.get(params::MODE) == Some(MODE_SERIES);
        Self {
            list: ListView::new(area, style.noted_row()).focus_on_keys(env),
            names: names(env.storage, series),
            chosen: None,
            names_focus: 0,
            walk: None,
            walked: false,
            series,
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
                    let books = books_of(env.storage, self.series, name);
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
            Some((name, _)) if name.is_empty() => env.i18n.tr("files.unknown-author").to_string(),
            Some((name, _)) => name.clone(),
            None if self.series => env.i18n.tr("files.by-series").to_string(),
            None => env.i18n.tr("files.by-author").to_string(),
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
                Some(walk) => env.i18n.plural("files.collecting", walk.found as i64, &[]),
                None if self.series => env.i18n.tr("files.no-series").to_string(),
                None => env.i18n.tr("files.no-books").to_string(),
            };
            ui::draw_message(canvas, env, &style, self.list.area, &message);
            return;
        }
        let series = self.series;
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
                    } else if series {
                        book.author.clone()
                    } else if book.number.is_empty() {
                        book.series.clone()
                    } else {
                        let number = env
                            .i18n
                            .format("files.series-number", &[("number", Arg::Str(&book.number))]);
                        format!("{} · {number}", book.series)
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
        if self.walk.is_none() && !self.walked {
            let env = *cx.env();
            self.walk = Some(Walk::start(&env));
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
        self.names = names(env.storage, self.series);
        if let Some((name, books)) = &mut self.chosen {
            *books = books_of(env.storage, self.series, name);
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
            if self.chosen.take().is_some() {
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
        let lines = [
            ("b/2.epub", "Ann", "Second", "Tales", "2"),
            ("b/1.epub", "Ann", "First", "Tales", "1"),
            ("b/x.epub", "bob", "Alone", "", ""),
            ("b/y.txt", "", "Nobody's", "", ""),
        ];
        let mut text = String::new();
        for (path, author, title, series, number) in lines {
            text.push_str(
                &Line {
                    path: path.into(),
                    author: author.into(),
                    title: title.into(),
                    series: series.into(),
                    number: number.into(),
                }
                .text(),
            );
        }
        storage.write(&catalog_path(), text.as_bytes()).unwrap();
        let authors = names(&storage, false);
        assert_eq!(
            authors,
            [
                (String::new(), 1),
                (String::from("Ann"), 2),
                (String::from("bob"), 1)
            ]
        );
        assert_eq!(names(&storage, true), [(String::from("Tales"), 2)]);
        let tales = books_of(&storage, true, "Tales");
        assert_eq!(tales[0].title, "First", "by number");
        assert_eq!(tales[1].path, "b/2.epub");
    }
}
