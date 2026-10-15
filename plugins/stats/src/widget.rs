//! The widget "Read today" for the home screen (or any screen showing
//! widgets): today's reading time and pages, and the time this week; with
//! a daily goal ([`GOAL`]), how far today is towards it and for how many
//! days in a row it was reached.

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::string::ToString;

use quireink_sdk::ui::{self, UiStyle};
use quireink_sdk::widgets::WidgetSpec;
use quireink_sdk::{
    Arg, Canvas, Color, Env, Id, Rect, RegisterError, Registrar, SettingKind, SettingSpec,
    SettingValue,
};

use crate::{fmt, overview, store};

pub(crate) const TODAY: Id = Id::new_static("stats.widget.today");
/// Setting: minutes a day to read, or none.
pub(crate) const GOAL: Id = Id::new_static("stats.goal");
/// Choices of [`GOAL`]: minutes, `off` for none.
const GOALS: [&str; 12] = [
    "stats.goal.off",
    "stats.goal.10",
    "stats.goal.15",
    "stats.goal.20",
    "stats.goal.30",
    "stats.goal.45",
    "stats.goal.60",
    "stats.goal.90",
    "stats.goal.120",
    "stats.goal.180",
    "stats.goal.240",
    "stats.goal.300",
];

/// The daily goal in seconds, if one is set.
fn goal(env: &Env<'_>) -> Option<u64> {
    env.settings
        .text(GOAL.as_str())
        .rsplit('.')
        .next()
        .and_then(|m| m.parse::<u64>().ok())
        .filter(|&m| m > 0)
        .map(|m| m * 60)
}

/// What the widget shows.
struct Numbers {
    today: u64,
    pages: u32,
    week: u64,
    /// Days in a row the goal was reached, up to today (or yesterday,
    /// while today is not done yet).
    streak: u32,
}

fn numbers(env: &Env<'_>) -> Option<Numbers> {
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
    let streak = goal(env).map_or(0, |goal| {
        let reached = |day: i32| seconds(day..=day) >= goal;
        let last = if reached(today) { today } else { today - 1 };
        (0..).take_while(|&back| reached(last - back)).count() as u32
    });
    Some(Numbers {
        today: seconds(today..=today),
        pages: overview::distinct_pages(env, today),
        week: seconds(since..=today),
        streak,
    })
}

/// "23 min · 41 pages", and below it the week or the goal: "7 min to
/// your daily goal", "Goal reached, 3 days in a row".
fn lines(env: &Env<'_>, numbers: &Numbers) -> (String, String) {
    let pages = env
        .i18n
        .plural("stats.pages", i64::from(numbers.pages), &[]);
    let first = format!("{} · {pages}", fmt::duration(env, numbers.today));
    let second = match goal(env) {
        None => env.i18n.format(
            "stats.widget.week",
            &[("time", Arg::Str(&fmt::duration(env, numbers.week)))],
        ),
        Some(goal) if numbers.today < goal => {
            let left = fmt::duration(env, (goal - numbers.today).max(60));
            let line = env
                .i18n
                .format("stats.goal.left", &[("time", Arg::Str(&left))]);
            match numbers.streak {
                0 => line,
                days => format!(
                    "{line} · {}",
                    env.i18n.plural("stats.goal.streak", i64::from(days), &[])
                ),
            }
        }
        Some(_) => match numbers.streak {
            0 | 1 => env.i18n.tr("stats.goal.reached").to_string(),
            days => env
                .i18n
                .plural("stats.goal.reached-days", i64::from(days), &[]),
        },
    };
    (first, second)
}

