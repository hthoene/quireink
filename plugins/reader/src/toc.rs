//! The table of contents screen.
//!
//! Lists the document's chapters with the current one focused; choosing an
//! entry emits [`GOTO`](crate::GOTO), which the reader (or any other
//! plugin) handles, and closes the list. Only the visible page of entries
//! is held in memory, so long tables of contents work on small devices.

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::{actions, params};
use quireink_sdk::document::{Document, Locator, TocEntry};
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
    document: Option<Box<dyn Document>>,
    len: usize,
    /// Index of the first entry in `visible`.
    first: usize,
    /// The entries of the visible page.
    visible: Vec<TocEntry>,
    style: UiStyle,
    header: ui::Header,
    list: ListView,
}

impl TocScreen {
    pub(crate) fn open(env: &Env<'_>, params: &Params) -> Self {
        let path = params.get(params::PATH).unwrap_or_default().to_string();
        let document = env.formats.open(env.storage, &path).ok();
        let len = document
            .as_ref()
            .map_or(0, |d| d.toc_len(env.storage) as usize);
        let style = UiStyle::new(env);
        let display = env.device.display;
        let header = ui::Header::new(env, &style, true);
        let top = header.area.height + style.gap as u16;
        let area = Rect::new(0, top, display.width, display.height.saturating_sub(top));
        let mut screen = Self {
            path,
            document,
            len,
            first: 0,
            visible: Vec::new(),
            style,
            header,
            list: ListView::new(area, style.row).focus_on_keys(env),
        };
        if let Some(current) = locator_of(params) {
            screen.list.focus = screen.current(env, current);
        }
        screen.load_page(env);
        screen
    }

    /// The last entry at or before `at`. Entries are in reading order, so
    /// only those in `at`'s chapter need their exact position.
    fn current(&self, env: &Env<'_>, at: Locator) -> usize {
        let Some(document) = &self.document else {
            return 0;
        };
        let mut focus = 0;
        for index in 0..self.len {
            let Some(entry) = document.toc_entry(env.storage, index as u32) else {
                continue;
            };
            let before = match entry.at.section.cmp(&at.section) {
                core::cmp::Ordering::Less => true,
                core::cmp::Ordering::Equal => document
                    .toc_target(env.storage, index as u32)
                    .is_some_and(|target| target <= at),
                core::cmp::Ordering::Greater => false,
            };
            if !before {
                break;
            }
            focus = index;
        }
        focus
    }

    /// Reads the entries of the page the focus is on.
    fn load_page(&mut self, env: &Env<'_>) {
        let rows = self.list.rows();
        self.first = self.list.page() * rows;
        let end = (self.first + rows).min(self.len);
        self.visible.clear();
        if let Some(document) = &self.document {
            for index in self.first..end {
                let entry = document.toc_entry(env.storage, index as u32);
                self.visible.push(entry.unwrap_or(TocEntry {
                    title: String::from("?"),
                    level: 0,
                    at: Locator::START,
                }));
            }
        }
    }

    fn choose(&self, index: usize, cx: &mut Cx<'_, '_>) {
        let Some(document) = &self.document else {
            return;
        };
        if let Some(at) = document.toc_target(cx.env().storage, index as u32) {
            cx.command(Command::Back);
            cx.command(Command::Emit(GOTO, locator_params(&self.path, at)));
        }
    }

    fn moved(&mut self, change: ListChange, cx: &mut Cx<'_, '_>) {
        if change == ListChange::Page {
            let env = *cx.env();
            self.load_page(&env);
        }
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
        let len = self.len;
        self.header.draw(
            canvas,
            env,
            &style,
            env.i18n.tr("reader.toc"),
            None,
            self.list.paging(len),
        );
        if len == 0 {
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
        let (first, visible) = (self.first, &self.visible);
        self.list
            .draw(canvas, &style, len, |canvas, index, rect, focused| {
                let Some(entry) = index.checked_sub(first).and_then(|i| visible.get(i)) else {
                    return;
                };
                let color = if focused { Color::WHITE } else { Color::BLACK };
                let indent = style.margin as f32 * (1.0 + 2.0 * f32::from(entry.level.min(4)));
                let width = f32::from(rect.width) - indent - style.margin as f32;
                let title = ui::fit(&font, style.text, &entry.title, width);
                ui::draw_label(canvas, &font, style.text, indent, rect, &title, color);
            });
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let len = self.len;
        let rows = self.list.rows() as isize;
        let a = action;
        if *a == actions::UP || *a == actions::LEFT || *a == actions::PREVIOUS {
            let change = self.list.move_focus(-1, len);
            self.moved(change, cx);
        } else if *a == actions::DOWN || *a == actions::RIGHT || *a == actions::NEXT {
            let change = self.list.move_focus(1, len);
            self.moved(change, cx);
        } else if *a == actions::NEXT_PAGE {
            let change = self.list.move_focus(rows, len);
            self.moved(change, cx);
        } else if *a == actions::PREV_PAGE {
            let change = self.list.move_focus(-rows, len);
            self.moved(change, cx);
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
        let env = *cx.env();
        let paging = self.list.paging(self.len);
        if let Some(action) = self.header.tap(&env, &self.style, gesture.start, paging) {
            cx.command(Command::RunAction(action));
            return Handled::Yes;
        }
        match self.list.hit(gesture.start, self.len) {
            Some(index) => {
                self.choose(index, cx);
                Handled::Yes
            }
            None => Handled::No,
        }
    }
}
