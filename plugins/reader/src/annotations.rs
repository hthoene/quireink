//! Bookmarks and highlights of a book, and the screen that lists them.
//! The list ends with *Export as Markdown*: a file in `Notes` beside the
//! library folder, then the event [`EXPORTED`] for plugins that take it
//! further (a notes app, a sync).
//!
//! They are kept in `annotations.txt` in the book's data folder, one per
//! line, sorted by position: kind, start and end (section and offset in
//! the source, so they outlast font sizes and layouts), the reading
//! progress, a snippet of the text and the reader's note on it. While
//! reading, only the positions are held in memory.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::{actions, params};
use quireink_sdk::document::Locator;
use quireink_sdk::ui::{self, ListChange, ListView, UiStyle};
use quireink_sdk::{
    Canvas, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Params, Rect, RefreshMode, Screen,
    Storage,
};

use crate::GOTO;
use crate::toc::locator_params;

/// Emitted after the notes of a book were exported: parameters `path`
/// (the Markdown file) and `title` (the book's).
pub const EXPORTED: Id = Id::new_static("reader.notes-exported");
/// Where exported notes go, beside the library folder.
const NOTES: &str = "Notes";

/// Longest snippet kept with an annotation.
const MAX_TEXT: usize = 240;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Bookmark,
    Highlight,
}

/// Where an annotation is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Mark {
    pub kind: Kind,
    pub start: Locator,
    pub end: Locator,
    /// It has a note.
    pub noted: bool,
}

impl Mark {
    /// Whether the mark covers part of `start..end`.
    pub fn overlaps(&self, start: Locator, end: Locator) -> bool {
        self.start < end && start < self.end
    }
}

/// An annotation with what is shown in the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Annotation {
    pub mark: Mark,
    /// Reading progress at the start, in percent.
    pub percent: u8,
    pub text: String,
    /// What the reader noted on it, if anything.
    pub note: String,
}

fn file(dir: &str) -> String {
    format!("{dir}/annotations.txt")
}

/// Every annotation of the book whose data is in `dir`, by position.
pub(crate) fn load(storage: &dyn Storage, dir: &str) -> Vec<Annotation> {
    let Ok(data) = quireink_sdk::read_file(storage, &file(dir), 1 << 20) else {
        return Vec::new();
    };
    String::from_utf8_lossy(&data)
        .lines()
        .filter_map(parse)
        .collect()
}

/// Only where the annotations are, for drawing them.
pub(crate) fn marks(storage: &dyn Storage, dir: &str) -> Vec<Mark> {
    load(storage, dir).into_iter().map(|a| a.mark).collect()
}

fn parse(line: &str) -> Option<Annotation> {
    let mut fields = line.splitn(8, '\t');
    let kind = match fields.next()? {
        "b" => Kind::Bookmark,
        "h" => Kind::Highlight,
        _ => return None,
    };
    let mut number = || fields.next()?.parse::<u32>().ok();
    let start = Locator::new(number()?, number()?);
    let end = Locator::new(number()?, number()?);
    let percent = number()?.min(100) as u8;
    let text = fields.next().unwrap_or_default().to_string();
    let note = fields.next().unwrap_or_default().to_string();
    Some(Annotation {
        mark: Mark {
            kind,
            start,
            end,
            noted: !note.is_empty(),
        },
        percent,
        text,
        note,
    })
}

fn save(storage: &dyn Storage, dir: &str, annotations: &[Annotation]) {
    let mut out = String::new();
    for a in annotations {
        let kind = match a.mark.kind {
            Kind::Bookmark => 'b',
            Kind::Highlight => 'h',
        };
        out.push_str(&format!(
            "{kind}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            a.mark.start.section,
            a.mark.start.offset,
            a.mark.end.section,
            a.mark.end.offset,
            a.percent,
            a.text,
            one_line(&a.note)
        ));
    }
    let _ = storage.write(&file(dir), out.as_bytes());
}

/// `text` on one line, at most [`MAX_TEXT`] bytes.
pub(crate) fn snippet(text: &str) -> String {
    let mut out: String = text
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .collect();
    if out.len() > MAX_TEXT {
        let mut end = MAX_TEXT;
        while !out.is_char_boundary(end) {
            end -= 1;
        }
        out.truncate(end);
        out.push('…');
    }
    out
}

/// `text` on one line.
fn one_line(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c == '\t' || c == '\n' || c == '\r' {
                ' '
            } else {
                c
            }
        })
        .collect::<String>()
        .trim()
        .to_string()
}

/// Puts `note` on the highlight that starts at `start` (an empty one
/// removes it); `false` if there is no such highlight.
pub(crate) fn set_note(storage: &dyn Storage, dir: &str, start: Locator, note: &str) -> bool {
    let mut all = load(storage, dir);
    let Some(a) = all
        .iter_mut()
        .find(|a| a.mark.kind == Kind::Highlight && a.mark.start == start)
    else {
        return false;
    };
    a.note = one_line(note);
    a.mark.noted = !a.note.is_empty();
    save(storage, dir, &all);
    true
}