pub(crate) fn register(r: &mut dyn Registrar) -> Result<(), RegisterError> {
    r.add_setting(
        SettingSpec::new(
            GOAL,
            "stats.setting.goal",
            SettingKind::Choice(GOALS.iter().map(|g| Cow::Borrowed(*g)).collect()),
            SettingValue::Text(Cow::Borrowed(GOALS[0])),
        )
        .group(crate::GROUP)
        .description("stats.setting.goal.help"),
    )?;
    // A goal set shows on the home screen: the widget goes onto its first
    // page, after the book being read, if no page shows it yet.
    r.subscribe(
        quireink_sdk::builtin::events::SETTING_CHANGED,
        Box::new(|env, params, out| {
            if params.get(quireink_sdk::builtin::params::ID) != Some(GOAL.as_str())
                || goal(env).is_none()
            {
                return;
            }
            let pages: [&str; 4] = ["home.page-1", "home.page-2", "home.page-3", "home.page-4"];
            let shown = pages.iter().any(|page| {
                quireink_sdk::status::item_ids(env.settings.text(page))
                    .any(|id| id == TODAY.as_str())
            });
            if let Some(SettingValue::Text(list)) = env.settings.get(pages[0])
                && !shown
            {
                let mut ids: alloc::vec::Vec<&str> = quireink_sdk::status::item_ids(list).collect();
                let at = ids
                    .iter()
                    .position(|id| *id == "home.continue")
                    .map_or(ids.len(), |i| i + 1);
                let today = TODAY;
                ids.insert(at, today.as_str());
                out.push(quireink_sdk::Command::SetSetting(
                    Id::new_static("home.page-1"),
                    SettingValue::Text(Cow::Owned(ids.join(", "))),
                ));
            }
        }),
    )?;
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
            let bar = goal(env).map_or(0.0, |_| bar_band(env));
            (small * 2.0 + text + bar) as u16
        }),
        draw: Box::new(draw),
        signature: Some(Box::new(|env| {
            let Some(numbers) = numbers(env) else {
                return String::new();
            };
            let (a, b) = lines(env, &numbers);
            format!("{a}|{b}|{}", progress(env, &numbers))
        })),
        targets: None,
    })
}

/// How full the goal's bar is, in thousandths.
fn progress(env: &Env<'_>, numbers: &Numbers) -> u64 {
    goal(env).map_or(0, |goal| (numbers.today * 1000 / goal.max(1)).min(1000))
}

/// The height of the goal's bar with the space around it.
fn bar_band(env: &Env<'_>) -> f32 {
    env.metrics.mm(3) as f32
}

fn draw(canvas: &mut Canvas<'_>, env: &Env<'_>, area: Rect) {
    let style = UiStyle::new(env);
    let Some(font) = style.font(env) else {
        return;
    };
    let Some(numbers) = numbers(env) else {
        return;
    };
    let small = font.line_metrics(style.small).line_height as u16;
    let text = font.line_metrics(style.title).line_height as u16;
    let x = f32::from(area.x);
    let (first, second) = lines(env, &numbers);
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
        &first,
        Color::BLACK,
    );
    let mut y = area.y + small + text;
    if goal(env).is_some() {
        // A bar filled up to today's share of the goal.
        let band = bar_band(env) as u16;
        let height = env.metrics.mm(1).max(3);
        let line = env.metrics.tenth_mm(3).max(1);
        let top = i32::from(y + (band - height as u16) / 2);
        let width = u32::from(area.width);
        canvas.stroke_rect(i32::from(area.x), top, width, height, line, Color::BLACK);
        let filled = (u64::from(width) * progress(env, &numbers) / 1000) as u32;
        canvas.fill_rect(i32::from(area.x), top, filled, height, Color::BLACK);
        y += band;
    }
    ui::draw_label(canvas, &font, style.small, x, row(y, small), &second, gray);
}

#[cfg(test)]
mod tests {
    use super::GOALS;
    use alloc::vec::Vec;

    #[test]
    fn goals_are_minutes_or_off() {
        let minutes: Vec<Option<u64>> = GOALS
            .iter()
            .map(|g| g.rsplit('.').next().and_then(|m| m.parse().ok()))
            .collect();
        assert_eq!(minutes[0], None);
        assert!(minutes[1..].iter().all(Option::is_some));
    }
}
