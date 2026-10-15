//! Bookmarks and highlights of a book, and the screen that lists them.
//!
//! They are kept in `annotations.txt` in the book's data folder, one per
//! line, sorted by position: kind, start and end (section and offset in
//! the source, so they outlast font sizes and layouts), the reading
//! progress and a snippet of the text. While reading, only the positions
//! are held in memory.

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
    let mut fields = line.splitn(7, '\t');
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
    Some(Annotation {
        mark: Mark { kind, start, end },
        percent,
        text,
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
            "{kind}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            a.mark.start.section,
            a.mark.start.offset,
            a.mark.end.section,
            a.mark.end.offset,
            a.percent,
            a.text
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

    fn open_entry(&self, index: usize, cx: &mut Cx<'_, '_>) {
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
        let len = self.annotations.len();
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
                let a = &annotations[index];
                let (kind, title) = match a.mark.kind {
                    Kind::Bookmark => ("reader.bookmark", a.text.clone()),
                    Kind::Highlight => ("reader.highlight", format!("„{}“", a.text)),
                };
                let note = format!("{} · {} %", env.i18n.tr(kind), a.percent);
                ui::draw_noted_row(canvas, env, &style, rect, focused, &title, "", &note);
            });
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let len = self.annotations.len();
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
        let paging = self.list.paging(self.annotations.len());
        if let Some(action) = self.header.tap(&env, &self.style, gesture.start, paging) {
            cx.command(Command::RunAction(action));
            return Handled::Yes;
        }
        match self.list.hit(gesture.start, self.annotations.len()) {
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
            },
            percent: 5,
            text: snippet(text),
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
