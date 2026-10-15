//! A menu as a bar at the top of the screen, as the reader opens its
//! menu: a title and a line about where one is (e.g. the book, chapter and
//! page), then the entries as icons with short labels, several to a row.
//!
//! Opened like any menu, with the parameter `mode` set to `bar`, and
//! optionally `title` and `text` for the two lines, and `page` shown at the
//! end of the second (always whole; the text is shortened instead); with
//! `action`, tapping the two lines runs it (e.g. the book's information).
//! Labels are the
//! translation of an entry's title key followed by `.short` where there is
//! one (`reader.toc.short` = "Contents"), else its title.
//!
//! Switches show their state (an icon on black is on) and change in
//! place; choices open their list; submenus open as a list below; actions
//! close the bar and run.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::{actions, events, params};
use quireink_sdk::ui::{self, UiStyle};
use quireink_sdk::{
    Canvas, Color, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Params, Rect, RefreshMode,
    Screen, SettingKind, SettingValue,
};

use crate::{CHOOSE_SCREEN, DEPTH, SCREEN, is_menu};

/// The value of [`params::MODE`] for a bar.
pub(crate) const MODE: &str = "bar";

/// Icons in a bar are this much larger than body text.
const ICON_SCALE: f32 = 1.6;

/// An entry of the bar.
#[derive(Debug)]
struct Entry {
    id: Id,
    label: String,
    icon: Option<String>,
}

#[derive(Debug)]
pub(crate) struct BarScreen {
    entries: Vec<Entry>,
    style: UiStyle,
    region: Rect,
    title: String,
    text: String,
    page: String,
    /// Where the title and text are.
    info: Rect,
    /// Run when they are tapped.
    info_action: Option<Id>,
    /// Where each entry is drawn (as wide as its icon or label).
    cells: Vec<Rect>,
    /// Where each entry is tapped.
    targets: Vec<Rect>,
    focus: usize,
    /// The focus is shown (buttons used, or no touch screen).
    keys: bool,
}

impl BarScreen {
    pub(crate) fn new(env: &Env<'_>, params: &Params) -> Self {
        let style = UiStyle::new(env);
        let display = env.device.display;
        let menu = params.get(params::MENU).unwrap_or_default();
        let entries: Vec<Entry> = env
            .menus
            .items(menu)
            .into_iter()
            .filter(|item| !item.section && !quireink_sdk::Menus::hidden(params, item.id.as_str()))
            .map(|item| Entry {
                id: item.id.clone(),
                label: env
                    .i18n
                    .get(&alloc::format!("{}.short", item.title))
                    .unwrap_or(env.i18n.tr(item.title))
                    .to_string(),
                icon: item.icon.map(String::from),
            })
            .collect();
        let title = params.get(params::TITLE).unwrap_or_default().to_string();
        let text = params.get(params::TEXT).unwrap_or_default().to_string();
        let page = params.get(params::PAGE).unwrap_or_default().to_string();
        let (line, small) = style.font(env).map_or((style.text, style.small), |font| {
            (
                font.line_metrics(style.text).line_height,
                font.line_metrics(style.small).line_height,
            )
        });
        let gap = style.gap as u16;
        let info_height = if title.is_empty() && text.is_empty() && page.is_empty() {
            0
        } else {
            (line + small) as u16 + gap
        };
        let info = Rect::new(0, gap, display.width, info_height);
        // Columns as wide as their entries, justified between the margins:
        // the first starts under the title, the last ends with the page.
        // As many to a row as fit with a gap between them.
        let margin = style.margin as u16;
        let room = f32::from(display.width.saturating_sub(2 * margin));
        let size = style.text * ICON_SCALE;
        let widths: Vec<f32> = entries
            .iter()
            .map(|e| {
                let label = style
                    .font(env)
                    .map_or(0.0, |f| f.measure(style.small, &e.label));
                let icon = e
                    .icon
                    .as_deref()
                    .map_or(0.0, |i| ui::icon_width(env, i, size));
                (label.max(icon) + 0.99) as u32 as f32
            })
            .collect();
        let least_gap = f32::from(gap) * 2.0;
        let count = entries.len().max(1);
        let column_widths = |columns: usize| -> Vec<f32> {
            (0..columns)
                .map(|c| {
                    widths
                        .iter()
                        .skip(c)
                        .step_by(columns)
                        .fold(0.0f32, |a, &w| a.max(w))
                })
                .collect()
        };
        let columns = (1..=count)
            .rev()
            .find(|&n| column_widths(n).iter().sum::<f32>() + least_gap * (n - 1) as f32 <= room)
            .unwrap_or(1);
        // Rows filled evenly (four and three rather than five and two).
        let rows = count.div_ceil(columns);
        let columns = count.div_ceil(rows);
        let column_width = column_widths(columns);
        let spare = (room - column_width.iter().sum::<f32>()).max(0.0);
        let between = if columns > 1 {
            spare / (columns - 1) as f32
        } else {
            0.0
        };
        let lefts: Vec<f32> = column_width
            .iter()
            .scan(f32::from(margin), |x, &w| {
                let left = *x;
                *x += w + between;
                Some(left)
            })
            .collect();
        let cell_height = (size + small) as u16 + 2 * gap;
        let top = info.y + info.height + gap;
        let cells: Vec<Rect> = (0..entries.len())
            .map(|i| {
                let (row, column) = (i / columns, i % columns);
                Rect::new(
                    lefts[column] as u16,
                    top + row as u16 * cell_height,
                    column_width[column] as u16,
                    cell_height,
                )
            })
            .collect();
        // Taps count up to halfway to the next entry.
        let half = (between / 2.0) as u16;
        let targets = cells
            .iter()
            .map(|c| Rect::new(c.x.saturating_sub(half), c.y, c.width + 2 * half, c.height))
            .collect();
        let border = (style.line * 2) as u16;
        let height = top + rows as u16 * cell_height + gap + border;
        Self {
            entries,
            region: Rect::new(0, 0, display.width, height.min(display.height)),
            title,
            text,
            page,
            info,
            info_action: params.get(params::ACTION).and_then(|a| Id::parse(a).ok()),
            cells,
            targets,
            focus: 0,
            keys: !env.device.input.touch,
            style,
        }
    }

