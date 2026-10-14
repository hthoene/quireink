//! The table of contents screen.
//!
//! Lists the document's chapters with the current one focused; choosing an
//! entry emits [`GOTO`](crate::GOTO), which the reader (or any other
//! plugin) handles, and closes the list.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::{actions, params};
use quireink_sdk::document::{Locator, TocEntry};
use quireink_sdk::ui::{self, ListChange, ListView, UiStyle};
use quireink_sdk::{
    Canvas, Color, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Params, Rect, RefreshMode,
    Screen,
};

use crate::GOTO;

/// Parameters carrying a position.
pub(crate) const SECTION: &str = "section";
pub(crate) const OFFSET: &str = "offset";

pub(crate) fn locator_params(path: &str, at: Locator) -> Params {
    Params::new()
        .with(params::PATH, path.to_string())
        .with(SECTION, alloc::format!("{}", at.section))
        .with(OFFSET, alloc::format!("{}", at.offset))
}

pub(crate) fn locator_of(params: &Params) -> Option<Locator> {
    Some(Locator::new(
        params.get(SECTION)?.parse().ok()?,
        params.get(OFFSET)?.parse().ok()?,
    ))
}

pub(crate) struct TocScreen {
    path: String,
    entries: Vec<TocEntry>,
    style: UiStyle,
    header: Rect,
    list: ListView,
}

impl TocScreen {
    pub(crate) fn open(env: &Env<'_>, params: &Params) -> Self {
        let path = params.get(params::PATH).unwrap_or_default().to_string();
        let entries = env
            .formats
            .open(env.storage, &path)
            .map(|document| document.toc(env.storage))
            .unwrap_or_default();
        let style = UiStyle::new(env);
        let display = env.device.display;
        let header = Rect::new(0, 0, display.width, style.header as u16);
        let top = header.height + style.gap as u16;
        let area = Rect::new(0, top, display.width, display.height.saturating_sub(top));
        let mut list = ListView::new(area, style.row);
        if let Some(current) = locator_of(params) {
            list.focus = entries.iter().rposition(|e| e.at <= current).unwrap_or(0);
        }
        Self {
            path,
            entries,
            style,
            header,
            list,
        }
    }

    fn choose(&self, index: usize, cx: &mut Cx<'_, '_>) {
        if let Some(entry) = self.entries.get(index) {
            cx.command(Command::Back);
            cx.command(Command::Emit(GOTO, locator_params(&self.path, entry.at)));
        }
    }

    fn moved(change: ListChange, cx: &mut Cx<'_, '_>) {
        if change != ListChange::None {
            cx.request_redraw();
            cx.request_refresh(if change == ListChange::Page {
                RefreshMode::Partial
            } else {
                RefreshMode::Fast
            });
        }
    }
}

impl Screen for TocScreen {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        let len = self.entries.len();
        let note = (self.list.pages(len) > 1)
            .then(|| alloc::format!("{} / {}", self.list.page() + 1, self.list.pages(len)));
        ui::draw_header(
            canvas,
            env,
            &style,
            self.header,
            env.i18n.tr("reader.toc"),
            note.as_deref(),
        );
        if self.entries.is_empty() {
            ui::draw_message(
                canvas,
                env,
                &style,
                self.list.area,
                env.i18n.tr("reader.no-toc"),
            );
            return;
        }
        let Some(font) = style.font(env) else {
            return;
        };
        let entries = &self.entries;
        self.list
            .draw(canvas, &style, len, |canvas, index, rect, focused| {
                let entry = &entries[index];
                let color = if focused { Color::WHITE } else { Color::BLACK };
                let indent = style.margin as f32 * (1.0 + 2.0 * f32::from(entry.level.min(4)));
                let width = f32::from(rect.width) - indent - style.margin as f32;
                let title = ui::fit(&font, style.text, &entry.title, width);
                ui::draw_label(canvas, &font, style.text, indent, rect, &title, color);
            });
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let len = self.entries.len();
        let rows = self.list.rows() as isize;
        let a = action;
        if *a == actions::UP || *a == actions::LEFT || *a == actions::PREVIOUS {
            Self::moved(self.list.move_focus(-1, len), cx);
        } else if *a == actions::DOWN || *a == actions::RIGHT || *a == actions::NEXT {
            Self::moved(self.list.move_focus(1, len), cx);
        } else if *a == actions::NEXT_PAGE {
            Self::moved(self.list.move_focus(rows, len), cx);
        } else if *a == actions::PREV_PAGE {
            Self::moved(self.list.move_focus(-rows, len), cx);
        } else if *a == actions::CONFIRM {
            self.choose(self.list.focus, cx);
        } else {
            return Handled::No;
        }
        Handled::Yes
    }

    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        if gesture.kind != GestureKind::Tap {
            return Handled::No;
        }
        match self.list.hit(gesture.start, self.entries.len()) {
            Some(index) => {
                self.choose(index, cx);
                Handled::Yes
            }
            None => Handled::No,
        }
    }
}
