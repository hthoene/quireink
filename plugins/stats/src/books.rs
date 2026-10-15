//! Every book with reading time, the last one read first.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use quireink_sdk::builtin::{actions, params};
use quireink_sdk::ui::{self, ListChange, ListView, UiStyle};
use quireink_sdk::{
    Arg, Canvas, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Params, Rect, RefreshMode,
    Screen,
};

use crate::store::{self, Book};
use crate::{BOOK, fmt, totals};

pub(crate) struct BookList {
    style: UiStyle,
    header: ui::Header,
    list: ListView,
    /// Books and their reading time.
    books: Vec<(Book, u64)>,
}

impl BookList {
    pub(crate) fn new(env: &Env<'_>) -> Self {
        let style = UiStyle::new(env);
        let header = ui::Header::new(env, &style, true);
        let display = env.device.display;
        let top = header.area.height + style.gap as u16;
        let area = Rect::new(0, top, display.width, display.height - top);
        let seconds = totals::seconds_per_book(env.storage);
        let mut books: Vec<(Book, u64)> = store::books(env.storage)
            .into_iter()
            .map(|b| {
                let s = seconds.get(usize::from(b.id)).copied().unwrap_or(0);
                (b, s)
            })
            .filter(|(_, s)| *s > 0)
            .collect();
        books.sort_by_key(|(b, _)| core::cmp::Reverse(b.last_open));
        Self {
            list: ListView::new(area, style.noted_row()).focus_on_keys(env),
            style,
            header,
            books,
        }
    }

    fn open(&self, index: usize, cx: &mut Cx<'_, '_>) {
        if let Some((book, _)) = self.books.get(index) {
            let params = Params::new().with(params::ID, format!("{}", book.id));
            cx.command(Command::OpenScreen(BOOK, params));
        }
    }
}

impl Screen for BookList {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        let len = self.books.len();
        let title = env.i18n.tr("stats.books");
        self.header
            .draw(canvas, env, &style, title, None, self.list.paging(len));
        if len == 0 {
            ui::draw_message(
                canvas,
                env,
                &style,
                self.list.area,
                env.i18n.tr("stats.no-books"),
            );
            return;
        }
        let books = &self.books;
        self.list
            .draw(canvas, &style, len, |canvas, index, rect, focused| {
                let (book, seconds) = &books[index];
                let mut note = authors(&book.authors);
                if book.last_open > 0 {
                    let day = fmt::day_of(env, book.last_open as u32);
                    let last = env.i18n.format(
                        "stats.last-read",
                        &[("date", Arg::Str(&fmt::date(env, day)))],
                    );
                    if !note.is_empty() {
                        note.push_str(" · ");
                    }
                    note.push_str(&last);
                }
                let value = fmt::duration(env, *seconds);
                ui::draw_noted_row(
                    canvas,
                    env,
                    &style,
                    rect,
                    focused,
                    &book.title,
                    &value,
                    &note,
                );
            });
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let len = self.books.len();
        let page = self.list.rows() as isize;
        let a = action;
        let delta = if [actions::UP, actions::LEFT, actions::PREVIOUS].contains(a) {
            -1
        } else if [actions::DOWN, actions::RIGHT, actions::NEXT].contains(a) {
            1
        } else if *a == actions::NEXT_PAGE {
            page
        } else if *a == actions::PREV_PAGE {
            -page
        } else if *a == actions::CONFIRM {
            self.open(self.list.focus, cx);
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
        let paging = self.list.paging(self.books.len());
        if let Some(action) = self
            .header
            .tap(cx.env(), &self.style, gesture.start, paging)
        {
            cx.command(Command::RunAction(action));
            return Handled::Yes;
        }
        let Some(index) = self.list.hit(gesture.start, self.books.len()) else {
            return Handled::No;
        };
        self.list.focus = index;
        self.open(index, cx);
        Handled::Yes
    }
}

/// Authors as shown: on one line, nothing for KOReader's `N/A`.
pub(crate) fn authors(authors: &str) -> String {
    if authors == "N/A" {
        return String::new();
    }
    authors.split('\n').collect::<Vec<_>>().join(", ")
}
