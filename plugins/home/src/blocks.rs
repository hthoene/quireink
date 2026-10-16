//! The home screen's own widgets, which its pages are made of (any
//! plugin's widgets go on them too), and its tools:
//!
//! - status line: the time, and the battery's charge (above the grid by
//!   default, see `pages`);
//! - date: today in words;
//! - clock: the time large, with the date;
//! - continue reading: the book read last, with its cover and progress;
//! - recently read: the books before it, as a row of covers;
//! - tools: an action's icon and name, in a cell of the grid.
//!
//! Every widget has settings of its own (from the editor, *Settings* of
//! the widget): where its text sits, how large it is, and which of its
//! parts it shows. Widgets fill the cells they are given.
//!
//! Widgets draw what is stored (covers once looked up, see `covers`),
//! never opening books themselves, so drawing stays quick.

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use quireink_sdk::builtin::{actions, params, slots};
use quireink_sdk::ui::{self, UiStyle};
use quireink_sdk::widgets::{self, Align, WidgetSpec, WidgetTarget};
use quireink_sdk::{
    Arg, Canvas, Color, Command, Env, Id, Params, Rect, RegisterError, Registrar, SettingKind,
    SettingSpec, SettingValue, time,
};

use crate::pages::{CONTINUE, DATE, RECENT, STATUS};
use crate::{covers, recent};

/// The clock with the date, large.
pub const CLOCK: &str = "home.clock";
/// Whether the clock shows the date below the time.
const CLOCK_DATE: &str = "home.clock.date";
/// How large the clock is: one of [`SIZES`] or [`FIT`].
const CLOCK_SIZE: &str = "home.clock.size";
/// How large the date is: one of [`SIZES`].
const DATE_SIZE: &str = "home.date.size";
/// Text sizes a widget may choose.
const SIZES: [&str; 3] = ["home.size.small", "home.size.medium", "home.size.large"];
/// As large as the widget's cells allow.
const FIT: &str = "home.size.fit";
/// Whether the continue card shows the cover, the progress, its frame.
const CARD_COVER: &str = "home.continue.cover";
const CARD_PROGRESS: &str = "home.continue.progress";
const CARD_FRAME: &str = "home.continue.frame";
/// Whether the recent books show their heading and their titles, how many
/// there are at most.
const RECENT_HEADING: &str = "home.recent.heading";
const RECENT_TITLES: &str = "home.recent.titles";
const RECENT_COUNT: &str = "home.recent.count";
/// Whether tools show their names below their icons.
pub(crate) const TOOL_LABELS: &str = "home.tool-labels";
/// Recent books shown as covers at most.
const MAX_COVERS: usize = 8;

