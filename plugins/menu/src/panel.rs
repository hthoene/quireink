//! A menu shown as a panel of controls at the bottom of the screen, for
//! quick changes while reading: switches become buttons (several to a
//! row), numbers get `−` and `+` (those marked as sliders, such as the
//! frontlight, a slider between them), choices with few options become
//! segments,
//! other choices open their list, actions become buttons. Changes apply at
//! once and the panel stays open.
//!
//! Bound to the `core.panel` slot with the parameter `menu`; what the
//! panel shows is that menu, so users and plugins choose its entries like
//! any menu's.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::{actions, events, params, slots};
use quireink_sdk::ui::{self, UiStyle};
use quireink_sdk::{
    Canvas, Color, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Params, Rect, RefreshMode,
    Screen, SettingKind, SettingSpec, SettingValue,
};

use crate::{CHOOSE_SCREEN, is_menu};

/// Most switches or buttons in one row.
const PER_ROW: usize = 3;
/// Most options shown as segments; more open a list.
const MAX_SEGMENTS: usize = 4;
/// Most of the screen the panel may cover.
const MAX_SHARE: f32 = 0.8;

/// What tapping a control does.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Control {
    /// Switches a setting on or off.
    Toggle(Id),
    /// Changes a number by a step down (`-1`) or up (`1`).
    Step(Id, i64),
    /// A number's slider: a tap sets the value where it lands.
    Slide(Id),
    /// Sets a choice to this option.
    Option(Id, String),
    /// Opens the setting's list or editor.
    Open(Id),
    /// Runs an action or opens a submenu.
    Run(Id),
}

/// A row: an optional label on the left, then its controls.
#[derive(Debug)]
struct Row {
    area: Rect,
    /// The setting the label names (a number's or a choice's).
    label: Option<Id>,
    controls: Vec<(Control, Rect)>,
}

pub(crate) struct PanelScreen {
    rows: Vec<Row>,
    style: UiStyle,
    region: Rect,
    /// The control chosen with buttons: its place in reading order.
    focus: Option<usize>,
    /// Rows shown at a time; more go to further pages.
    per_page: usize,
    page: usize,
    pager: Option<ui::Pager>,
}

fn spec<'e>(env: &Env<'e>, id: &Id) -> Option<&'e SettingSpec> {
    env.settings.specs().find(|spec| spec.id == *id)
}

