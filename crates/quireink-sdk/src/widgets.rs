//! Widgets: small boxes plugins contribute for screens that show them,
//! such as the home screen's pages (a clock, the book being read, today's
//! reading, a grid of tools). Any plugin adds widgets with
//! [`Registrar::add_widget`](crate::Registrar::add_widget); screens let
//! the user choose which to show and where.
//!
//! A widget may have targets: places that do something when tapped, which
//! buttons reach one after the other too (see [`WidgetTarget`]).
//!
//! A widget may have settings of its own, e.g. where its text sits
//! ([`align_setting`]): they go in a settings group with the widget's id
//! ([`settings_group`]). The settings list leaves such groups out; screens
//! that edit widgets open them (slot
//! [`SETTINGS`](crate::builtin::slots::SETTINGS) with parameter
//! [`ID`](crate::builtin::params::ID)).
//!
//! The kernel brings two widgets for room between the others, which a list
//! may hold more than once ([`repeatable`]): [`SPACE`], a gap of a set
//! height, and [`FILL`], the room the others leave. Screens that place
//! widgets one below the other give each fill its share of that room
//! ([`fill_height`]); elsewhere it is not there.
//!
//! Lists of widgets may put several side by side in a row ([`rows`]):
//! equally wide unless one is given a part of the row ([`Width`]); a space
//! is then a gap across, a free space shares the room across.

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use quireink_graphics::Canvas;
use quireink_platform::Rect;

use crate::env::Env;
use crate::id::Id;
use crate::settings::{SettingGroupSpec, SettingKind, SettingSpec, SettingValue};

/// The height a widget needs for a width, in pixels.
pub type WidgetHeightFn = Box<dyn Fn(&Env<'_>, u16) -> u16>;
/// Draws a widget into its area (cleared before).
pub type WidgetDrawFn = Box<dyn Fn(&mut Canvas<'_>, &Env<'_>, Rect)>;
/// What a widget shows now, as text: screens redraw it when it changes
/// (they look each minute).
pub type WidgetSignatureFn = Box<dyn Fn(&Env<'_>) -> String>;
/// The targets of a widget drawn in an area, in reading order.
pub type WidgetTargetsFn = Box<dyn Fn(&Env<'_>, Rect) -> Vec<WidgetTarget>>;

/// A place in a widget that does something: a tap on it, or confirm
/// once buttons moved the focus there, runs `command`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WidgetTarget {
    pub area: Rect,
    pub command: crate::Command,
}

/// A widget contribution.
pub struct WidgetSpec {
    pub id: Id,
    /// Translation key of its name, e.g. "Clock".
    pub title: Cow<'static, str>,
    pub height: WidgetHeightFn,
    pub draw: WidgetDrawFn,
    /// Without one, a widget is drawn when its screen is.
    pub signature: Option<WidgetSignatureFn>,
    /// Without them, a widget only shows something.
    pub targets: Option<WidgetTargetsFn>,
}

impl fmt::Debug for WidgetSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WidgetSpec")
            .field("id", &self.id)
            .field("title", &self.title)
            .finish_non_exhaustive()
    }
}

/// The registered widgets.
#[derive(Debug, Default)]
pub struct Widgets {
    widgets: Vec<WidgetSpec>,
}