pub(crate) fn register(r: &mut dyn Registrar) -> Result<(), RegisterError> {
    let widget = |id: &'static str,
                  title: &'static str,
                  height: fn(&Env<'_>, u16) -> u16,
                  draw: fn(&mut Canvas<'_>, &Env<'_>, Rect)| WidgetSpec {
        id: Id::new_static(id),
        title: Cow::Borrowed(title),
        height: Box::new(height),
        draw: Box::new(draw),
        signature: None,
        targets: None,
    };
    let mut status = widget(STATUS, "home.widget.status", status_height, draw_status);
    status.signature = Some(Box::new(|env| {
        let battery = env.status.battery.map_or(0, |b| b.percent);
        format!("{}|{battery}", time::clock_text(env).unwrap_or_default())
    }));
    r.add_widget(status)?;
    let mut date = widget(DATE, "home.widget.date", date_height, draw_date);
    date.signature = Some(Box::new(date_text));
    r.add_widget(date)?;
    let mut card = widget(CONTINUE, "home.widget.continue", card_height, draw_card);
    card.targets = Some(Box::new(card_targets));
    r.add_widget(card)?;
    let mut shelf = widget(RECENT, "home.widget.recent", shelf_height, draw_shelf);
    shelf.targets = Some(Box::new(shelf_targets));
    r.add_widget(shelf)?;
    let mut clock = widget(CLOCK, "home.widget.clock", clock_height, draw_clock);
    clock.signature = Some(Box::new(|env| {
        format!(
            "{}|{}",
            time::clock_text(env).unwrap_or_default(),
            date_text(env)
        )
    }));
    r.add_widget(clock)?;

    // Each widget's settings, in its own group.
    for (id, title, align) in [
        (STATUS, "home.widget.status", Align::Left),
        (DATE, "home.widget.date", Align::Center),
        (CLOCK, "home.widget.clock", Align::Center),
        (CONTINUE, "home.widget.continue", Align::Left),
        (RECENT, "home.widget.recent", Align::Center),
    ] {
        let id = Id::new_static(id);
        r.add_setting_group(widgets::settings_group(&id, title))?;
        r.add_setting(widgets::align_setting(&id, align))?;
    }
    let choice = |id: &'static str,
                  title: &'static str,
                  group: &'static str,
                  values: &[&'static str],
                  default: &'static str| {
        SettingSpec::new(
            Id::new_static(id),
            title,
            SettingKind::Choice(values.iter().map(|v| Cow::Borrowed(*v)).collect()),
            SettingValue::Text(Cow::Borrowed(default)),
        )
        .group(Id::new_static(group))
    };
    let switch = |id: &'static str, title: &'static str, group: &'static str, default: bool| {
        SettingSpec::new(
            Id::new_static(id),
            title,
            SettingKind::Bool,
            SettingValue::Bool(default),
        )
        .group(Id::new_static(group))
    };
    let clock_sizes = [SIZES[0], SIZES[1], SIZES[2], FIT];
    r.add_setting(choice(
        CLOCK_SIZE,
        "home.setting.size",
        CLOCK,
        &clock_sizes,
        FIT,
    ))?;
    r.add_setting(switch(CLOCK_DATE, "home.setting.clock-date", CLOCK, true))?;
    r.add_setting(choice(
        DATE_SIZE,
        "home.setting.size",
        DATE,
        &SIZES,
        SIZES[1],
    ))?;
    r.add_setting(switch(
        CARD_COVER,
        "home.setting.card-cover",
        CONTINUE,
        true,
    ))?;
    r.add_setting(switch(
        CARD_PROGRESS,
        "home.setting.card-progress",
        CONTINUE,
        true,
    ))?;
    r.add_setting(switch(
        CARD_FRAME,
        "home.setting.card-frame",
        CONTINUE,
        true,
    ))?;
    r.add_setting(switch(
        RECENT_HEADING,
        "home.setting.recent-heading",
        RECENT,
        true,
    ))?;
    r.add_setting(switch(
        RECENT_TITLES,
        "home.setting.recent-titles",
        RECENT,
        true,
    ))?;
    r.add_setting(
        SettingSpec::new(
            Id::new_static(RECENT_COUNT),
            "home.setting.recent-count",
            SettingKind::Int {
                min: 1,
                max: MAX_COVERS as i64,
            },
            SettingValue::Int(6),
        )
        .group(Id::new_static(RECENT)),
    )?;
    Ok(())
}

fn line_height(env: &Env<'_>, size: f32) -> u16 {
    UiStyle::new(env)
        .font(env)
        .map_or(size as u16, |f| f.line_metrics(size).line_height as u16)
}

/// A text size of [`SIZES`] for a widget.
fn sized(style: &UiStyle, value: &str) -> f32 {
    match SIZES.iter().position(|s| *s == value) {
        Some(0) => style.text,
        Some(2) => style.title * 1.6,
        _ => style.title,
    }
}

/// `height` pixels placed in the middle of `area`, top to bottom.
fn centred(area: Rect, height: u16) -> u16 {
    area.y + area.height.saturating_sub(height) / 2
}

// ---- Status line --------------------------------------------------------

pub(crate) fn status_height(env: &Env<'_>, _: u16) -> u16 {
    let style = UiStyle::new(env);
    line_height(env, style.small) + style.gap as u16 / 2
}

pub(crate) fn draw_status(canvas: &mut Canvas<'_>, env: &Env<'_>, area: Rect) {
    let style = UiStyle::new(env);
    let Some(font) = style.font(env) else {
        return;
    };
    let ink = style.faint(60);
    let align = widgets::align(env, STATUS);
    if let Some(clock) = time::clock_text(env) {
        let x = align.x(area, font.measure(style.small, &clock));
        ui::draw_label(canvas, &font, style.small, x, area, &clock, ink);
    }
    if let Some(battery) = env.status.battery {
        let text = env.i18n.format(
            "home.battery",
            &[("percent", Arg::Int(i64::from(battery.percent)))],
        );
        let width = font.measure(style.small, &text);
        let icon = if battery.charging { "power" } else { "battery" };
        let size = style.small * 1.2;
        let icon_width = ui::icon_width(env, icon, size);
        let gap = style.gap as f32 / 3.0;
        // At the right, or at the left where the time is at the right.
        let start = if align == Align::Right {
            f32::from(area.x)
        } else {
            f32::from(area.x + area.width) - width - gap - icon_width
        };
        ui::draw_icon(canvas, env, icon, size, start, area, ink);
        let x = start + icon_width + gap;
        ui::draw_label(canvas, &font, style.small, x, area, &text, ink);
    }
}

