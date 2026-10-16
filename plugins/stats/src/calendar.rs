//! The calendar: a month, its days shaded by how long one read, and below
//! it the chosen day: its reading time, pages and books.
//!
//! Tapping a day chooses it, the arrows beside the month's name turn the
//! month. With buttons, left and right move by a day, up and down by a
//! week, page forward and back by a month.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::actions;
use quireink_sdk::ui::{self, UiStyle};
use quireink_sdk::{
    Canvas, Color, Command, Cx, Direction, Env, Gesture, GestureKind, Handled, Id, Point, Rect,
    RefreshMode, Screen,
};

use crate::store::{self, Book, Day};
use crate::{fmt, totals};

/// Most books listed for a day.
const DAY_BOOKS: usize = 3;

pub(crate) struct Calendar {
    style: UiStyle,
    header: ui::Header,
    /// The row with the month's name and its arrows.
    nav: Rect,
    grid: Rect,
    detail: Rect,
    days: Vec<Day>,
    books: Vec<Book>,
    today: i32,
    /// The chosen day.
    day: i32,
    /// Its books (titles and seconds) and distinct pages.
    day_books: Vec<(String, u32)>,
    day_pages: u32,
}

/// Year and month of a day.
fn month_of(day: i32) -> (i32, u8) {
    let d = fmt::date_of(day);
    (d.year, d.month)
}

/// The day the month of `day` begins on.
fn month_start(day: i32) -> i32 {
    day - i32::from(fmt::date_of(day).day) + 1
}

/// The first day of the month after the one of `day`.
fn next_month(day: i32) -> i32 {
    let start = month_start(day);
    // At most 31 days on, then back to that month's start.
    month_start(start + 31)
}

fn previous_month(day: i32) -> i32 {
    month_start(month_start(day) - 1)
}

impl Calendar {
    pub(crate) fn new(env: &Env<'_>) -> Self {
        let style = UiStyle::new(env);
        let header = ui::Header::new(env, &style, true);
        let display = env.device.display;
        let row = style.row as u16;
        let margin = style.margin as u16;
        let nav = Rect::new(0, header.area.height + style.gap as u16, display.width, row);
        let width = display.width - 2 * margin;
        // Square days across the width, unless that leaves too little room
        // for the chosen day below: the weekdays' names (half a day high)
        // and up to six weeks.
        let room = display.height.saturating_sub(nav.y + nav.height + row * 4);
        let cell = (width / 7).min(room * 2 / 13);
        let grid = Rect::new(
            (display.width - cell * 7) / 2,
            nav.y + nav.height,
            cell * 7,
            cell / 2 + cell * 6,
        );
        let detail_top = grid.y + grid.height + style.gap as u16;
        let detail = Rect::new(
            0,
            detail_top,
            display.width,
            display.height.saturating_sub(detail_top),
        );
        let today = fmt::today(env).unwrap_or(0);
        let days = store::days(env.storage, |unix| fmt::day_of(env, unix));
        let mut calendar = Self {
            style,
            header,
            nav,
            grid,
            detail,
            days,
            books: store::books(env.storage),
            today,
            day: today,
            day_books: Vec::new(),
            day_pages: 0,
        };
        calendar.load_day(env);
        calendar
    }

    fn seconds_on(&self, day: i32) -> u32 {
        self.days
            .binary_search_by_key(&day, |d| d.day)
            .map_or(0, |i| self.days[i].seconds)
    }

    /// What was read on the chosen day.
    fn load_day(&mut self, env: &Env<'_>) {
        let day = self.day;
        let since = fmt::unix_near(day).saturating_sub(14 * 3600).max(0) as u32;
        let mut per_book: Vec<(u16, u32)> = Vec::new();
        let mut pages: Vec<(u16, u32)> = Vec::new();
        if self.seconds_on(day) > 0 {
            store::scan_recent(env.storage, since, |stat| {
                if fmt::day_of(env, stat.start) != day {
                    return;
                }
                pages.push((stat.book, stat.page));
                match per_book.iter_mut().find(|(b, _)| *b == stat.book) {
                    Some((_, s)) => *s += u32::from(stat.duration),
                    None => per_book.push((stat.book, u32::from(stat.duration))),
                }
            });
        }
        pages.sort_unstable();
        pages.dedup();
        per_book.sort_by_key(|(_, s)| core::cmp::Reverse(*s));
        self.day_pages = pages.len() as u32;
        self.day_books = per_book
            .into_iter()
            .map(|(id, s)| {
                let title = self
                    .books
                    .iter()
                    .find(|b| b.id == id)
                    .map_or_else(|| "?".to_string(), |b| b.title.clone());
                (title, s)
            })
            .collect();
    }

    /// The first day shown: the start of the week the month begins in.
    fn first_shown(&self, env: &Env<'_>) -> i32 {
        let start = month_start(self.day);
        let weekday = i32::from(fmt::date_of(start).weekday);
        let from = i32::from(fmt::week_start(env));
        start - (weekday - from).rem_euclid(7)
    }

