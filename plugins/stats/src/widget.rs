//! The widget "Read today" for the home screen (or any screen showing
//! widgets): today's reading time and pages, and the time this week.

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;

use quireink_sdk::ui::{self, UiStyle};
use quireink_sdk::widgets::WidgetSpec;
use quireink_sdk::{Arg, Canvas, Color, Env, Id, Rect, RegisterError, Registrar};

use crate::{fmt, overview, store};

pub(crate) const TODAY: Id = Id::new_static("stats.widget.today");

/// Today's seconds and pages, and this week's seconds.
fn numbers(env: &Env<'_>) -> Option<(u64, u32, u64)> {
    let today = fmt::today(env)?;
    let days = store::days(env.storage, |unix| fmt::day_of(env, unix));
    let weekday = fmt::date_of(today).weekday;
    let since = today - i32::from((7 + weekday - fmt::week_start(env)) % 7);
    let seconds = |range: core::ops::RangeInclusive<i32>| -> u64 {
        days.iter()
            .filter(|d| range.contains(&d.day))
            .map(|d| u64::from(d.seconds))
            .sum()
    };
    Some((
        seconds(today..=today),
        overview::distinct_pages(env, today),
        seconds(since..=today),
    ))
}

/// "23 min · 41 pages", and "This week: 2 h 10 min".
fn lines(env: &Env<'_>) -> (String, String) {
    let Some((today, pages, week)) = numbers(env) else {
        return (String::new(), String::new());
    };
    let pages = env.i18n.plural("stats.pages", i64::from(pages), &[]);
    (
        format!("{} · {pages}", fmt::duration(env, today)),
        env.i18n.format(
            "stats.widget.week",
            &[("time", Arg::Str(&fmt::duration(env, week)))],
        ),
    )
}

pub(crate) fn register(r: &mut dyn Registrar) -> Result<(), RegisterError> {
    r.add_widget(WidgetSpec {
        id: TODAY,
        title: Cow::Borrowed("stats.widget.today"),
        height: Box::new(|env, _| {
            let style = UiStyle::new(env);
            let Some(font) = style.font(env) else {
                return 0;
            };
            let small = font.line_metrics(style.small).line_height;
            let text = font.line_metrics(style.title).line_height;
            (small * 2.0 + text) as u16
        }),
        draw: Box::new(draw),
        signature: Some(Box::new(|env| {
            let (a, b) = lines(env);
            format!("{a}|{b}")
        })),
    })
}

fn draw(canvas: &mut Canvas<'_>, env: &Env<'_>, area: Rect) {
    let style = UiStyle::new(env);
    let Some(font) = style.font(env) else {
        return;
    };
    let small = font.line_metrics(style.small).line_height as u16;
    let text = font.line_metrics(style.title).line_height as u16;
    let x = f32::from(area.x);
    let (today, week) = lines(env);
    let row = |y: u16, h: u16| Rect::new(area.x, y, area.width, h);
    let gray = Color::gray(80);
    ui::draw_label(
        canvas,
        &font,
        style.small,
        x,
        row(area.y, small),
        env.i18n.tr("stats.today"),
        gray,
    );
    ui::draw_label(
        canvas,
        &font,
        style.title,
        x,
        row(area.y + small, text),
        &today,
        Color::BLACK,
    );
    ui::draw_label(
        canvas,
        &font,
        style.small,
        x,
        row(area.y + small + text, small),
        &week,
        gray,
    );
}