impl PanelScreen {
    pub(crate) fn open(env: &Env<'_>, params: &Params) -> Self {
        let menu = params.get(params::MENU).unwrap_or_default();
        let style = UiStyle::new(env);
        let display = env.device.display;
        let width = display.width;
        let margin = style.margin as u16;
        let row_height = style.row as u16;
        let items = env.menus.items(menu);
        // Labels as wide as the longest needs, at most half the row.
        let font = style.font(env);
        let measure = |text: &str| font.as_ref().map_or(0.0, |f| f.measure(style.text, text));
        let longest = items
            .iter()
            .filter_map(|item| spec(env, item.id))
            .filter(|s| !matches!(s.kind, SettingKind::Bool))
            .map(|s| measure(env.i18n.tr(&s.title)))
            .fold(0.0, f32::max);
        let label_width =
            ((longest as u16).saturating_add(style.gap as u16 * 2)).min((width - 2 * margin) / 2);
        // As many buttons to a row as stay readable.
        let label_of = |control: &Control| -> String {
            match control {
                Control::Toggle(id) => spec(env, id).map_or(String::new(), |s| {
                    alloc::format!("• {}", env.i18n.tr(&s.title))
                }),
                Control::Run(id) => {
                    let title = env
                        .actions
                        .get(id.as_str())
                        .map_or(id.as_str(), |a| a.title.as_ref());
                    env.i18n.tr(title).to_string()
                }
                _ => String::new(),
            }
        };
        let small = |text: &str| font.as_ref().map_or(0.0, |f| f.measure(style.small, text));
        let per_row = |group: &[Control]| -> usize {
            let inner = f32::from(width - 2 * margin);
            (1..=PER_ROW.min(group.len()))
                .rev()
                .find(|&n| {
                    let cell = inner / n as f32 - 2.0 * style.gap as f32;
                    group.iter().all(|c| small(&label_of(c)) <= cell)
                })
                .unwrap_or(1)
        };

        // Rows first at y = 0, moved into place once their number is known.
        let mut rows: Vec<Row> = Vec::new();
        let mut group: Vec<Control> = Vec::new();
        let flush = |group: &mut Vec<Control>, rows: &mut Vec<Row>| {
            if group.is_empty() {
                return;
            }
            let n = per_row(group);
            for chunk in core::mem::take(group).chunks(n) {
                let area = Rect::new(0, 0, width, row_height);
                let cells = split(inset(area, margin), chunk.len(), style.gap as u16 / 2);
                rows.push(Row {
                    area,
                    label: None,
                    controls: chunk.iter().cloned().zip(cells).collect(),
                });
            }
        };
        for item in &items {
            let id = item.id.clone();
            let setting = spec(env, &id).filter(|s| s.editor.is_none());
            let control = match setting.map(|s| &s.kind) {
                Some(SettingKind::Bool) => Some(Control::Toggle(id.clone())),
                None if spec(env, &id).is_none() => Some(Control::Run(id.clone())),
                _ => None,
            };
            if let Some(control) = control {
                // Switches and buttons share rows, but not with each other.
                let same = group.first().is_some_and(|first| {
                    matches!(first, Control::Toggle(_)) == matches!(control, Control::Toggle(_))
                });
                if !group.is_empty() && (!same || group.len() == PER_ROW) {
                    flush(&mut group, &mut rows);
                }
                group.push(control);
                continue;
            }
            flush(&mut group, &mut rows);
            let area = Rect::new(0, 0, width, row_height);
            let inner = inset(area, margin);
            let rest = Rect::new(
                inner.x + label_width,
                inner.y,
                inner.width - label_width,
                inner.height,
            );
            let controls = match spec(env, &id).map(|s| &s.kind) {
                // A wide range: − slider +.
                Some(SettingKind::Int { .. }) if setting.is_some_and(|s| s.slider) => {
                    let size = ui::button_height(area);
                    let y = area.y + (area.height - size) / 2;
                    let minus = Rect::new(rest.x, y, size, size);
                    let plus = Rect::new(rest.x + rest.width - size, y, size, size);
                    let gap = style.gap as u16;
                    let track = Rect::new(
                        minus.x + size + gap,
                        y,
                        rest.width.saturating_sub(2 * (size + gap)),
                        size,
                    );
                    alloc::vec![
                        (Control::Step(id.clone(), -1), minus),
                        (Control::Slide(id.clone()), track),
                        (Control::Step(id.clone(), 1), plus),
                    ]
                }
                // The − and + are placed by the value, as in menus.
                Some(SettingKind::Int { .. }) if setting.is_some() => alloc::vec![
                    (Control::Step(id.clone(), -1), area),
                    (Control::Step(id.clone(), 1), area),
                ],
                Some(SettingKind::Choice(options))
                    if setting.is_some() && fits_segments(env, &style, options, rest) =>
                {
                    let cells = split(rest, options.len(), style.gap as u16 / 2);
                    options
                        .iter()
                        .zip(cells)
                        .map(|(o, r)| (Control::Option(id.clone(), o.to_string()), r))
                        .collect()
                }
                _ => alloc::vec![(Control::Open(id.clone()), inner)],
            };
            rows.push(Row {
                area,
                label: Some(id),
                controls,
            });
        }
        flush(&mut group, &mut rows);

        let border = (style.line * 2).max(2) as u16;
        let gap = style.gap as u16;
        let room = (f32::from(display.height) * MAX_SHARE) as u16 - border - 2 * gap;
        let max_rows = usize::from(room / row_height.max(1)).max(1);
        // Too many rows: pages, with a bar to turn them.
        let paged = rows.len() > max_rows;
        let bar = if paged { ui::Pager::height(&style) } else { 0 };
        let per_page = if paged {
            usize::from((room - bar) / row_height.max(1)).max(1)
        } else {
            rows.len().max(1)
        };
        let height = border + 2 * gap + row_height * per_page as u16 + bar;
        let region = Rect::new(0, display.height - height, width, height);
        for (index, row) in rows.iter_mut().enumerate() {
            let dy = region.y + border + gap + row_height * (index % per_page) as u16;
            row.area.y += dy;
            for (_, rect) in &mut row.controls {
                rect.y += dy;
            }
        }
        let pager = paged.then(|| {
            let area = Rect::new(0, region.y + region.height - bar, width, bar);
            ui::Pager::new(env, area)
        });
        Self {
            rows,
            style,
            region,
            focus: (!env.device.input.touch).then_some(0),
            per_page,
            page: 0,
            pager,
        }
    }

