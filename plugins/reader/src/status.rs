//! The reader's status lines above and below the text.
//!
//! For each of the two places the user chooses status lines to switch
//! between (setting [`LINES`]); a tap on the line shows the next one, and
//! the choice is kept. Lines come from any plugin
//! (`Registrar::add_status_line`); the reader brings three:
//!
//! - *Text and symbols* ([`lines::ITEMS`]): a left, centre and right part,
//!   each with the status items chosen for it (`core.status-items`: the
//!   kernel's time and battery, the reader's title, chapter, page and
//!   progress, and whatever other plugins add);
//! - *Progress bar* ([`lines::PROGRESS`]): the book as a bar, with a mark
//!   where each chapter begins, and the page;
//! - *Empty* ([`lines::EMPTY`]): nothing, until the next tap.
//!
//! A place without lines takes no space. A place's height depends only
//! on the text size of status lines ([`SIZE`]), so switching lines never
//! changes the pages.

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::{fonts, params, screens};
use quireink_sdk::status::{StatusFrame, StatusLine, StatusValue, context, item_ids};
use quireink_sdk::ui::UiStyle;
use quireink_sdk::{
    Arg, Canvas, Env, Id, Params, RegisterError, Registrar, SettingGroupSpec, SettingKind,
    SettingSpec, SettingValue, StatusItemSpec, StatusLineSpec, Storage,
};

use crate::PLUGIN;

/// The settings group of the status lines.
pub const GROUP: Id = Id::new_static("reader.group.status");

/// The battery's charge in percent beside its symbol.
pub const PERCENT: Id = Id::new_static("reader.status.battery-percent");
/// Text size of the status lines, in points.
pub const SIZE: Id = Id::new_static("reader.status.size");
/// The status lines to switch between, above and below the text (ids
/// separated by commas; the first shows at first).
pub const LINES: [Id; 2] = [
    Id::new_static("reader.status.top-lines"),
    Id::new_static("reader.status.bottom-lines"),
];
/// The settings' sections: the line above and the line below.
const SECTIONS: [&str; 2] = ["reader.status.section.top", "reader.status.section.bottom"];

/// The reader's status lines.
pub mod lines {
    use quireink_sdk::Id;

    pub const ITEMS: Id = Id::new_static("reader.status-line.items");
    pub const PROGRESS: Id = Id::new_static("reader.status-line.progress");
    pub const EMPTY: Id = Id::new_static("reader.status-line.empty");
}

/// The parts: top left, centre, right, then bottom left, centre, right.
pub const PARTS: [Id; 6] = [
    Id::new_static("reader.status.top-left"),
    Id::new_static("reader.status.top-center"),
    Id::new_static("reader.status.top-right"),
    Id::new_static("reader.status.bottom-left"),
    Id::new_static("reader.status.bottom-center"),
    Id::new_static("reader.status.bottom-right"),
];

/// The reader's status items.
pub mod items {
    use quireink_sdk::Id;

    pub const TITLE: Id = Id::new_static("reader.status.title");
    /// The chapter, or the title in books without a table of contents.
    pub const CHAPTER: Id = Id::new_static("reader.status.chapter");
    /// `12 / 340`, or a percentage while pages are being counted.
    pub const PAGE: Id = Id::new_static("reader.status.page");
    /// Reading progress in percent.
    pub const PROGRESS: Id = Id::new_static("reader.status.progress");
    /// A speaker while the book is read aloud, a clock while its pages
    /// turn by themselves.
    pub const ALOUD: Id = Id::new_static("reader.status.aloud");
    /// The page of the printed edition, where the book names it.
    pub const PRINTED_PAGE: Id = Id::new_static("reader.status.printed-page");
}

/// Context: the printed edition's page.
pub(crate) const PRINTED_CONTEXT: &str = "reader.printed-page";

/// Context: the icon of what turns the pages by itself (reading aloud,
/// turning automatically).
pub(crate) const ALOUD_CONTEXT: &str = "reader.aloud";