impl Widgets {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, widget: WidgetSpec) {
        self.widgets.push(widget);
    }

    pub fn shrink_to_fit(&mut self) {
        self.widgets.shrink_to_fit();
    }

    fn get(&self, id: &str) -> Option<&WidgetSpec> {
        self.widgets.iter().find(|w| w.id.as_str() == id)
    }

    /// Every widget: its id and the translation key of its name.
    pub fn iter(&self) -> impl Iterator<Item = (&Id, &str)> {
        self.widgets.iter().map(|w| (&w.id, w.title.as_ref()))
    }

    /// The translation key of the name of widget `id`.
    pub fn title(&self, id: &str) -> Option<&str> {
        self.get(id).map(|w| w.title.as_ref())
    }

    /// The height widget `id` needs at `width`; `None` if there is none.
    pub fn height(&self, id: &str, env: &Env<'_>, width: u16) -> Option<u16> {
        self.get(id).map(|w| (w.height)(env, width))
    }

    /// Draws widget `id` in `area`.
    pub fn draw(&self, id: &str, canvas: &mut Canvas<'_>, env: &Env<'_>, area: Rect) {
        if let Some(widget) = self.get(id) {
            (widget.draw)(canvas, env, area);
        }
    }

    /// The targets of widget `id` drawn in `area` (see [`WidgetTarget`]).
    pub fn targets(&self, id: &str, env: &Env<'_>, area: Rect) -> Vec<WidgetTarget> {
        self.get(id)
            .and_then(|w| w.targets.as_ref())
            .map_or_else(Vec::new, |targets| targets(env, area))
    }

    /// Whether widget `id` exists.
    pub fn has(&self, id: &str) -> bool {
        self.get(id).is_some()
    }

    /// What widget `id` shows now (see [`WidgetSignatureFn`]).
    pub fn signature(&self, id: &str, env: &Env<'_>) -> Option<String> {
        self.get(id)?.signature.as_ref().map(|s| s(env))
    }
}

/// A gap of the height [`SPACE_SIZE`] says.
pub const SPACE: &str = "core.space";
/// The room the other widgets leave, shared among the fills: its own
/// height is 0, screens that place widgets give it the rest.
pub const FILL: &str = "core.fill";
/// The height of [`SPACE`]: one of [`SPACE_SIZES`].
pub const SPACE_SIZE: &str = "core.space.size";
/// The values of [`SPACE_SIZE`]: small, medium, large.
pub const SPACE_SIZES: [&str; 3] = ["core.space.small", "core.space.medium", "core.space.large"];

/// Whether a list of widgets may hold `id` more than once.
pub fn repeatable(id: &str) -> bool {
    id == SPACE || id == FILL
}

/// The height of each of `fills` fills where widgets `used` pixels high
/// are placed in `room` pixels.
pub fn fill_height(room: u32, used: u32, fills: usize) -> u16 {
    if fills == 0 {
        return 0;
    }
    (room.saturating_sub(used) / fills as u32).min(u32::from(u16::MAX)) as u16
}

/// The values of a widget's alignment setting ([`align_setting`]).
pub const ALIGNMENTS: [&str; 3] = ["core.align.left", "core.align.center", "core.align.right"];

/// Where a widget's text sits in its place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

impl Align {
    /// The alignment a value of [`ALIGNMENTS`] names; left for others.
    pub fn of(value: &str) -> Self {
        match ALIGNMENTS.iter().position(|a| *a == value) {
            Some(1) => Align::Center,
            Some(2) => Align::Right,
            _ => Align::Left,
        }
    }

    /// Its value in [`ALIGNMENTS`].
    pub fn value(self) -> &'static str {
        ALIGNMENTS[self as usize]
    }

    /// Where something `width` wide starts in `area`.
    pub fn x(self, area: Rect, width: f32) -> f32 {
        let left = f32::from(area.x);
        let room = (f32::from(area.width) - width).max(0.0);
        match self {
            Align::Left => left,
            Align::Center => left + room / 2.0,
            Align::Right => left + room,
        }
    }
}

/// The settings group of `widget`'s own settings, named as the widget.
pub fn settings_group(widget: &Id, title: impl Into<Cow<'static, str>>) -> SettingGroupSpec {
    SettingGroupSpec {
        id: widget.clone(),
        title: title.into(),
        order: 0,
    }
}

/// The setting of where `widget`'s text sits, `<widget>.align`, in the
/// widget's settings group; read it with [`align`].
pub fn align_setting(widget: &Id, default: Align) -> SettingSpec {
    let id = Id::parse(&alloc::format!("{widget}.align")).unwrap_or_else(|_| widget.clone());
    SettingSpec::new(
        id,
        "core.setting.align",
        SettingKind::Choice(ALIGNMENTS.iter().map(|a| Cow::Borrowed(*a)).collect()),
        SettingValue::Text(Cow::Borrowed(default.value())),
    )
    .group(widget.clone())
}