// ---- Date ---------------------------------------------------------------

/// The date in words: "Thursday, 15 October".
pub(crate) fn date_text(env: &Env<'_>) -> String {
    let Some(now) = time::local_time(env) else {
        return String::new();
    };
    env.i18n.format(
        "home.date",
        &[
            (
                "weekday",
                Arg::Str(env.i18n.tr(&format!("home.weekday.{}", now.weekday))),
            ),
            ("day", Arg::Int(i64::from(now.day))),
            (
                "month",
                Arg::Str(env.i18n.tr(&format!("home.month.{}", now.month))),
            ),
        ],
    )
}

fn date_height(env: &Env<'_>, _: u16) -> u16 {
    if time::local_time(env).is_none() {
        return 0;
    }
    let style = UiStyle::new(env);
    line_height(env, sized(&style, env.settings.text(DATE_SIZE)))
}

fn draw_date(canvas: &mut Canvas<'_>, env: &Env<'_>, area: Rect) {
    let style = UiStyle::new(env);
    if let Some(font) = style.font(env) {
        let size = sized(&style, env.settings.text(DATE_SIZE));
        let date = ui::fit(&font, size, &date_text(env), f32::from(area.width));
        let x = widgets::align(env, DATE).x(area, font.measure(size, &date));
        ui::draw_label(canvas, &font, size, x, area, &date, Color::BLACK);
    }
}

// ---- Clock ----------------------------------------------------------------

/// The clock's size in `area`: as set, or as large as fits.
fn clock_size(env: &Env<'_>, style: &UiStyle, area: Option<Rect>) -> f32 {
    let value = env.settings.text(CLOCK_SIZE);
    if value != FIT {
        return sized(style, value) * 1.4;
    }
    let Some(area) = area else {
        return style.title * 2.2;
    };
    let Some(font) = style.font(env) else {
        return style.title * 2.2;
    };
    // The digits fill the height left by the date, and fit the width.
    let date = if env.settings.bool(CLOCK_DATE) {
        f32::from(line_height(env, style.small))
    } else {
        0.0
    };
    let by_height = (f32::from(area.height) - date) * 0.7;
    let sample = time::clock_text(env).unwrap_or_else(|| String::from("00:00"));
    let at_title = font.measure(style.title, &sample).max(1.0);
    let by_width = style.title * f32::from(area.width) * 0.95 / at_title;
    by_height.min(by_width).max(style.title)
}

fn clock_height(env: &Env<'_>, _: u16) -> u16 {
    let style = UiStyle::new(env);
    let date = if env.settings.bool(CLOCK_DATE) {
        line_height(env, style.small)
    } else {
        0
    };
    line_height(env, clock_size(env, &style, None)) + date + style.gap as u16
}

fn draw_clock(canvas: &mut Canvas<'_>, env: &Env<'_>, area: Rect) {
    let style = UiStyle::new(env);
    let Some(font) = style.font(env) else {
        return;
    };
    let size = clock_size(env, &style, Some(area));
    let big = line_height(env, size);
    let with_date = env.settings.bool(CLOCK_DATE);
    let small = if with_date {
        line_height(env, style.small)
    } else {
        0
    };
    let top = centred(area, big + small);
    let clock = time::clock_text(env).unwrap_or_else(|| String::from("--:--"));
    let align = widgets::align(env, CLOCK);
    ui::draw_label(
        canvas,
        &font,
        size,
        align.x(area, font.measure(size, &clock)),
        Rect::new(area.x, top, area.width, big),
        &clock,
        Color::BLACK,
    );
    if !with_date {
        return;
    }
    let date = date_text(env);
    ui::draw_label(
        canvas,
        &font,
        style.small,
        align.x(area, font.measure(style.small, &date)),
        Rect::new(area.x, top + big, area.width, small),
        &date,
        style.faint(80),
    );
}