pub(crate) fn register(r: &mut dyn Registrar) -> Result<(), RegisterError> {
    r.add_setting_group(SettingGroupSpec {
        id: GROUP,
        title: Cow::Borrowed("reader.group.status"),
        order: 15,
    })?;
    r.add_icon(GROUP, Cow::Borrowed("layout"))?;
    // The chapter, the time and the battery with the page.
    let defaults = [
        "",
        "",
        "",
        "reader.status.aloud, reader.status.chapter",
        "core.status.clock",
        "core.status.battery, reader.status.page",
    ];
    let titles = [
        "reader.status.top-left",
        "reader.status.top-center",
        "reader.status.top-right",
        "reader.status.bottom-left",
        "reader.status.bottom-center",
        "reader.status.bottom-right",
    ];
    r.add_setting(
        SettingSpec::new(
            SIZE,
            "reader.status.size",
            SettingKind::Int { min: 5, max: 16 },
            SettingValue::Int(6),
        )
        .group(GROUP)
        .unit("core.unit.pt")
        .description("reader.status.size.help"),
    )?;
    // Below the text: the items, the progress bar, nothing; above: none.
    let line_defaults = [
        "",
        "reader.status-line.items, reader.status-line.progress, reader.status-line.empty",
    ];
    let line_titles = ["reader.status.top-lines", "reader.status.bottom-lines"];
    for (i, id) in LINES.into_iter().enumerate() {
        r.add_setting(
            SettingSpec::new(
                id,
                line_titles[i],
                SettingKind::List,
                SettingValue::Text(Cow::Borrowed(line_defaults[i])),
            )
            .group(GROUP)
            .section(SECTIONS[i])
            .editor(screens::STATUS_LINES)
            .description("reader.status.lines.help"),
        )?;
    }
    for (n, ((id, title), default)) in PARTS.into_iter().zip(titles).zip(defaults).enumerate() {
        let spec = SettingSpec::new(
            id,
            title,
            SettingKind::List,
            SettingValue::Text(Cow::Owned(default.to_string())),
        )
        .group(GROUP)
        .section(SECTIONS[n / 3])
        .editor(screens::STATUS_ITEMS);
        r.add_setting(spec)?;
    }
    r.add_setting(
        SettingSpec::new(
            PERCENT,
            "reader.status.battery-percent",
            SettingKind::Bool,
            SettingValue::Bool(true),
        )
        .group(GROUP)
        .description("reader.status.battery-percent.help"),
    )?;
    r.add_status_line(StatusLineSpec {
        id: lines::ITEMS,
        title: Cow::Borrowed("reader.status.line.items"),
        draw: Box::new(|canvas, env, frame| {
            let Some(font) = env.fonts.role(fonts::UI.as_str()) else {
                return;
            };
            let line = StatusLine {
                area: frame.area,
                lists: lists(env, frame.top),
                size: frame.size,
                color: frame.color,
                percent: env.settings.bool(PERCENT.as_str()),
            };
            line.draw(canvas, env, &font, frame.context);
        }),
        signature: Some(Box::new(items_signature)),
    })?;
    r.add_status_line(StatusLineSpec {
        id: lines::PROGRESS,
        title: Cow::Borrowed("reader.status.line.progress"),
        draw: Box::new(draw_progress),
        signature: Some(Box::new(|_, cx| {
            format!(
                "{}|{}|{}",
                cx.get(params::PROGRESS).unwrap_or_default(),
                cx.get(context::PAGE).unwrap_or_default(),
                cx.get(context::CHAPTERS).unwrap_or_default().len(),
            )
        })),
    })?;
    r.add_status_line(StatusLineSpec {
        id: lines::EMPTY,
        title: Cow::Borrowed("reader.status.line.empty"),
        draw: Box::new(|_, _, _| {}),
        signature: Some(Box::new(|_, _| String::new())),
    })?;
    let text = |value: Option<&str>| value.map(|v| StatusValue::Text(v.to_string()));
    r.add_status_item(StatusItemSpec {
        id: items::TITLE,
        title: Cow::Borrowed("reader.status.item.title"),
        value: Box::new(move |_, cx| text(cx.get(params::TITLE))),
    })?;
    r.add_status_item(StatusItemSpec {
        id: items::CHAPTER,
        title: Cow::Borrowed("reader.status.item.chapter"),
        value: Box::new(move |_, cx| text(cx.get(context::CHAPTER).or(cx.get(params::TITLE)))),
    })?;
    r.add_status_item(StatusItemSpec {
        id: items::ALOUD,
        title: Cow::Borrowed("reader.status.item.aloud"),
        value: Box::new(|_, cx| {
            cx.get(ALOUD_CONTEXT)
                .map(|icon| StatusValue::Icon(Cow::Owned(icon.to_string())))
        }),
    })?;
    r.add_status_item(StatusItemSpec {
        id: items::PRINTED_PAGE,
        title: Cow::Borrowed("reader.status.item.printed-page"),
        value: Box::new(|env, cx| {
            let page = cx.get(PRINTED_CONTEXT)?;
            Some(StatusValue::Text(
                env.i18n
                    .format("reader.printed-page", &[("page", Arg::Str(page))]),
            ))
        }),
    })?;
    r.add_status_item(StatusItemSpec {
        id: items::PAGE,
        title: Cow::Borrowed("reader.status.item.page"),
        value: Box::new(move |_, cx| text(cx.get(context::PAGE))),
    })?;
    r.add_status_item(StatusItemSpec {
        id: items::PROGRESS,
        title: Cow::Borrowed("reader.status.item.progress"),
        value: Box::new(|env, cx| {
            let percent: f32 = cx.get(params::PROGRESS)?.parse().ok()?;
            Some(StatusValue::Text(env.i18n.format(
                "reader.percent",
                &[("percent", Arg::Int(percent as i64))],
            )))
        }),
    })?;
    Ok(())
}