/// The note on the highlight that starts at `start`.
pub(crate) fn note_of(storage: &dyn Storage, dir: &str, start: Locator) -> String {
    load(storage, dir)
        .into_iter()
        .find(|a| a.mark.kind == Kind::Highlight && a.mark.start == start)
        .map(|a| a.note)
        .unwrap_or_default()
}

/// Adds an annotation.
pub(crate) fn add(storage: &dyn Storage, dir: &str, annotation: Annotation) {
    let mut all = load(storage, dir);
    let at = all.partition_point(|a| a.mark.start <= annotation.mark.start);
    all.insert(at, annotation);
    save(storage, dir, &all);
}

/// Removes the annotations `remove` picks; `true` if there were any.
pub(crate) fn remove(storage: &dyn Storage, dir: &str, remove: impl Fn(&Mark) -> bool) -> bool {
    let mut all = load(storage, dir);
    let before = all.len();
    all.retain(|a| !remove(&a.mark));
    let removed = all.len() != before;
    if removed {
        save(storage, dir, &all);
    }
    removed
}

/// The notes of the book at `path` as Markdown: its title and author,
/// the highlights as quotes, the bookmarks as a list, each with where it
/// is in the book.
pub(crate) fn markdown(env: &Env<'_>, title: &str, author: &str, all: &[Annotation]) -> String {
    let mut text = format!("# {title}\n\n");
    if !author.is_empty() {
        text.push_str(&format!("{author}\n\n"));
    }
    let highlights: Vec<&Annotation> = all
        .iter()
        .filter(|a| a.mark.kind == Kind::Highlight)
        .collect();
    if !highlights.is_empty() {
        text.push_str(&format!("## {}\n\n", env.i18n.tr("reader.highlights")));
        for a in highlights {
            text.push_str(&format!("> {}\n>\n> {} %\n\n", a.text, a.percent));
            if !a.note.is_empty() {
                text.push_str(&format!("{}\n\n", a.note));
            }
        }
    }
    let bookmarks: Vec<&Annotation> = all
        .iter()
        .filter(|a| a.mark.kind == Kind::Bookmark)
        .collect();
    if !bookmarks.is_empty() {
        text.push_str(&format!("## {}\n\n", env.i18n.tr("reader.bookmarks")));
        for a in bookmarks {
            text.push_str(&format!("- {} %: {}\n", a.percent, a.text));
        }
        text.push('\n');
    }
    text
}

/// The folder notes are exported to: `Notes` beside the library folder.
fn notes_dir(env: &Env<'_>) -> String {
    let library = env.settings.text("files.root").trim_matches('/');
    let parent = quireink_sdk::files::parent(library);
    if parent.is_empty() {
        String::from(NOTES)
    } else {
        format!("{parent}/{NOTES}")
    }
}

/// Lists the bookmarks and highlights of a book (parameter `path`);
/// choosing one goes there.
pub(crate) struct AnnotationsScreen {
    path: String,
    annotations: Vec<Annotation>,
    style: UiStyle,
    header: ui::Header,
    list: ListView,
}

impl AnnotationsScreen {
    pub(crate) fn open(env: &Env<'_>, params: &Params) -> Self {
        let path = params.get(params::PATH).unwrap_or_default().to_string();
        let annotations = load(env.storage, &crate::screen::book_dir(&path));
        let style = UiStyle::new(env);
        let display = env.device.display;
        let header = ui::Header::new(env, &style, true);
        let top = header.area.height + style.gap as u16;
        let area = Rect::new(0, top, display.width, display.height.saturating_sub(top));
        Self {
            path,
            annotations,
            list: ListView::new(area, style.noted_row()).focus_on_keys(env),
            style,
            header,
        }
    }

    /// Writes the notes as Markdown and tells where.
    fn export(&self, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        let (title, author) = match env.formats.open(env.storage, &self.path) {
            Ok(document) => {
                let m = document.metadata();
                (m.title.clone().unwrap_or_default(), m.authors.join(", "))
            }
            Err(_) => (String::new(), String::new()),
        };
        let title = if title.is_empty() {
            quireink_sdk::files::name(&self.path).to_string()
        } else {
            title
        };
        let base = if author.is_empty() {
            title.clone()
        } else {
            format!("{author} - {title}")
        };
        let name =
            quireink_sdk::web::clean_name(&base, 100).unwrap_or_else(|| String::from("notes"));
        let dir = notes_dir(&env);
        let _ = env.storage.create_dir(&dir);
        let file = format!("{dir}/{name}.md");
        let text = markdown(&env, &title, &author, &self.annotations);
        let message = match env.storage.write(&file, text.as_bytes()) {
            Ok(()) => {
                cx.command(Command::Emit(
                    EXPORTED,
                    Params::new()
                        .with(params::PATH, file.clone())
                        .with(params::TITLE, title),
                ));
                env.i18n.format(
                    "reader.exported",
                    &[("path", quireink_sdk::Arg::Str(&file))],
                )
            }
            Err(_) => env.i18n.tr("reader.export-failed").to_string(),
        };
        cx.command(Command::Notify(message));
    }