/// Where `widget`'s text sits, by its [`align_setting`]; left without one.
pub fn align(env: &Env<'_>, widget: &str) -> Align {
    Align::of(env.settings.text(&alloc::format!("{widget}.align")))
}

// ---- Rows: widgets side by side ----------------------------------------------

/// How wide a widget is in a row of several.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Width {
    /// A share of what the others leave, as much as each other automatic
    /// one: widgets of a row are equally wide unless told otherwise.
    Auto,
    /// A part of the row, e.g. 1/3.
    Part(u8, u8),
}

/// The widths a widget in a row may be given, in this order.
pub const WIDTHS: [Width; 6] = [
    Width::Auto,
    Width::Part(1, 4),
    Width::Part(1, 3),
    Width::Part(1, 2),
    Width::Part(2, 3),
    Width::Part(3, 4),
];

impl Width {
    /// As written after a widget's id: `1/3`; nothing for automatic.
    pub fn text(self) -> Option<String> {
        match self {
            Width::Auto => None,
            Width::Part(n, d) => Some(alloc::format!("{n}/{d}")),
        }
    }

    fn parse(text: &str) -> Option<Width> {
        let (n, d) = text.trim().split_once('/')?;
        let (n, d) = (n.trim().parse::<u8>().ok()?, d.trim().parse::<u8>().ok()?);
        (n > 0 && d > 0 && n <= d).then_some(Width::Part(n, d))
    }
}

/// A widget in a list: its id, and how wide it is in its row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    pub id: String,
    pub width: Width,
}

impl Placement {
    pub fn new(id: &str) -> Self {
        Self {
            id: String::from(id),
            width: Width::Auto,
        }
    }
}

/// The rows of a list of widgets as settings keep it: rows separated by
/// commas, widgets side by side by `|`, a width in parentheses, e.g.
/// `home.status, home.clock (1/3) | home.date, home.continue`. Lists
/// without `|` are a widget a row, as before rows existed.
pub fn rows(list: &str) -> Vec<Vec<Placement>> {
    list.split(',')
        .map(|row| {
            row.split('|')
                .filter_map(|item| {
                    let item = item.trim();
                    let (id, width) = match item.split_once('(') {
                        Some((id, rest)) => (
                            id.trim(),
                            Width::parse(rest.trim_end_matches(')')).unwrap_or(Width::Auto),
                        ),
                        None => (item, Width::Auto),
                    };
                    (!id.is_empty()).then(|| Placement {
                        id: String::from(id),
                        width,
                    })
                })
                .collect::<Vec<_>>()
        })
        .filter(|row| !row.is_empty())
        .collect()
}

/// `rows` as settings keep them (see [`rows`]).
pub fn write_rows(rows: &[Vec<Placement>]) -> String {
    let mut out = String::new();
    for (r, row) in rows.iter().enumerate() {
        if r > 0 {
            out.push_str(", ");
        }
        for (i, p) in row.iter().enumerate() {
            if i > 0 {
                out.push_str(" | ");
            }
            out.push_str(&p.id);
            if let Some(width) = p.width.text() {
                out.push_str(&alloc::format!(" ({width})"));
            }
        }
    }
    out
}

/// Every widget id of a list, row after row.
pub fn ids(list: &str) -> Vec<String> {
    rows(list).into_iter().flatten().map(|p| p.id).collect()
}

/// Where the widgets of a row go across `width` pixels starting at `x`,
/// with `gap` between them: each one's x and width, in order. Parts take
/// their part of the row, [`SPACE`] its size, and automatic widgets and
/// [`FILL`] share the rest equally.
pub fn columns(env: &Env<'_>, row: &[Placement], x: u16, width: u16, gap: u16) -> Vec<(u16, u16)> {
    let n = row.len().max(1) as u32;
    let room = u32::from(width).saturating_sub(u32::from(gap) * (n - 1));
    let fixed = |p: &Placement| -> Option<u32> {
        match p.width {
            Width::Part(a, b) => Some(room * u32::from(a) / u32::from(b)),
            Width::Auto if p.id == SPACE => Some(u32::from(
                env.widgets.height(SPACE, env, width).unwrap_or(0),
            )),
            Width::Auto => None,
        }
    };
    let taken: u32 = row.iter().filter_map(fixed).sum();
    let autos = row.iter().filter(|p| fixed(p).is_none()).count() as u32;
    let share = room.saturating_sub(taken).checked_div(autos).unwrap_or(0);
    let mut at = u32::from(x);
    row.iter()
        .map(|p| {
            let w = fixed(p).unwrap_or(share).min(u32::from(u16::MAX));
            let column = (at.min(u32::from(u16::MAX)) as u16, w as u16);
            at += w + u32::from(gap);
            column
        })
        .collect()
}