// ---- Continue reading -----------------------------------------------------

fn card_height(env: &Env<'_>, _: u16) -> u16 {
    (UiStyle::new(env).row as f32 * 3.0) as u16
}

fn card_targets(env: &Env<'_>, area: Rect) -> Vec<WidgetTarget> {
    let command = match recent::load(env.storage).into_iter().next() {
        Some(book) => Command::OpenSlot(slots::READER, Params::new().with(params::PATH, book.path)),
        None => Command::RunAction(actions::LIBRARY),
    };
    alloc::vec![WidgetTarget { area, command }]
}

fn draw_card(canvas: &mut Canvas<'_>, env: &Env<'_>, r: Rect) {
    let style = UiStyle::new(env);
    let Some(font) = style.font(env) else {
        return;
    };
    let framed = env.settings.bool(CARD_FRAME);
    let line = if framed { style.line * 2 } else { 0 };
    if framed {
        ui::stroke_round(canvas, &style, r, line, Color::BLACK);
    }
    let Some(book) = recent::load(env.storage).into_iter().next() else {
        ui::draw_message(canvas, env, &style, r, env.i18n.tr("home.nothing-read"));
        return;
    };
    let gray = style.faint(90);
    // The cover on the left, where there is one.
    let inset = if framed {
        style.margin as u16 / 2 + line as u16
    } else {
        0
    };
    let cover = env
        .settings
        .bool(CARD_COVER)
        .then(|| covers::get(env.storage, &book.path).flatten())
        .flatten();
    let r = match cover {
        Some(cover) => {
            let (w, h) = ui::fit_image(&cover, r.width / 3, r.height.saturating_sub(2 * inset));
            let area = Rect::new(r.x + inset, r.y + (r.height - h) / 2, w, h);
            ui::draw_image(canvas, env.storage, &cover, area);
            let shift = inset + w;
            Rect::new(r.x + shift, r.y, r.width.saturating_sub(shift), r.height)
        }
        None => r,
    };
    let pad = style.margin as f32 / 2.0 + line as f32;
    let inner = f32::from(r.width) - 2.0 * pad;
    let progress = env.settings.bool(CARD_PROGRESS);
    // The lines of text, together in the middle.
    let label_h = line_height(env, style.small);
    // Long titles in the body size rather than cut short.
    let size = if font.measure(style.title, &book.title) <= inner {
        style.title
    } else {
        style.text
    };
    let title_h = line_height(env, size);
    let gap = style.gap as u16 / 2;
    let total = label_h + gap + title_h + if progress { gap + label_h } else { 0 };
    let mut y = centred(r, total);
    let align = widgets::align(env, CONTINUE);
    let x_of = |width: f32| {
        let inner_area = Rect::new(
            (f32::from(r.x) + pad) as u16,
            r.y,
            inner.max(0.0) as u16,
            r.height,
        );
        align.x(inner_area, width)
    };
    let label = env.i18n.tr("home.continue");
    let row = |y: u16, h: u16| Rect::new(r.x, y, r.width, h);
    ui::draw_label(
        canvas,
        &font,
        style.small,
        x_of(font.measure(style.small, label)),
        row(y, label_h),
        label,
        gray,
    );
    y += label_h + gap;
    let title = ui::fit(&font, size, &book.title, inner);
    ui::draw_label(
        canvas,
        &font,
        size,
        x_of(font.measure(size, &title)),
        row(y, title_h),
        &title,
        Color::BLACK,
    );
    y += title_h + gap;
    if !progress {
        return;
    }
    let percent = env.i18n.format(
        "home.percent",
        &[("percent", Arg::Int((book.progress + 0.5) as i64))],
    );
    let x = f32::from(r.x) + pad;
    ui::draw_label(
        canvas,
        &font,
        style.small,
        x,
        row(y, label_h),
        &percent,
        gray,
    );
    // The progress bar, beside it.
    let bar_h = (style.line * 4).max(3);
    let percent_w = font.measure(style.small, &percent) + style.gap as f32;
    let bx = (x + percent_w) as i32;
    let by = i32::from(y) + i32::from(label_h) / 2 - bar_h as i32 / 2;
    let w = (inner - percent_w).max(1.0) as u32;
    canvas.stroke_rect(bx, by, w, bar_h, style.line.max(1), Color::BLACK);
    let filled = (w as f32 * (book.progress / 100.0).clamp(0.0, 1.0)) as u32;
    canvas.fill_rect(bx, by, filled, bar_h, Color::BLACK);
}

