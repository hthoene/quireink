//! The screen that lists the bookmarks and highlights of a book (kept by
//! [`quireink_sdk::annotations`]). The list ends with *Export as
//! Markdown*: a file in `Notes` beside the library folder, then the event
//! [`EXPORTED`] for plugins that take it further (a notes app, a sync).
//! While reading, only the positions are held in memory.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

pub(crate) use quireink_sdk::annotations::{
    Annotation, Kind, Mark, add, changed, load, marks, note_of, remove, set_note, snippet,
};
use quireink_sdk::builtin::{actions, params};
use quireink_sdk::ui::{self, ListChange, ListView, UiStyle};
use quireink_sdk::{
    Canvas, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Params, Rect, RefreshMode, Screen,
};

use crate::GOTO;
use crate::toc::locator_params;

/// Emitted after the notes of a book were exported: parameters `path`
/// (the Markdown file) and `title` (the book's). Plugins read the file
/// (e.g. `readwise`), so its form (see [`markdown`]) stays as it is.
pub const EXPORTED: Id = Id::new_static("reader.notes-exported");
/// Where exported notes go, beside the library folder.
const NOTES: &str = "Notes";

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
        let annotations = load(env.storage, &path);
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