    fn pages(&self) -> usize {
        self.rows.len().div_ceil(self.per_page).max(1)
    }

    /// The rows of the page shown, with the number of controls before them.
    fn visible_rows(&self) -> impl Iterator<Item = (usize, &Row)> {
        let first = self.page * self.per_page;
        let before: usize = self.rows[..first.min(self.rows.len())]
            .iter()
            .map(|r| r.controls.len())
            .sum();
        self.rows
            .iter()
            .skip(first)
            .take(self.per_page)
            .scan(before, |count, row| {
                let at = *count;
                *count += row.controls.len();
                Some((at, row))
            })
    }

    /// The page with control `index`.
    fn page_of(&self, index: usize) -> usize {
        let mut count = 0;
        for (row_index, row) in self.rows.iter().enumerate() {
            count += row.controls.len();
            if index < count {
                return row_index / self.per_page;
            }
        }
        0
    }

    /// Every control in reading order.
    fn controls(&self) -> impl Iterator<Item = &(Control, Rect)> {
        self.rows.iter().flat_map(|row| row.controls.iter())
    }

    /// Where the controls of `row` are now: a number's `−` and `+` follow
    /// its value's width.
    fn placed(&self, env: &Env<'_>, row: &Row) -> Vec<(Control, Rect)> {
        let steps = row.label.as_ref().and_then(|id| spec(env, id)).filter(|s| {
            matches!(s.kind, SettingKind::Int { .. })
                && matches!(row.controls.first(), Some((Control::Step(..), _)))
                && row.controls.len() == 2
        });
        match steps {
            Some(spec) => {
                let value = ui::setting_value(env, spec);
                match ui::stepper(env, &self.style, row.area, &value) {
                    Some((minus, plus)) => alloc::vec![
                        (row.controls[0].0.clone(), minus),
                        (row.controls[1].0.clone(), plus),
                    ],
                    None => Vec::new(),
                }
            }
            None => row.controls.clone(),
        }
    }

    fn activate(&mut self, control: &Control, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        let set = |cx: &mut Cx<'_, '_>, id: &Id, value: SettingValue| {
            if env.settings.get(id.as_str()) != Some(&value) {
                cx.command(Command::SetSetting(id.clone(), value));
            }
        };
        match control {
            Control::Toggle(id) => {
                let on = env.settings.bool(id.as_str());
                set(cx, id, SettingValue::Bool(!on));
            }
            Control::Step(id, step) => {
                if let Some(value) = spec(&env, id).and_then(|s| ui::next_value(&env, s, *step)) {
                    set(cx, id, value);
                }
            }
            Control::Option(id, option) => {
                set(cx, id, SettingValue::Text(option.clone().into()));
            }
            // Set by where it is tapped (see `on_gesture`).
            Control::Slide(_) => {}
            Control::Open(id) => {
                let screen = spec(&env, id)
                    .and_then(|s| s.editor.clone())
                    .unwrap_or(CHOOSE_SCREEN);
                cx.command(Command::OpenScreen(
                    screen,
                    Params::new().with("id", id.as_str().to_string()),
                ));
            }
            Control::Run(id) if is_menu(&env, id) => {
                cx.command(Command::Back);
                cx.command(Command::OpenSlot(
                    slots::MENU,
                    Params::new().with(params::MENU, id.as_str().to_string()),
                ));
            }
            Control::Run(id) => {
                cx.command(Command::Back);
                cx.command(Command::RunAction(id.clone()));
            }
        }
    }