// ---- Recently read ----------------------------------------------------------

/// The recent books after the current one, as many as set.
fn shelf_books(env: &Env<'_>) -> Vec<recent::Recent> {
    let count = env.settings.int(RECENT_COUNT).clamp(1, MAX_COVERS as i64) as usize;
    let mut books = recent::load(env.storage);
    books.retain(|b| env.storage.exists(&b.path));
    books.into_iter().skip(1).take(count).collect()
}

/// The covers a row holds at `width`, and how wide each one's place is.
fn shelf_columns(env: &Env<'_>, width: u16) -> (usize, u16) {
    let count = env.settings.int(RECENT_COUNT).clamp(1, MAX_COVERS as i64) as u32;
    let columns = (u32::from(width) / env.metrics.mm(18).max(1)).clamp(1, count);
    (columns as usize, width / columns as u16)
}

/// Heights of the heading and of a title below a cover (0 where not
/// shown).
fn shelf_labels(env: &Env<'_>) -> (u16, u16) {
    let style = UiStyle::new(env);
    let label = line_height(env, style.small);
    let heading = if env.settings.bool(RECENT_HEADING) {
        label + style.gap as u16 / 2
    } else {
        0
    };
    let title = if env.settings.bool(RECENT_TITLES) {
        label
    } else {
        0
    };
    (heading, title)
}

fn shelf_height(env: &Env<'_>, width: u16) -> u16 {
    if shelf_books(env).is_empty() {
        return 0;
    }
    let style = UiStyle::new(env);
    let (_, cell) = shelf_columns(env, width);
    let (heading, title) = shelf_labels(env);
    let cover = (cell.saturating_sub(style.gap as u16)) * 4 / 3;
    heading + cover + title
}

/// Where each cover goes (with its title below), with its book: as large
/// as the area allows, the row in the middle.
fn shelf_places(env: &Env<'_>, area: Rect) -> Vec<(Rect, recent::Recent)> {
    let style = UiStyle::new(env);
    let books = shelf_books(env);
    // Few books: larger covers, as many columns as books.
    let (fit, _) = shelf_columns(env, area.width);
    let columns = books.len().clamp(1, fit);
    let shown = columns;
    let (heading, title) = shelf_labels(env);
    let gap = style.gap as u16;
    let room = area.height.saturating_sub(heading + title);
    // Covers 3:4, as wide as the columns or as high as the room allows.
    let column = area.width / columns as u16;
    let w = column.saturating_sub(gap).min(room * 3 / 4);
    let h = w * 4 / 3;
    let row_w = column * shown as u16;
    let left = area.x + (area.width.saturating_sub(row_w)) / 2;
    let top = area.y + heading + room.saturating_sub(h) / 2;
    books
        .into_iter()
        .take(columns)
        .enumerate()
        .map(|(i, book)| {
            let x = left + i as u16 * column + (column - w) / 2;
            (Rect::new(x, top, w, h + title), book)
        })
        .collect()
}

fn shelf_targets(env: &Env<'_>, area: Rect) -> Vec<WidgetTarget> {
    shelf_places(env, area)
        .into_iter()
        .map(|(place, book)| WidgetTarget {
            area: place,
            command: Command::OpenSlot(slots::READER, Params::new().with(params::PATH, book.path)),
        })
        .collect()
}

