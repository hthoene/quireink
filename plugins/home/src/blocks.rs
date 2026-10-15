//! The home screen's own widgets, which its pages are made of (any
//! plugin's widgets go on them too):
//!
//! - status line: the time, and the battery's charge;
//! - date: today in words;
//! - continue reading: the book read last, with its cover and progress;
//! - recently read: the books before it, as a row of covers;
//! - tools: the shortcuts (`home.actions`) and the tools (`core.tools`)
//!   as a grid of icons, then the settings;
//! - clock: the time large, with the date.
//!
//! Widgets draw what is stored (covers once looked up, see `covers`),
//! never opening books themselves, so drawing stays quick.

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::{actions, menus, params, slots};
use quireink_sdk::ui::{self, UiStyle};
use quireink_sdk::widgets::{WidgetSpec, WidgetTarget};
use quireink_sdk::{
    Arg, Canvas, Color, Command, Env, Id, Params, Rect, RegisterError, Registrar, time,
};

use crate::pages::{CONTINUE, DATE, RECENT, STATUS, TOOLS};
use crate::{ACTIONS, covers, recent};

/// The clock with the date, large.
pub const CLOCK: &str = "home.clock";
/// The clock's digits are this much larger than titles.
const CLOCK_SCALE: f32 = 2.2;
/// Recent books shown as covers at most.
const MAX_COVERS: usize = 6;

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
    let mut tools = widget(TOOLS, "home.widget.tools", tools_height, draw_tools);
    tools.targets = Some(Box::new(tools_targets));
    r.add_widget(tools)?;
    let mut clock = widget(CLOCK, "home.widget.clock", clock_height, draw_clock);
    clock.signature = Some(Box::new(|env| {
        format!(
            "{}|{}",
            time::clock_text(env).unwrap_or_default(),
            date_text(env)
        )
    }));
    r.add_widget(clock)?;
    Ok(())
}

fn line_height(env: &Env<'_>, size: f32) -> u16 {
    UiStyle::new(env)
        .font(env)
        .map_or(size as u16, |f| f.line_metrics(size).line_height as u16)
}

// ---- Status line --------------------------------------------------------

fn status_height(env: &Env<'_>, _: u16) -> u16 {
    let style = UiStyle::new(env);
    line_height(env, style.small) + style.gap as u16 / 2
}