/// What a row is in a list placed one row below the other.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowKind {
    /// Widgets `height` pixels high, at their columns (see [`columns`]).
    Widgets {
        height: u16,
        columns: Vec<(u16, u16)>,
    },
    /// A free space alone: a share of the room the rows leave.
    Fill,
}

/// The kind and height of `row` placed across `width` pixels from `x`;
/// `None` if it shows nothing.
pub fn row_kind(env: &Env<'_>, row: &[Placement], x: u16, width: u16, gap: u16) -> Option<RowKind> {
    if row.iter().all(|p| p.id == FILL) {
        return Some(RowKind::Fill);
    }
    let columns = columns(env, row, x, width, gap);
    // A space alone is as high as its size; in a row, only as wide.
    let height = if row.len() == 1 {
        env.widgets.height(&row[0].id, env, width)?
    } else {
        row.iter()
            .zip(&columns)
            .filter(|(p, _)| p.id != SPACE && p.id != FILL)
            .filter_map(|(p, (_, w))| env.widgets.height(&p.id, env, *w))
            .max()?
    };
    (height > 0).then_some(RowKind::Widgets { height, columns })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alignments_place_text_in_the_area() {
        let area = Rect::new(10, 0, 100, 20);
        assert_eq!(Align::Left.x(area, 40.0), 10.0);
        assert_eq!(Align::Center.x(area, 40.0), 40.0);
        assert_eq!(Align::Right.x(area, 40.0), 70.0);
        // Too wide: from the left edge.
        assert_eq!(Align::Right.x(area, 140.0), 10.0);
    }

    #[test]
    fn alignments_round_trip() {
        for align in [Align::Left, Align::Center, Align::Right] {
            assert_eq!(Align::of(align.value()), align);
        }
        assert_eq!(Align::of("sideways"), Align::Left);
    }

    #[test]
    fn fills_share_the_room_left() {
        assert_eq!(fill_height(1000, 400, 2), 300);
        assert_eq!(fill_height(1000, 1200, 1), 0);
        assert_eq!(fill_height(1000, 400, 0), 0);
        assert!(repeatable(FILL) && repeatable(SPACE) && !repeatable("home.clock"));
    }

    #[test]
    fn rows_are_read_and_written() {
        let list = "home.status, home.clock (1/3) | core.fill | home.date,home.continue";
        let read = rows(list);
        assert_eq!(read.len(), 3);
        assert_eq!(read[1].len(), 3);
        assert_eq!(read[1][0].width, Width::Part(1, 3));
        assert_eq!(read[1][2], Placement::new("home.date"));
        assert_eq!(
            write_rows(&read),
            "home.status, home.clock (1/3) | core.fill | home.date, home.continue"
        );
        assert_eq!(
            ids(list),
            [
                "home.status",
                "home.clock",
                "core.fill",
                "home.date",
                "home.continue"
            ]
        );
        // Nonsense widths are automatic; empty items are none.
        assert_eq!(rows("a (7/2) | , ,b")[0][0].width, Width::Auto);
        assert_eq!(rows("a (7/2) | , ,b").len(), 2);
    }

    #[test]
    fn the_setting_belongs_to_the_widget() {
        let widget = Id::new_static("home.clock");
        let spec = align_setting(&widget, Align::Center);
        assert_eq!(spec.id.as_str(), "home.clock.align");
        assert_eq!(spec.group, widget);
        assert_eq!(
            spec.default,
            SettingValue::Text(Cow::Borrowed("core.align.center"))
        );
    }
}