/// The item lists of a line's parts: `top` or bottom.
pub(crate) fn lists<'e>(env: &Env<'e>, top: bool) -> [&'e str; 3] {
    let offset = if top { 0 } else { 3 };
    core::array::from_fn(|i| env.settings.text(PARTS[offset + i].as_str()))
}

/// The text size of status lines in pixels.
pub(crate) fn size(env: &Env<'_>) -> f32 {
    env.metrics
        .pt(env.settings.int(SIZE.as_str()).clamp(5, 16) as f32)
}

/// The height of a status line's place.
pub(crate) fn height(env: &Env<'_>) -> f32 {
    (size(env) * 2.1 + 0.5) as u32 as f32
}

/// The lines to switch between at the top (or bottom).
pub(crate) fn rotation<'e>(env: &Env<'e>, top: bool) -> Vec<&'e str> {
    item_ids(env.settings.text(LINES[usize::from(!top)].as_str())).collect()
}

/// The line showing at the top (or bottom): the one chosen last if it is
/// still among the lines to switch between, else the first.
pub(crate) fn current<'e>(env: &Env<'e>, top: bool, chosen: &str) -> &'e str {
    let rotation = rotation(env, top);
    rotation
        .iter()
        .find(|id| **id == chosen)
        .or(rotation.first())
        .copied()
        .unwrap_or("")
}

/// The line after `chosen` at the top (or bottom).
pub(crate) fn next<'e>(env: &Env<'e>, top: bool, chosen: &str) -> &'e str {
    let rotation = rotation(env, top);
    let at = rotation
        .iter()
        .position(|id| *id == current(env, top, chosen));
    match at {
        Some(at) => rotation[(at + 1) % rotation.len()],
        None => "",
    }
}

fn choice_path() -> String {
    format!("{}/status-lines.txt", Env::plugin_dir(PLUGIN.as_str()))
}

/// The lines chosen last, top then bottom.
pub(crate) fn load_choice(storage: &dyn Storage) -> [String; 2] {
    let data = quireink_sdk::read_file(storage, &choice_path(), 1024).unwrap_or_default();
    let text = String::from_utf8_lossy(&data);
    let mut lines = text.lines().map(|l| l.trim().to_string());
    [
        lines.next().unwrap_or_default(),
        lines.next().unwrap_or_default(),
    ]
}