    fn cell(&self, index: i32) -> Rect {
        let size = self.grid.width / 7;
        // Below the weekdays' names, half a day high.
        let (row, col) = (index / 7, index % 7);
        Rect::new(
            self.grid.x + size * col as u16,
            self.grid.y + size / 2 + size * row as u16,
            size,
            size,
        )
    }

    /// The month's arrows.
    fn arrows(&self) -> (Rect, Rect) {
        let n = self.nav;
        let size = ui::button_height(n);
        let margin = self.style.margin as u16;
        let y = n.y + (n.height - size) / 2;
        (
            Rect::new(margin, y, size * 3 / 2, size),
            Rect::new(n.width - margin - size * 3 / 2, y, size * 3 / 2, size),
        )
    }

    fn choose(&mut self, day: i32, cx: &mut Cx<'_, '_>) {
        if day == self.day {
            return;
        }
        let whole = month_of(day) != month_of(self.day);
        self.day = day;
        let env = *cx.env();
        self.load_day(&env);
        if whole {
            cx.request_redraw();
            cx.request_refresh(RefreshMode::Partial);
        } else {
            cx.request_redraw_area(Rect::new(
                0,
                self.grid.y,
                self.grid.width + self.grid.x * 2,
                self.detail.y + self.detail.height - self.grid.y,
            ));
            cx.request_refresh(RefreshMode::Fast);
        }
    }

    fn draw_grid(&self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        let Some(font) = style.font(env) else {
            return;
        };
        let size = self.grid.width / 7;
        let from = fmt::week_start(env);
        for col in 0..7u8 {
            let name = fmt::weekday_short(env, (from + col) % 7);
            let area = Rect::new(
                self.grid.x + size * u16::from(col),
                self.grid.y,
                size,
                size / 2,
            );
            let x = f32::from(area.x) + (f32::from(size) - font.measure(style.small, name)) / 2.0;
            ui::draw_label(canvas, &font, style.small, x, area, name, style.faint(90));
        }
        let first = self.first_shown(env);
        let (year, month) = month_of(self.day);
        let pad = (style.line * 2).max(2) as u16;
        for index in 0..42 {
            let day = first + index;
            if month_of(day) != (year, month) {
                continue;
            }
            let r = self.cell(index);
            let inner = Rect::new(r.x + pad, r.y + pad, r.width - 2 * pad, r.height - 2 * pad);
            let minutes = self.seconds_on(day) / 60;
            // Darker the longer one read.
            let shade = match minutes {
                0 => None,
                1..=14 => Some(220),
                15..=29 => Some(180),
                30..=59 => Some(120),
                _ => Some(40),
            };
            if let Some(gray) = shade {
                ui::fill(canvas, inner, Color::gray(gray));
                // And as a bar, an hour long at most: visible on displays
                // without grays too.
                let full = u32::from(inner.width) * 3 / 4;
                let len = (full * minutes.min(60) / 60).max(style.line.max(2) * 2);
                let thick = (style.line * 2).max(2);
                let bar_ink = if gray < 150 {
                    Color::WHITE
                } else {
                    Color::BLACK
                };
                canvas.fill_rect(
                    i32::from(inner.x) + (i32::from(inner.width) - len as i32) / 2,
                    i32::from(inner.y + inner.height) - thick as i32 * 2,
                    len,
                    thick,
                    bar_ink,
                );
            }
            let ink = if shade.is_some_and(|g| g < 150) {
                Color::WHITE
            } else {
                Color::BLACK
            };
            let number = format!("{}", fmt::date_of(day).day);
            let x = f32::from(inner.x)
                + (f32::from(inner.width) - font.measure(style.small, &number)) / 2.0;
            // Above the bar of reading time, if there is one.
            let bar_room = if shade.is_some() {
                (style.line * 2).max(2) as u16 * 4
            } else {
                0
            };
            let text = Rect::new(
                inner.x,
                inner.y,
                inner.width,
                inner.height.saturating_sub(bar_room),
            );
            ui::draw_label(canvas, &font, style.small, x, text, &number, ink);
            let border = ui::button_border(&style);
            if day == self.day {
                canvas.stroke_rect(
                    i32::from(r.x),
                    i32::from(r.y),
                    u32::from(r.width),
                    u32::from(r.height),
                    border * 2,
                    Color::BLACK,
                );
            } else if day == self.today {
                canvas.stroke_rect(
                    i32::from(inner.x),
                    i32::from(inner.y),
                    u32::from(inner.width),
                    u32::from(inner.height),
                    border,
                    style.faint(90),
                );
            }
        }
    }