    fn redraw(&self, cx: &mut Cx<'_, '_>) {
        cx.request_redraw();
        cx.request_refresh(RefreshMode::Fast);
    }

    fn draw_control(
        &self,
        canvas: &mut Canvas<'_>,
        env: &Env<'_>,
        control: &Control,
        rect: Rect,
        focused: bool,
    ) {
        let style = self.style;
        let Some(font) = style.font(env) else {
            return;
        };
        let (on, label): (bool, String) = match control {
            Control::Toggle(id) => {
                let name = spec(env, id).map_or("", |s| env.i18n.tr(&s.title));
                let mark = if env.settings.is_scoped(id.as_str()) {
                    "• "
                } else {
                    ""
                };
                (
                    env.settings.bool(id.as_str()),
                    alloc::format!("{mark}{name}"),
                )
            }
            Control::Step(_, step) => (false, String::from(if *step < 0 { "−" } else { "+" })),
            Control::Slide(id) => {
                return draw_slider(canvas, env, &style, id, rect, focused);
            }
            Control::Option(id, option) => (
                env.settings.text(id.as_str()) == option,
                env.i18n.tr(option).to_string(),
            ),
            Control::Open(id) => {
                // A row: the setting's name, its value and an arrow.
                let Some(spec) = spec(env, id) else {
                    return;
                };
                if focused {
                    ui::fill(canvas, rect, Color::BLACK);
                }
                let color = if focused { Color::WHITE } else { Color::BLACK };
                let value = alloc::format!("{} ›", ui::setting_value(env, spec));
                let title = env.i18n.tr(&spec.title);
                let gap = style.gap as f32;
                let width = f32::from(rect.width);
                let (name, value, value_width) =
                    ui::fit_pair(&font, style.text, title, &value, width, gap);
                let right = f32::from(rect.x) + f32::from(rect.width);
                ui::draw_label(
                    canvas,
                    &font,
                    style.text,
                    right - value_width,
                    rect,
                    &value,
                    color,
                );
                ui::draw_label(
                    canvas,
                    &font,
                    style.text,
                    f32::from(rect.x),
                    rect,
                    &name,
                    color,
                );
                return;
            }
            Control::Run(id) => {
                let title = env
                    .actions
                    .get(id.as_str())
                    .map_or(id.as_str(), |a| a.title.as_ref());
                (false, env.i18n.tr(title).to_string())
            }
        };
        ui::draw_button(canvas, env, &style, rect, &label, on, focused);
    }
}

/// A slider: a thin track, filled up to the value, with a knob there.
fn draw_slider(
    canvas: &mut Canvas<'_>,
    env: &Env<'_>,
    style: &UiStyle,
    id: &Id,
    rect: Rect,
    focused: bool,
) {
    let Some(SettingKind::Int { min, max }) = spec(env, id).map(|s| s.kind.clone()) else {
        return;
    };
    let value = env.settings.int(id.as_str()).clamp(min, max);
    let share = (value - min) as f32 / (max - min).max(1) as f32;
    let knob = (rect.height / 2).max(4);
    let track = (style.line * 2).max(2) as u16;
    let mid = rect.y + rect.height / 2;
    let left = rect.x + knob / 2;
    let width = rect.width.saturating_sub(knob);
    let filled = (f32::from(width) * share) as u16;
    ui::fill(
        canvas,
        Rect::new(left, mid - track / 2, width, track),
        Color::gray(170),
    );
    ui::fill(
        canvas,
        Rect::new(left, mid - track, filled, track * 2),
        Color::BLACK,
    );
    let knob_rect = Rect::new(left + filled - knob / 2, mid - knob / 2, knob, knob);
    ui::fill(canvas, knob_rect, Color::BLACK);
    if focused {
        canvas.stroke_rect(
            i32::from(rect.x),
            i32::from(rect.y),
            u32::from(rect.width),
            u32::from(rect.height),
            ui::button_border(style),
            Color::BLACK,
        );
    }
}