pub(crate) fn save_choice(storage: &dyn Storage, chosen: &[String; 2]) {
    let _ = storage.write(
        &choice_path(),
        format!("{}\n{}\n", chosen[0], chosen[1]).as_bytes(),
    );
}

/// The colour of status lines: gray, or black for strong contrast.
pub(crate) fn color(env: &Env<'_>) -> quireink_sdk::Color {
    UiStyle::new(env).faint(80)
}

/// The book as a bar from the left, filled up to where the page is, with
/// a mark where each chapter begins; the page on the right.
fn draw_progress(canvas: &mut Canvas<'_>, env: &Env<'_>, frame: &StatusFrame<'_>) {
    let Some(font) = env.fonts.role(fonts::UI.as_str()) else {
        return;
    };
    let area = frame.area;
    let progress = frame
        .context
        .get(params::PROGRESS)
        .and_then(|p| p.parse::<f32>().ok())
        .unwrap_or(0.0)
        .clamp(0.0, 100.0)
        / 100.0;
    let label = frame.context.get(context::PAGE).unwrap_or_default();
    let label_width = font.measure(frame.size, label);
    let gap = if label.is_empty() { 0.0 } else { frame.size };
    let x0 = f32::from(area.x);
    let width = (f32::from(area.width) - label_width - gap).max(1.0);
    let thickness = (frame.size * 0.3).max(3.0) as u32;
    let mid = i32::from(area.y) + i32::from(area.height) / 2;
    let top = mid - thickness as i32 / 2;
    let line = (frame.size / 12.0).max(1.0) as u32;
    canvas.stroke_rect(x0 as i32, top, width as u32, thickness, line, frame.color);
    let filled = (width * progress) as u32;
    canvas.fill_rect(x0 as i32, top, filled, thickness, frame.color);
    // Chapter marks stand out above and below the bar.
    let reach = thickness as i32;
    for mark in frame
        .context
        .get(context::CHAPTERS)
        .unwrap_or_default()
        .split(',')
        .filter_map(|m| m.trim().parse::<u32>().ok())
        .filter(|&m| m > 0 && m < 1000)
    {
        let x = x0 as i32 + (width * mark as f32 / 1000.0) as i32;
        canvas.fill_rect(
            x,
            top - reach / 2,
            line,
            thickness + reach as u32,
            frame.color,
        );
    }
    if !label.is_empty() {
        let metrics = font.line_metrics(frame.size);
        let baseline = i32::from(area.y)
            + ((f32::from(area.height) + metrics.ascent - metrics.descent) / 2.0) as i32;
        let x = x0 + f32::from(area.width) - label_width;
        font.draw(canvas, frame.size, x, baseline, label, frame.color);
    }
}

/// Whether any part shows item `id`.
pub(crate) fn uses(env: &Env<'_>, id: &Id) -> bool {
    PARTS
        .iter()
        .any(|part| item_ids(env.settings.text(part.as_str())).any(|i| i == id.as_str()))
}

/// Whether there is a line at the top (or bottom): some status line to
/// show there.
pub(crate) fn shown(env: &Env<'_>, top: bool) -> bool {
    !rotation(env, top).is_empty()
}

/// What the lines shown now show, to notice when they need a redraw.
pub(crate) fn signature(env: &Env<'_>, context: &Params, chosen: &[String; 2]) -> String {
    let mut out = String::new();
    for top in [true, false] {
        let id = current(env, top, &chosen[usize::from(!top)]);
        out.push_str(id);
        out.push('|');
        out.push_str(&env.status_items.line_signature(id, env, context));
        out.push('|');
    }
    out
}

/// What the items of both lines show now.
fn items_signature(env: &Env<'_>, context: &Params) -> String {
    let mut out = String::new();
    for list in lists(env, true).iter().chain(&lists(env, false)) {
        for id in item_ids(list) {
            if let Some(value) = env.status_items.value(id, env, context) {
                out.push_str(&format!("{value:?}|"));
            }
        }
    }
    out
}