fn draw_status(canvas: &mut Canvas<'_>, env: &Env<'_>, area: Rect) {
    let style = UiStyle::new(env);
    let Some(font) = style.font(env) else {
        return;
    };
    let ink = Color::gray(60);
    if let Some(clock) = time::clock_text(env) {
        ui::draw_label(
            canvas,
            &font,
            style.small,
            f32::from(area.x),
            area,
            &clock,
            ink,
        );
    }
    if let Some(battery) = env.status.battery {
        let text = env.i18n.format(
            "home.battery",
            &[("percent", Arg::Int(i64::from(battery.percent)))],
        );
        let right = f32::from(area.x + area.width);
        let width = font.measure(style.small, &text);
        ui::draw_label(canvas, &font, style.small, right - width, area, &text, ink);
        let icon = if battery.charging { "power" } else { "battery" };
        let size = style.small * 1.2;
        let icon_width = ui::icon_width(env, icon, size);
        let gap = style.gap as f32 / 3.0;
        ui::draw_icon(
            canvas,
            env,
            icon,
            size,
            right - width - gap - icon_width,
            area,
            ink,
        );
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
    line_height(env, UiStyle::new(env).text)
}

fn draw_date(canvas: &mut Canvas<'_>, env: &Env<'_>, area: Rect) {
    let style = UiStyle::new(env);
    if let Some(font) = style.font(env) {
        let date = date_text(env);
        ui::draw_label(
            canvas,
            &font,
            style.text,
            f32::from(area.x),
            area,
            &date,
            Color::BLACK,
        );
    }
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
    let line = style.line * 2;
    canvas.stroke_rect(
        i32::from(r.x),
        i32::from(r.y),
        u32::from(r.width),
        u32::from(r.height),
        line,
        Color::BLACK,
    );
    let Some(book) = recent::load(env.storage).into_iter().next() else {
        ui::draw_message(canvas, env, &style, r, env.i18n.tr("home.nothing-read"));
        return;
    };
    let gray = Color::gray(90);
    // The cover on the left, where there is one.
    let inset = style.margin as u16 / 2 + line as u16;
    let r = match covers::get(env.storage, &book.path).flatten() {
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
    let x = f32::from(r.x) + pad;
    let rows = r.height / 4;
    let row = |i: u16| Rect::new(r.x, r.y + rows / 2 + i * rows, r.width, rows);
    ui::draw_label(
        canvas,
        &font,
        style.small,
        x,
        row(0),
        env.i18n.tr("home.continue"),
        gray,
    );
    // Long titles in the body size rather than cut short.
    let size = if font.measure(style.title, &book.title) <= inner {
        style.title
    } else {
        style.text
    };
    let title = ui::fit(&font, size, &book.title, inner);
    ui::draw_label(canvas, &font, size, x, row(1), &title, Color::BLACK);
    let percent = env.i18n.format(
        "home.percent",
        &[("percent", Arg::Int((book.progress + 0.5) as i64))],
    );
    ui::draw_label(canvas, &font, style.small, x, row(2), &percent, gray);
    // The progress bar, below.
    let bar_h = (style.line * 4).max(3);
    let bar = row(2);
    let percent_w = font.measure(style.small, &percent) + style.gap as f32;
    let bx = (x + percent_w) as i32;
    let by = i32::from(bar.y) + i32::from(bar.height) / 2 - bar_h as i32 / 2;
    let w = (inner - percent_w).max(1.0) as u32;
    canvas.stroke_rect(bx, by, w, bar_h, style.line.max(1), Color::BLACK);
    let filled = (w as f32 * (book.progress / 100.0).clamp(0.0, 1.0)) as u32;
    canvas.fill_rect(bx, by, filled, bar_h, Color::BLACK);
}

// ---- Recently read ----------------------------------------------------------

/// The recent books after the current one.
fn shelf_books(env: &Env<'_>) -> Vec<recent::Recent> {
    let mut books = recent::load(env.storage);
    books.retain(|b| env.storage.exists(&b.path));
    books.into_iter().skip(1).take(MAX_COVERS).collect()
}

/// The covers a row holds at `width`, and how wide each one's place is.
fn shelf_columns(env: &Env<'_>, width: u16) -> (usize, u16) {
    let columns = (u32::from(width) / env.metrics.mm(22).max(1)).clamp(3, MAX_COVERS as u32);
    (columns as usize, width / columns as u16)
}

fn shelf_height(env: &Env<'_>, width: u16) -> u16 {
    if shelf_books(env).is_empty() {
        return 0;
    }
    let style = UiStyle::new(env);
    let (_, cell) = shelf_columns(env, width);
    let label = line_height(env, style.small);
    let cover = (cell.saturating_sub(style.gap as u16)) * 4 / 3;
    label + style.gap as u16 / 2 + cover + label
}

/// Where each cover goes, with its book.
fn shelf_places(env: &Env<'_>, area: Rect) -> Vec<(Rect, recent::Recent)> {
    let style = UiStyle::new(env);
    let (columns, cell) = shelf_columns(env, area.width);
    let label = line_height(env, style.small);
    let gap = style.gap as u16;
    let top = area.y + label + gap / 2;
    shelf_books(env)
        .into_iter()
        .take(columns)
        .enumerate()
        .map(|(i, book)| {
            let x = area.x + i as u16 * cell;
            let w = cell.saturating_sub(gap);
            (Rect::new(x, top, w, w * 4 / 3 + label), book)
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
    let label = line_height(env, style.small);
    let gray = Color::gray(90);
    ui::draw_label(
        canvas,
        &font,
        style.small,
        f32::from(area.x),
        Rect::new(area.x, area.y, area.width, label),
        env.i18n.tr("home.recent"),
        gray,
    );
    for (place, book) in shelf_places(env, area) {
        let cover = Rect::new(place.x, place.y, place.width, place.height - label);
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
        let title = ui::fit(&font, style.small, &book.title, f32::from(place.width));
        let row = Rect::new(place.x, cover.y + cover.height, place.width, label);
        ui::draw_label(
            canvas,
            &font,
            style.small,
            f32::from(place.x),
            row,
            &title,
            Color::BLACK,
        );
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

/// The grid's entries: the shortcuts, the tools, then the settings.
fn tool_items(env: &Env<'_>) -> Vec<(Id, String)> {
    let mut items: Vec<(Id, String)> = Vec::new();
    for menu in [ACTIONS, menus::TOOLS] {
        for item in env.menus.items(menu.as_str()) {
            if !item.section && !items.iter().any(|(id, _)| id == item.id) {
                items.push((item.id.clone(), item.title.to_string()));
            }
        }
    }
    if !items.iter().any(|(id, _)| *id == actions::SETTINGS) {
        items.push((actions::SETTINGS, String::from("core.action.settings")));
    }
    items
}

/// Columns of the grid at `width`, and the height of a row.
fn tool_grid(env: &Env<'_>, width: u16) -> (usize, u16) {
    let style = UiStyle::new(env);
    let columns = (u32::from(width) / env.metrics.mm(26).max(1)).clamp(3, 5) as usize;
    let icon = (style.title * 1.6) as u16;
    let row = icon + line_height(env, style.small) + style.gap as u16 * 2;
    (columns, row)
}

fn tools_height(env: &Env<'_>, width: u16) -> u16 {
    let (columns, row) = tool_grid(env, width);
    let count = tool_items(env).len();
    count.div_ceil(columns) as u16 * row
}

fn tool_places(env: &Env<'_>, area: Rect) -> Vec<(Rect, Id, String)> {
    let (columns, row) = tool_grid(env, area.width);
    let cell = area.width / columns as u16;
    tool_items(env)
        .into_iter()
        .enumerate()
        .map(|(i, (id, title))| {
            let x = area.x + (i % columns) as u16 * cell;
            let y = area.y + (i / columns) as u16 * row;
            (Rect::new(x, y, cell, row), id, title)
        })
        .collect()
}

fn tools_targets(env: &Env<'_>, area: Rect) -> Vec<WidgetTarget> {
    tool_places(env, area)
        .into_iter()
        .map(|(place, id, _)| WidgetTarget {
            area: place,
            command: Command::RunAction(id),
        })
        .collect()
}

fn draw_tools(canvas: &mut Canvas<'_>, env: &Env<'_>, area: Rect) {
    let style = UiStyle::new(env);
    let Some(font) = style.font(env) else {
        return;
    };
    let size = style.title * 1.6;
    let label = line_height(env, style.small);
    for (place, id, title) in tool_places(env, area) {
        let gap = style.gap as u16;
        let icon_row = Rect::new(place.x, place.y + gap, place.width, size as u16);
        if let Some(icon) = env.menus.icon(id.as_str()) {
            let w = ui::icon_width(env, icon, size);
            let x = f32::from(place.x) + (f32::from(place.width) - w) / 2.0;
            ui::draw_icon(canvas, env, icon, size, x, icon_row, Color::BLACK);
        }
        let name = env
            .i18n
            .get(&format!("{title}.short"))
            .unwrap_or(env.i18n.tr(&title))
            .to_string();
        let name = ui::fit(&font, style.small, &name, f32::from(place.width) - 4.0);
        let x =
            f32::from(place.x) + (f32::from(place.width) - font.measure(style.small, &name)) / 2.0;
        let row = Rect::new(
            place.x,
            icon_row.y + icon_row.height + gap / 2,
            place.width,
            label,
        );
        ui::draw_label(canvas, &font, style.small, x, row, &name, Color::BLACK);
    }
}

// ---- Clock ----------------------------------------------------------------

fn clock_height(env: &Env<'_>, _: u16) -> u16 {
    let style = UiStyle::new(env);
    line_height(env, style.title * CLOCK_SCALE) + line_height(env, style.small) + style.gap as u16
}

fn draw_clock(canvas: &mut Canvas<'_>, env: &Env<'_>, area: Rect) {
    let style = UiStyle::new(env);
    let Some(font) = style.font(env) else {
        return;
    };
    let size = style.title * CLOCK_SCALE;
    let big = line_height(env, size);
    let clock = time::clock_text(env).unwrap_or_else(|| String::from("--:--"));
    let x = f32::from(area.x);
    ui::draw_label(
        canvas,
        &font,
        size,
        x,
        Rect::new(area.x, area.y, area.width, big),
        &clock,
        Color::BLACK,
    );
    let small = line_height(env, style.small);
    ui::draw_label(
        canvas,
        &font,
        style.small,
        x,
        Rect::new(area.x, area.y + big, area.width, small),
        &date_text(env),
        Color::gray(80),
    );
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