    /// The rows: the annotations, then *Export* if there are any.
    fn len(&self) -> usize {
        match self.annotations.len() {
            0 => 0,
            n => n + 1,
        }
    }

    fn open_entry(&self, index: usize, cx: &mut Cx<'_, '_>) {
        if index == self.annotations.len() && index > 0 {
            return self.export(cx);
        }
        if let Some(annotation) = self.annotations.get(index) {
            cx.command(Command::Back);
            cx.command(Command::Emit(
                GOTO,
                locator_params(&self.path, annotation.mark.start),
            ));
        }
    }
}

impl Screen for AnnotationsScreen {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        let len = self.len();
        self.header.draw(
            canvas,
            env,
            &style,
            env.i18n.tr("reader.annotations"),
            None,
            self.list.paging(len),
        );
        if len == 0 {
            ui::draw_message(
                canvas,
                env,
                &style,
                self.list.area,
                env.i18n.tr("reader.annotations.empty"),
            );
            return;
        }
        let annotations = &self.annotations;
        self.list
            .draw(canvas, &style, len, |canvas, index, rect, focused| {
                let Some(a) = annotations.get(index) else {
                    let title = env.i18n.tr("reader.export");
                    let note = env.i18n.tr("reader.export.help");
                    ui::draw_noted_row(canvas, env, &style, rect, focused, title, "›", note);
                    return;
                };
                let (kind, title) = match a.mark.kind {
                    Kind::Bookmark => ("reader.bookmark", a.text.clone()),
                    Kind::Highlight => (
                        "reader.highlight",
                        env.i18n.format(
                            "reader.quoted",
                            &[("text", quireink_sdk::Arg::Str(&a.text))],
                        ),
                    ),
                };
                let what = if a.note.is_empty() {
                    env.i18n.tr(kind)
                } else {
                    a.note.as_str()
                };
                let note = format!("{what} · {} %", a.percent);
                ui::draw_noted_row(canvas, env, &style, rect, focused, &title, "", &note);
            });
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let len = self.len();
        let rows = self.list.rows() as isize;
        let a = action;
        let delta = if [actions::UP, actions::LEFT, actions::PREVIOUS].contains(a) {
            -1
        } else if [actions::DOWN, actions::RIGHT, actions::NEXT].contains(a) {
            1
        } else if *a == actions::NEXT_PAGE {
            rows
        } else if *a == actions::PREV_PAGE {
            -rows
        } else if *a == actions::CONFIRM {
            self.open_entry(self.list.focus, cx);
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
        let env = *cx.env();
        let paging = self.list.paging(self.len());
        if let Some(action) = self.header.tap(&env, &self.style, gesture.start, paging) {
            cx.command(Command::RunAction(action));
            return Handled::Yes;
        }
        match self.list.hit(gesture.start, self.len()) {
            Some(index) => {
                self.list.focus = index;
                self.open_entry(index, cx);
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

    fn annotation(kind: Kind, start: u32, end: u32, text: &str) -> Annotation {
        Annotation {
            mark: Mark {
                kind,
                start: Locator::new(1, start),
                end: Locator::new(1, end),
                noted: false,
            },
            percent: 5,
            text: snippet(text),
            note: String::new(),
        }
    }

    #[test]
    fn kept_by_position_and_removed() {
        let storage = MemoryStorage::new();
        add(
            &storage,
            "b",
            annotation(Kind::Highlight, 50, 60, "zweite\tStelle"),
        );
        add(&storage, "b", annotation(Kind::Bookmark, 10, 10, "erste"));
        let all = load(&storage, "b");
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].mark.kind, Kind::Bookmark);
        assert_eq!(all[1].text, "zweite Stelle");
        assert!(
            all[1]
                .mark
                .overlaps(Locator::new(1, 55), Locator::new(1, 70))
        );
        assert!(
            !all[1]
                .mark
                .overlaps(Locator::new(1, 60), Locator::new(1, 70))
        );
        // Notes on highlights.
        assert!(set_note(&storage, "b", Locator::new(1, 50), "Why?\tReally"));
        assert_eq!(note_of(&storage, "b", Locator::new(1, 50)), "Why? Really");
        assert!(marks(&storage, "b")[1].noted);
        assert!(!set_note(
            &storage,
            "b",
            Locator::new(1, 10),
            "not a highlight"
        ));
        assert!(remove(&storage, "b", |m| m.kind == Kind::Bookmark));
        assert_eq!(marks(&storage, "b").len(), 1);
        assert!(!remove(&storage, "b", |m| m.kind == Kind::Bookmark));
    }

    #[test]
    fn snippets_are_short_single_lines() {
        assert_eq!(snippet("a\nb"), "a b");
        let long = "ø".repeat(200);
        assert!(snippet(&long).len() <= MAX_TEXT + '…'.len_utf8());
    }
}