fn draw_shelf(canvas: &mut Canvas<'_>, env: &Env<'_>, area: Rect) {
    let style = UiStyle::new(env);
    let Some(font) = style.font(env) else {
        return;
    };
    // Nothing read before the current book: nothing to show.
    if shelf_books(env).is_empty() {
        return;
    }
    let (heading, title_h) = shelf_labels(env);
    let gray = style.faint(90);
    let align = widgets::align(env, RECENT);
    if heading > 0 {
        let text = env.i18n.tr("home.recent");
        ui::draw_label(
            canvas,
            &font,
            style.small,
            align.x(area, font.measure(style.small, text)),
            Rect::new(area.x, area.y, area.width, line_height(env, style.small)),
            text,
            gray,
        );
    }
    for (place, book) in shelf_places(env, area) {
        let cover = Rect::new(place.x, place.y, place.width, place.height - title_h);
        match covers::get(env.storage, &book.path).flatten() {
            Some(image) => {
                let (w, h) = ui::fit_image(&image, cover.width, cover.height);
                let at = Rect::new(
                    cover.x + (cover.width - w) / 2,
                    cover.y + cover.height - h,
                    w,
                    h,
                );
                ui::draw_image(canvas, env.storage, &image, at);
            }
            None => draw_placeholder(canvas, env, &style, cover, &book.title),
        }
        if title_h == 0 {
            continue;
        }
        let title = ui::fit(&font, style.small, &book.title, f32::from(place.width));
        let row = Rect::new(place.x, cover.y + cover.height, place.width, title_h);
        let x = align.x(row, font.measure(style.small, &title));
        ui::draw_label(canvas, &font, style.small, x, row, &title, Color::BLACK);
    }
}

/// A book without a cover: a frame with its title.
fn draw_placeholder(
    canvas: &mut Canvas<'_>,
    env: &Env<'_>,
    style: &UiStyle,
    area: Rect,
    title: &str,
) {
    canvas.stroke_rect(
        i32::from(area.x),
        i32::from(area.y),
        u32::from(area.width),
        u32::from(area.height),
        style.line.max(2),
        Color::BLACK,
    );
    let Some(font) = style.font(env) else {
        return;
    };
    let pad = style.gap as f32 / 2.0;
    let width = f32::from(area.width) - 2.0 * pad;
    let line = font.line_metrics(style.small).line_height;
    let lines = font.wrap(style.small, title, width);
    let fits = ((f32::from(area.height) - 2.0 * pad) / line).max(1.0) as usize;
    for (n, range) in lines.into_iter().take(fits).enumerate() {
        let y = area.y + pad as u16 + (n as f32 * line) as u16;
        let row = Rect::new(area.x, y, area.width, line as u16);
        ui::draw_label(
            canvas,
            &font,
            style.small,
            f32::from(area.x) + pad,
            row,
            &title[range],
            Color::BLACK,
        );
    }
}

// ---- Tools ----------------------------------------------------------------

/// A tool in its cells: its icon, and its name below (where shown),
/// together in the middle.
pub(crate) fn draw_tool(canvas: &mut Canvas<'_>, env: &Env<'_>, area: Rect, action: &Id) {
    let style = UiStyle::new(env);
    let Some(font) = style.font(env) else {
        return;
    };
    let labels = env.settings.bool(TOOL_LABELS);
    let label = line_height(env, style.small);
    let gap = style.gap as u16 / 2;
    let text_room = if labels { label + gap } else { 0 };
    let size = (style.title * 1.6).min(f32::from(area.height.saturating_sub(text_room)) * 0.8);
    let icon_h = line_height(env, size).min(area.height);
    let top = centred(area, icon_h + text_room);
    let icon_row = Rect::new(area.x, top, area.width, icon_h);
    if let Some(icon) = env.menus.icon(action.as_str()) {
        let w = ui::icon_width(env, icon, size);
        let x = f32::from(area.x) + (f32::from(area.width) - w) / 2.0;
        ui::draw_icon(canvas, env, icon, size, x, icon_row, Color::BLACK);
    }
    if !labels {
        return;
    }
    let title = crate::grid::tools(env)
        .into_iter()
        .find(|(id, _)| id == action)
        .map(|(_, title)| title)
        .unwrap_or_default();
    let name = env
        .i18n
        .get(&format!("{title}.short"))
        .unwrap_or(env.i18n.tr(&title))
        .into();
    let name: String = name;
    let name = ui::fit(&font, style.small, &name, f32::from(area.width) - 4.0);
    let x = f32::from(area.x) + (f32::from(area.width) - font.measure(style.small, &name)) / 2.0;
    let row = Rect::new(
        area.x,
        icon_row.y + icon_row.height + gap,
        area.width,
        label,
    );
    ui::draw_label(canvas, &font, style.small, x, row, &name, Color::BLACK);
}

/// The covers worth looking up: the current book's and the recent ones'.
pub(crate) fn wanted_covers(env: &Env<'_>) -> Vec<String> {
    recent::load(env.storage)
        .into_iter()
        .take(MAX_COVERS + 1)
        .map(|b| b.path)
        .filter(|p| env.storage.exists(p))
        .collect()
}
