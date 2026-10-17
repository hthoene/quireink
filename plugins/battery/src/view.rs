//! The view: the battery now, this time awake, the use on battery so far
//! (an hour awake, 100 pages, the hours left at that pace) and the recent
//! times awake.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use quireink_sdk::ui::{self, UiStyle};
use quireink_sdk::{
    Arg, Canvas, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Rect, Screen,
};

use crate::log::{self, Kind, Reading, Session, Sessions};

/// A line of the view.
enum Line {
    Heading(String),
    Value(String, String),
    Message(String),
}

pub(crate) struct BatteryView {
    style: UiStyle,
    header: ui::Header,
    lines: Vec<Line>,
    /// Where the log is, said at the bottom.
    footer: String,
}

/// `ms` as hours and minutes.
fn duration(env: &Env<'_>, ms: u64) -> String {
    let minutes = ms / 60_000;
    let (h, m) = ((minutes / 60) as i64, (minutes % 60) as i64);
    if minutes == 0 {
        env.i18n.tr("battery.under-a-minute").into()
    } else if h == 0 {
        env.i18n.format("battery.minutes", &[("m", Arg::Int(m))])
    } else {
        env.i18n
            .format("battery.hours", &[("h", Arg::Int(h)), ("m", Arg::Int(m))])
    }
}

/// A share of the battery with one decimal, e.g. `4.2 %`.
fn percent(env: &Env<'_>, value: f32) -> String {
    let tenths = (value * 10.0 + 0.5) as u32;
    let point = env.i18n.tr("battery.decimal-point");
    let number = format!("{}{point}{}", tenths / 10, tenths % 10);
    env.i18n
        .format("battery.percent", &[("value", Arg::Str(&number))])
}

/// The voltage, e.g. `4.01 V`.
fn volts(env: &Env<'_>, millivolts: u16) -> String {
    let point = env.i18n.tr("battery.decimal-point");
    format!(
        "{}{point}{:02} V",
        millivolts / 1000,
        (millivolts % 1000 + 5) / 10
    )
}

/// Plugins listed in each section.
const PLUGINS_SHOWN: usize = 4;

/// A plugin's name; the kernel's work is QuireInk's.
fn plugin_name(env: &Env<'_>, id: &str) -> String {
    match env.plugins.get(id) {
        Some(info) => String::from(info.name(env.i18n)),
        None if id == "core" => String::from(env.i18n.tr("battery.core")),
        None => String::from(id),
    }
}

/// Microseconds as seconds with a decimal, e.g. `3.2 s`.
fn seconds(env: &Env<'_>, micros: u64) -> String {
    let tenths = micros / 100_000;
    let point = env.i18n.tr("battery.decimal-point");
    format!("{}{point}{} s", tenths / 10, tenths % 10)
}

/// What a time awake used, for the list of recent ones.
fn session_line(env: &Env<'_>, s: &Session) -> Line {
    let pages = env
        .i18n
        .plural("battery.pages-count", i64::from(s.pages()), &[]);
    let used = s.used().unwrap_or(0);
    Line::Value(
        format!("{} · {pages}", duration(env, s.millis())),
        env.i18n.format(
            "battery.used-percent",
            &[("value", Arg::Int(i64::from(used)))],
        ),
    )
}