    /// Whether entry `index` is a switch that is on.
    fn on(env: &Env<'_>, id: &Id) -> bool {
        env.settings
            .specs()
            .find(|spec| spec.id == *id)
            .is_some_and(|spec| spec.kind == SettingKind::Bool && env.settings.bool(id.as_str()))
    }

    fn run(&mut self, index: usize, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        let Some(entry) = self.entries.get(index) else {
            return;
        };
        let id = entry.id.clone();
        if is_menu(&env, &id) {
            cx.command(Command::OpenScreen(
                SCREEN,
                Params::new()
                    .with(params::MENU, id.as_str().to_string())
                    .with(DEPTH, "1"),
            ));
            return;
        }
        let Some(spec) = env.settings.specs().find(|spec| spec.id == id) else {
            cx.command(Command::Back);
            cx.command(Command::RunAction(id));
            return;
        };
        match &spec.kind {
            SettingKind::Bool => {
                let value = SettingValue::Bool(!env.settings.bool(id.as_str()));
                cx.command(Command::SetSetting(id, value));
                cx.request_redraw();
                cx.request_refresh(RefreshMode::Fast);
            }
            SettingKind::Choice(_) | SettingKind::Language => {
                cx.command(Command::OpenScreen(
                    CHOOSE_SCREEN,
                    Params::new().with("id", id.as_str().to_string()),
                ));
            }
            _ => {}
        }
    }

    fn move_focus(&mut self, delta: isize, cx: &mut Cx<'_, '_>) {
        if !self.keys {
            // The first button press shows where the focus is.
            self.keys = true;
        } else {
            let last = self.entries.len().saturating_sub(1) as isize;
            self.focus = (self.focus as isize + delta).clamp(0, last) as usize;
        }
        cx.request_redraw_area(self.region);
        cx.request_refresh(RefreshMode::Fast);
    }
}