    fn draw_detail(&self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        let Some(font) = style.font(env) else {
            return;
        };
        let d = self.detail;
        let row = style.row as u16 * 3 / 4;
        let line = |n: u16| Rect::new(d.x, d.y + row * n, d.width, row);
        let margin = style.margin as f32;
        let seconds = self.seconds_on(self.day);
        let summary = if seconds == 0 {
            env.i18n.tr("stats.not-read").to_string()
        } else {
            format!(
                "{} · {}",
                fmt::duration(env, u64::from(seconds)),
                env.i18n
                    .plural("stats.pages", i64::from(self.day_pages), &[])
            )
        };
        let title = fmt::long_date(env, self.day);
        ui::draw_label(
            canvas,
            &font,
            style.text,
            margin,
            line(0),
            &title,
            Color::BLACK,
        );
        ui::draw_label(
            canvas,
            &font,
            style.small,
            margin,
            line(1),
            &summary,
            style.faint(60),
        );
        for (n, (book, s)) in self.day_books.iter().take(DAY_BOOKS).enumerate() {
            let value = fmt::duration(env, u64::from(*s));
            let width = f32::from(d.width) - 2.0 * margin;
            let (book, value, vw) = ui::fit_pair(&font, style.small, book, &value, width, margin);
            let r = line(n as u16 + 2);
            ui::draw_label(
                canvas,
                &font,
                style.small,
                margin,
                r,
                &book,
                style.faint(60),
            );
            ui::draw_label(
                canvas,
                &font,
                style.small,
                f32::from(d.width) - margin - vw,
                r,
                &value,
                style.faint(60),
            );
        }
    }
}

impl Screen for Calendar {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        let title = env.i18n.tr("stats.calendar");
        let streak = totals::streak(&self.days, self.today);
        let note =
            (streak > 1).then(|| env.i18n.plural("stats.streak-note", i64::from(streak), &[]));
        self.header
            .draw(canvas, env, &style, title, note.as_deref(), None);
        let (left, right) = self.arrows();
        ui::draw_button(canvas, env, &style, left, "‹", false, false);
        ui::draw_button(canvas, env, &style, right, "›", false, false);
        if let Some(font) = style.font(env) {
            let (year, month) = month_of(self.day);
            let name = format!("{} {year}", fmt::month(env, month));
            let x = (f32::from(self.nav.width) - font.measure(style.title, &name)) / 2.0;
            ui::draw_label(canvas, &font, style.title, x, self.nav, &name, Color::BLACK);
        }
        self.draw_grid(canvas, env);
        self.draw_detail(canvas, env);
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let a = action;
        let day = if *a == actions::LEFT || *a == actions::PREVIOUS {
            self.day - 1
        } else if *a == actions::RIGHT || *a == actions::NEXT {
            self.day + 1
        } else if *a == actions::UP {
            self.day - 7
        } else if *a == actions::DOWN {
            self.day + 7
        } else if *a == actions::PREV_PAGE {
            previous_month(self.day)
        } else if *a == actions::NEXT_PAGE {
            next_month(self.day)
        } else {
            return Handled::No;
        };
        self.choose(day, cx);
        Handled::Yes
    }

    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        let env = *cx.env();
        // A swipe turns the month, like a page.
        match gesture.kind {
            GestureKind::Swipe(Direction::Left) => return self.on_action(&actions::NEXT_PAGE, cx),
            GestureKind::Swipe(Direction::Right) => return self.on_action(&actions::PREV_PAGE, cx),
            _ => {}
        }
        if gesture.kind != GestureKind::Tap {
            return Handled::No;
        }
        let p: Point = gesture.start;
        if let Some(action) = self.header.tap(&env, &self.style, p, None) {
            cx.command(Command::RunAction(action));
            return Handled::Yes;
        }
        let (left, right) = self.arrows();
        if left.contains(p) {
            self.choose(previous_month(self.day), cx);
            return Handled::Yes;
        }
        if right.contains(p) {
            self.choose(next_month(self.day), cx);
            return Handled::Yes;
        }
        let first = self.first_shown(&env);
        let hit = (0..42).find(|&i| self.cell(i).contains(p));
        if let Some(index) = hit {
            let day = first + index;
            if month_of(day) == month_of(self.day) {
                self.choose(day, cx);
            }
            return Handled::Yes;
        }
        Handled::No
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The local day of a date.
    fn day(year: i32, month: u8, d: u8) -> i32 {
        (0..30_000)
            .find(|&n| {
                let t = fmt::date_of(n);
                (t.year, t.month, t.day) == (year, month, d)
            })
            .unwrap()
    }

    #[test]
    fn months_turn() {
        let d = day(2026, 1, 31);
        assert_eq!(month_start(d), day(2026, 1, 1));
        assert_eq!(next_month(d), day(2026, 2, 1));
        assert_eq!(next_month(day(2026, 12, 15)), day(2027, 1, 1));
        assert_eq!(previous_month(day(2026, 3, 31)), day(2026, 2, 1));
        assert_eq!(previous_month(day(2026, 1, 5)), day(2025, 12, 1));
    }
}