impl BatteryView {
    pub(crate) fn new(env: &Env<'_>, now: Reading, dir: &str) -> Self {
        let style = UiStyle::new(env);
        let header = ui::Header::new(env, &style, true);
        let tr = |key: &str| String::from(env.i18n.tr(key));
        let mut sessions = Sessions::read(env.storage, dir);
        // This time awake up to now.
        sessions.feed(Kind::Sample, now);
        let mut lines = Vec::new();

        lines.push(Line::Heading(tr("battery.now")));
        if let Some(charge) = now.percent {
            let mut value = env
                .i18n
                .format("battery.percent", &[("value", Arg::Int(i64::from(charge)))]);
            if let Some(mv) = now.millivolts {
                value = format!("{value} · {}", volts(env, mv));
            }
            lines.push(Line::Value(tr("battery.charge"), value));
            let source = if now.charging {
                "battery.charging"
            } else {
                "battery.on-battery"
            };
            lines.push(Line::Value(tr("battery.power"), tr(source)));
        } else {
            lines.push(Line::Message(tr("battery.unknown")));
        }

        if let Some(current) = &sessions.current {
            lines.push(Line::Heading(tr("battery.awake")));
            lines.push(Line::Value(
                tr("battery.awake-for"),
                duration(env, current.millis()),
            ));
            if let (Some(from), Some(to)) = (current.first.percent, current.last.percent) {
                lines.push(Line::Value(
                    tr("battery.from-to"),
                    env.i18n.format(
                        "battery.percent-from-to",
                        &[
                            ("from", Arg::Int(i64::from(from))),
                            ("to", Arg::Int(i64::from(to))),
                        ],
                    ),
                ));
            }
            lines.push(Line::Value(
                tr("battery.pages"),
                format!("{}", current.pages()),
            ));
            lines.push(Line::Value(
                tr("battery.refreshes"),
                format!("{}", current.refreshes()),
            ));
        }

        lines.push(Line::Heading(tr("battery.so-far")));
        let totals = sessions.totals();
        match totals.per_hour() {
            Some(rate) => {
                lines.push(Line::Value(tr("battery.per-hour"), percent(env, rate)));
                if let Some(per_pages) = totals.per_100_pages() {
                    lines.push(Line::Value(
                        tr("battery.per-100-pages"),
                        percent(env, per_pages),
                    ));
                }
                if let Some(hours) = now.percent.and_then(|p| totals.hours_left(p)) {
                    lines.push(Line::Value(
                        tr("battery.left"),
                        env.i18n.format(
                            "battery.about-hours",
                            &[("h", Arg::Int((hours + 0.5) as i64))],
                        ),
                    ));
                }
            }
            None => lines.push(Line::Message(tr("battery.too-little"))),
        }

        // What each plugin took this time awake (its screens shown, its
        // code running): for developers.
        let developer = env
            .settings
            .bool(quireink_sdk::builtin::settings::DEVELOPER.as_str())
            && env.settings.bool(crate::BY_PLUGIN.as_str());
        let mut shown: Vec<_> = env
            .usage
            .plugins
            .iter()
            .filter(|p| developer && p.shown_ms >= 60_000)
            .collect();
        shown.sort_by_key(|p| core::cmp::Reverse(p.shown_ms));
        if !shown.is_empty() {
            lines.push(Line::Heading(tr("battery.by-screen")));
            for p in shown.into_iter().take(PLUGINS_SHOWN) {
                let refreshes = p.refreshes.iter().sum::<u32>();
                lines.push(Line::Value(
                    plugin_name(env, p.plugin.as_str()),
                    env.i18n.format(
                        "battery.shown-refreshes",
                        &[
                            ("time", Arg::Str(&duration(env, p.shown_ms))),
                            ("count", Arg::Int(i64::from(refreshes))),
                        ],
                    ),
                ));
            }
        }
        let mut busy: Vec<_> = env
            .usage
            .plugins
            .iter()
            .filter(|p| developer && p.busy_us >= 100_000)
            .collect();
        busy.sort_by_key(|p| core::cmp::Reverse(p.busy_us));
        if !busy.is_empty() {
            lines.push(Line::Heading(tr("battery.by-processor")));
            for p in busy.into_iter().take(PLUGINS_SHOWN) {
                lines.push(Line::Value(
                    plugin_name(env, p.plugin.as_str()),
                    seconds(env, p.busy_us),
                ));
            }
        }

        let recent: Vec<&Session> = sessions.ended.iter().rev().filter(|s| s.counts()).collect();
        if !recent.is_empty() {
            lines.push(Line::Heading(tr("battery.recent")));
            lines.extend(recent.into_iter().map(|s| session_line(env, s)));
        }

        let footer = env.i18n.format(
            "battery.log-file",
            &[("path", Arg::Str(&format!("{dir}/{}", log::FILE)))],
        );
        Self {
            style,
            header,
            lines,
            footer,
        }
    }
}

impl Screen for BatteryView {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        self.header.draw(
            canvas,
            env,
            &style,
            env.i18n.tr("battery.title"),
            None,
            None,
        );
        let display = env.device.display;
        let footer_height = style.row as u16;
        let bottom = display.height.saturating_sub(footer_height);
        let mut y = self.header.area.y + self.header.area.height;
        let heading = ui::heading_height(env, &style);
        for line in &self.lines {
            let height = match line {
                Line::Heading(_) => heading,
                Line::Value(..) | Line::Message(_) => style.row as u16,
            };
            if y + height > bottom {
                break;
            }
            let rect = Rect::new(0, y, display.width, height);
            match line {
                Line::Heading(text) => ui::draw_heading(canvas, env, &style, rect, text),
                Line::Value(title, value) => {
                    ui::draw_value_row(canvas, env, &style, rect, false, title, value);
                }
                Line::Message(text) => {
                    if let Some(font) = style.font(env) {
                        ui::draw_label(
                            canvas,
                            &font,
                            style.small,
                            style.margin as f32,
                            rect,
                            text,
                            style.faint(90),
                        );
                    }
                }
            }
            y += height;
        }
        if let Some(font) = style.font(env) {
            let rect = Rect::new(0, bottom, display.width, footer_height);
            ui::draw_label(
                canvas,
                &font,
                style.small,
                style.margin as f32,
                rect,
                &self.footer,
                style.faint(60),
            );
        }
    }

    fn on_action(&mut self, _: &Id, _: &mut Cx<'_, '_>) -> Handled {
        Handled::No
    }

    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        if gesture.kind != GestureKind::Tap {
            return Handled::No;
        }
        match self.header.tap(cx.env(), &self.style, gesture.start, None) {
            Some(action) => {
                cx.command(Command::RunAction(action));
                Handled::Yes
            }
            None => Handled::No,
        }
    }
}