impl Screen for BarScreen {
    fn region(&self, _: &Env<'_>) -> Option<Rect> {
        Some(self.region)
    }

    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        let r = self.region;
        ui::fill(canvas, r, Color::WHITE);
        let border = (style.line * 2) as u16;
        ui::fill(
            canvas,
            Rect::new(r.x, r.y + r.height - border, r.width, border),
            Color::BLACK,
        );
        let Some(font) = style.font(env) else {
            return;
        };
        let margin = style.margin as f32;
        if self.info.height > 0 {
            let line = font.line_metrics(style.text).line_height as u16;
            let title = Rect::new(0, self.info.y, r.width, line);
            let text = Rect::new(0, self.info.y + line, r.width, self.info.height - line);
            ui::draw_label(
                canvas,
                &font,
                style.text,
                margin,
                title,
                &self.title,
                Color::BLACK,
            );
            let width = f32::from(r.width) - 2.0 * margin;
            let (chapter, page, page_width) =
                ui::fit_pair(&font, style.small, &self.text, &self.page, width, margin);
            let gray = Color::gray(70);
            ui::draw_label(canvas, &font, style.small, margin, text, &chapter, gray);
            let x = f32::from(r.width) - margin - page_width;
            ui::draw_label(canvas, &font, style.small, x, text, &page, gray);
        }
        let size = style.text * ICON_SCALE;
        let small_line = font.line_metrics(style.small).line_height as u16;
        for (i, (entry, cell)) in self.entries.iter().zip(&self.cells).enumerate() {
            let on = Self::on(env, &entry.id);
            let inner = *cell;
            // Switched on or focused: a frame a little around the entry.
            let pad = style.gap as u16 / 2;
            let frame = Rect::new(
                inner.x.saturating_sub(pad),
                inner.y,
                inner.width + 2 * pad,
                inner.height,
            );
            if on {
                ui::fill(canvas, frame, Color::BLACK);
            }
            if self.keys && i == self.focus {
                canvas.stroke_rect(
                    i32::from(frame.x),
                    i32::from(frame.y),
                    u32::from(frame.width),
                    u32::from(frame.height),
                    ui::button_border(&style),
                    Color::BLACK,
                );
            }
            let ink = if on { Color::WHITE } else { Color::BLACK };
            let icon_area = Rect::new(
                inner.x,
                inner.y + style.gap as u16,
                inner.width,
                size as u16,
            );
            if let Some(icon) = &entry.icon {
                let width = ui::icon_width(env, icon, size);
                let x = f32::from(inner.x) + (f32::from(inner.width) - width) / 2.0;
                ui::draw_icon(canvas, env, icon, size, x, icon_area, ink);
            }
            let label_area = Rect::new(
                inner.x,
                icon_area.y + icon_area.height,
                inner.width,
                small_line,
            );
            let room = f32::from(inner.width) + 1.0;
            let label = ui::fit(&font, style.small, &entry.label, room);
            let x = f32::from(inner.x)
                + (f32::from(inner.width) - font.measure(style.small, &label)) / 2.0;
            ui::draw_label(canvas, &font, style.small, x, label_area, &label, ink);
        }
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let a = action;
        // Turning pages (also by swiping) does not show the focus on
        // touch screens; directions do.
        if !self.keys && (*a == actions::NEXT_PAGE || *a == actions::PREV_PAGE) {
            return Handled::Yes;
        }
        if [
            actions::LEFT,
            actions::UP,
            actions::PREVIOUS,
            actions::PREV_PAGE,
        ]
        .contains(a)
        {
            self.move_focus(-1, cx);
        } else if [
            actions::RIGHT,
            actions::DOWN,
            actions::NEXT,
            actions::NEXT_PAGE,
        ]
        .contains(a)
        {
            self.move_focus(1, cx);
        } else if *a == actions::CONFIRM {
            self.run(self.focus, cx);
        } else if *a == actions::MENU {
            cx.command(Command::Back);
        } else {
            return Handled::No;
        }
        Handled::Yes
    }

    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        if gesture.kind != GestureKind::Tap {
            return Handled::No;
        }
        if let Some(index) = self.targets.iter().position(|c| c.contains(gesture.start)) {
            self.focus = index;
            self.run(index, cx);
        } else if let Some(action) = self.info_action.clone()
            && self.info.contains(gesture.start)
        {
            cx.command(Command::Back);
            cx.command(Command::RunAction(action));
        }
        Handled::Yes
    }

    fn on_event(&mut self, event: &Id, _: &Params, cx: &mut Cx<'_, '_>) {
        // A switch shown here may have changed.
        if *event == events::SETTING_CHANGED {
            cx.request_redraw_area(self.region);
        }
    }
}