/// The value of a slider for a tap at `x`.
fn slider_value(min: i64, max: i64, rect: Rect, x: i32) -> i64 {
    let knob = i32::from((rect.height / 2).max(4));
    let left = i32::from(rect.x) + knob / 2;
    let width = (i32::from(rect.width) - knob).max(1);
    let share = ((x - left) as f32 / width as f32).clamp(0.0, 1.0);
    min + ((max - min) as f32 * share + 0.5) as i64
}

/// Whether a choice's options fit side by side in `area`, readably.
fn fits_segments(
    env: &Env<'_>,
    style: &UiStyle,
    options: &[alloc::borrow::Cow<'static, str>],
    area: Rect,
) -> bool {
    let Some(font) = style.font(env) else {
        return false;
    };
    if options.is_empty() || options.len() > MAX_SEGMENTS {
        return false;
    }
    let cell = f32::from(area.width) / options.len() as f32 - 2.0 * style.gap as f32;
    options
        .iter()
        .all(|o| font.measure(style.small, env.i18n.tr(o)) <= cell)
}

/// `area` without `margin` left and right and a little above and below.
fn inset(area: Rect, margin: u16) -> Rect {
    let pad = (area.height - ui::button_height(area)) / 2;
    Rect::new(
        area.x + margin,
        area.y + pad,
        area.width.saturating_sub(2 * margin),
        area.height.saturating_sub(2 * pad),
    )
}

/// `count` equal parts of `area` side by side, `gap` apart.
fn split(area: Rect, count: usize, gap: u16) -> Vec<Rect> {
    let count = count.max(1) as u16;
    let width = area.width.saturating_sub(gap * (count - 1)) / count;
    (0..count)
        .map(|i| Rect::new(area.x + i * (width + gap), area.y, width, area.height))
        .collect()
}

impl Screen for PanelScreen {
    fn region(&self, _: &Env<'_>) -> Option<Rect> {
        Some(self.region)
    }

    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        let r = self.region;
        ui::fill(canvas, r, Color::WHITE);
        canvas.fill_rect(
            i32::from(r.x),
            i32::from(r.y),
            u32::from(r.width),
            (style.line * 2).max(2),
            Color::BLACK,
        );
        let Some(font) = style.font(env) else {
            return;
        };
        if self.rows.is_empty() {
            ui::draw_message(canvas, env, &style, r, env.i18n.tr("menu.empty"));
            return;
        }
        for (first, row) in self.visible_rows() {
            let label = row.label.as_ref().and_then(|id| spec(env, id));
            // A number with − and + by its value (a slider row has three).
            let number = label.filter(|_| {
                matches!(row.controls.first(), Some((Control::Step(..), _)))
                    && row.controls.len() == 2
            });
            if let Some(spec) = number {
                // As in menus: title, then [−] value [+].
                let focus = match self.focus {
                    Some(f) if f == first => Some(false),
                    Some(f) if f == first + 1 => Some(true),
                    _ => None,
                };
                let title = env.i18n.tr(&spec.title);
                let value = ui::setting_value(env, spec);
                ui::draw_number(canvas, env, &style, row.area, title, &value, &value, focus);
                continue;
            }
            if let Some(spec) = label
                && !matches!(row.controls.first(), Some((Control::Open(_), _)))
            {
                let x = style.margin as f32;
                let first = row.controls.first().map_or(r.width, |(_, c)| c.x);
                let room = f32::from(first) - x - style.gap as f32;
                let name = ui::fit(&font, style.text, env.i18n.tr(&spec.title), room);
                ui::draw_label(canvas, &font, style.text, x, row.area, &name, Color::BLACK);
            }
            for (index, (control, rect)) in (first..).zip(row.controls.iter()) {
                self.draw_control(canvas, env, control, *rect, self.focus == Some(index));
            }
        }
        if let Some(pager) = &self.pager {
            pager.draw(canvas, env, &style, (self.page, self.pages()));
        }
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let count = self.controls().count();
        let a = action;
        if *a == actions::CONFIRM {
            let control = self
                .focus
                .and_then(|f| self.controls().nth(f))
                .map(|(c, _)| c.clone());
            match control {
                Some(control) => self.activate(&control, cx),
                None => {
                    self.focus = Some(0);
                    self.redraw(cx);
                }
            }
            return Handled::Yes;
        }
        if *a == actions::MENU || *a == actions::BACK {
            cx.command(Command::Back);
            return Handled::Yes;
        }
        let back = [
            actions::UP,
            actions::LEFT,
            actions::PREVIOUS,
            actions::PREV_PAGE,
        ];
        let forward = [
            actions::DOWN,
            actions::RIGHT,
            actions::NEXT,
            actions::NEXT_PAGE,
        ];
        let delta: isize = if back.contains(a) {
            -1
        } else if forward.contains(a) {
            1
        } else {
            return Handled::No;
        };
        if count > 0 {
            let current = self.focus.map_or(-1, |f| f as isize);
            let focus = (current + delta).rem_euclid(count as isize) as usize;
            self.focus = Some(focus);
            self.page = self.page_of(focus);
            self.redraw(cx);
        }
        Handled::Yes
    }

    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        if !self.region.contains(gesture.start) {
            return Handled::No;
        }
        if let Some(action) = self.pager.and_then(|p| p.tap(gesture.start)) {
            let pages = self.pages();
            self.page = if action == actions::NEXT_PAGE {
                (self.page + 1).min(pages - 1)
            } else {
                self.page.saturating_sub(1)
            };
            self.focus = None;
            self.redraw(cx);
            return Handled::Yes;
        }
        if gesture.kind == GestureKind::Tap {
            let env = *cx.env();
            let control = self
                .visible_rows()
                .flat_map(|(_, row)| self.placed(&env, row))
                .find(|(_, rect)| rect.contains(gesture.start))
                .map(|(c, _)| c);
            if let Some(control) = control {
                self.focus = None;
                if let Control::Slide(id) = &control
                    && let Some((SettingKind::Int { min, max }, rect)) =
                        spec(&env, id).map(|s| s.kind.clone()).zip(
                            self.controls()
                                .find(|(c, _)| *c == control)
                                .map(|(_, r)| *r),
                        )
                {
                    let value = SettingValue::Int(slider_value(min, max, rect, gesture.start.x));
                    if env.settings.get(id.as_str()) != Some(&value) {
                        cx.command(Command::SetSetting(id.clone(), value));
                    }
                } else {
                    self.activate(&control, cx);
                }
            }
        }
        Handled::Yes
    }

    fn on_event(&mut self, event: &Id, _: &Params, cx: &mut Cx<'_, '_>) {
        // Values shown may have changed.
        if *event == events::SETTING_CHANGED {
            self.redraw(cx);
        }
    }
}

#[cfg(test)]
mod slider_tests {
    use super::*;

    #[test]
    fn a_tap_sets_the_value_where_it_lands() {
        // Knob 15 wide: the track runs from 107 to 192.
        let rect = Rect::new(100, 0, 100, 30);
        assert_eq!(slider_value(0, 255, rect, 50), 0);
        assert_eq!(slider_value(0, 255, rect, 107), 0);
        assert_eq!(slider_value(0, 255, rect, 150), 129);
        assert_eq!(slider_value(0, 255, rect, 192), 255);
        assert_eq!(slider_value(0, 255, rect, 400), 255);
    }
}
