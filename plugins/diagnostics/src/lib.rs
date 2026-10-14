//! A diagnostics screen for bringing up new devices and for testing the
//! kernel: it shows every gray level of the panel, exercises button and
//! touch focus navigation, and demonstrates partial versus full refreshes.
//!
//! It binds itself to the home slot with the lowest regular priority, so it
//! only appears until a real home screen plugin is installed — or when a
//! user binds it explicitly:
//!
//! ```ini
//! [slots]
//! core.home = diagnostics.screen
//! ```

#![no_std]

extern crate alloc;

use alloc::borrow::Cow;
use alloc::boxed::Box;

use quireink_sdk::builtin::{actions, fonts, slots};
use quireink_sdk::{
    ActionSpec, Arg, Canvas, Color, Cx, Env, Gesture, GestureKind, Handled, Id, Manifest, Metrics,
    Plugin, Priority, Rect, RefreshMode, RegisterError, Registrar, SDK_VERSION, Screen, ScreenSpec,
    SettingKind, SettingSpec, SettingValue, Version,
};

pub const PLUGIN: Id = Id::new_static("diagnostics");
pub const SCREEN: Id = Id::new_static("diagnostics.screen");
pub const RESET: Id = Id::new_static("diagnostics.reset");
pub const PAGES: Id = Id::new_static("diagnostics.pages");

/// The diagnostics plugin.
#[derive(Debug, Default)]
pub struct Diagnostics;

impl Plugin for Diagnostics {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: PLUGIN,
            name: Cow::Borrowed("Diagnostics"),
            version: Version::new(0, 1, 0),
            sdk: SDK_VERSION,
        }
    }

    fn register(&self, r: &mut dyn Registrar) -> Result<(), RegisterError> {
        r.add_screen(ScreenSpec {
            id: SCREEN,
            title: Cow::Borrowed("diagnostics.title"),
            factory: Box::new(|env| Box::new(DiagnosticsScreen::new(env))),
        })?;
        r.bind_slot(slots::HOME, SCREEN, Priority::FALLBACK)?;
        r.add_action(ActionSpec {
            id: RESET,
            title: Cow::Borrowed("diagnostics.action.reset"),
            handler: None,
        })?;
        r.add_setting(SettingSpec {
            id: PAGES,
            title: Cow::Borrowed("diagnostics.setting.pages"),
            kind: SettingKind::Int {
                min: 1,
                max: 10_000,
            },
            default: SettingValue::Int(20),
        })?;
        r.add_keymap(Cow::Borrowed(include_str!("../keymap.ini")))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/en.ini")))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/de.ini")))?;
        Ok(())
    }
}

/// Screen regions, computed once per screen size.
#[derive(Debug, Clone, Copy)]
struct Layout {
    frame: Rect,
    header: Rect,
    ramp: Rect,
    boxes: [Rect; 4],
    /// Page counter text above the progress bar.
    label: Rect,
    progress: Rect,
}

impl Layout {
    fn new(width: u16, height: u16, metrics: Metrics) -> Self {
        let px = |mm| metrics.mm(mm).min(u32::from(u16::MAX)) as u16;
        let margin = px(3);
        let gap = px(4);
        let inner_w = width.saturating_sub(2 * margin);
        let frame = Rect::new(margin, margin, inner_w, height.saturating_sub(2 * margin));

        let content_x = margin + gap;
        let content_w = inner_w.saturating_sub(2 * gap);
        let header = Rect::new(content_x, margin + gap, content_w, px(8));
        let ramp = Rect::new(content_x, header.y + header.height + gap, content_w, px(10));

        let progress_h = px(6);
        let progress_y = (frame.y + frame.height).saturating_sub(gap + progress_h);
        let progress = Rect::new(content_x, progress_y, content_w, progress_h);
        let label_h = px(7);
        let label_y = progress_y.saturating_sub(label_h);
        let label = Rect::new(content_x, label_y, content_w, label_h);

        let grid_y = ramp.y + ramp.height + gap;
        let grid_h = label_y.saturating_sub(grid_y + gap / 2);
        let box_w = content_w.saturating_sub(gap) / 2;
        let box_h = grid_h.saturating_sub(gap) / 2;
        let cell = |col: u16, row: u16| {
            Rect::new(
                content_x + col * (box_w + gap),
                grid_y + row * (box_h + gap),
                box_w,
                box_h,
            )
        };
        Self {
            frame,
            header,
            ramp,
            boxes: [cell(0, 0), cell(1, 0), cell(0, 1), cell(1, 1)],
            label,
            progress,
        }
    }
}

#[derive(Debug)]
struct DiagnosticsScreen {
    layout: Layout,
    metrics: Metrics,
    page: u32,
    focus: usize,
    checked: [bool; 4],
}

impl DiagnosticsScreen {
    fn new(env: &Env<'_>) -> Self {
        let display = env.device.display;
        Self {
            layout: Layout::new(display.width, display.height, env.metrics),
            metrics: env.metrics,
            page: 0,
            focus: 0,
            checked: [false; 4],
        }
    }

    fn pages(env: &Env<'_>) -> u32 {
        env.settings
            .int(PAGES.as_str())
            .clamp(1, i64::from(u32::MAX)) as u32
    }

    fn draw_box(&self, canvas: &mut Canvas<'_>, index: usize) {
        let r = self.layout.boxes[index];
        let (x, y, w, h) = (
            i32::from(r.x),
            i32::from(r.y),
            u32::from(r.width),
            u32::from(r.height),
        );
        canvas.fill_rect(x, y, w, h, Color::WHITE);
        if self.checked[index] {
            let inset = self.metrics.mm(3);
            canvas.fill_rect(
                x + inset as i32,
                y + inset as i32,
                w.saturating_sub(2 * inset),
                h.saturating_sub(2 * inset),
                Color::gray(64),
            );
        }
        let border = if index == self.focus {
            self.metrics.tenth_mm(12)
        } else {
            self.metrics.tenth_mm(3)
        };
        canvas.stroke_rect(x, y, w, h, border, Color::BLACK);
    }

    /// Black bar with the translated title.
    fn draw_header(&self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let r = self.layout.header;
        let (x, y, w, h) = (
            i32::from(r.x),
            i32::from(r.y),
            u32::from(r.width),
            u32::from(r.height),
        );
        canvas.fill_rect(x, y, w, h, Color::BLACK);
        let title = env.i18n.tr("diagnostics.title");
        let padding = self.metrics.mm(2) as f32;
        draw_label(
            &mut canvas.clipped(r),
            env,
            r,
            padding,
            title,
            Color::WHITE,
            false,
        );
    }

    /// "Page x of y", centred above the progress bar.
    fn draw_page_label(&self, canvas: &mut Canvas<'_>, env: &Env<'_>, pages: u32) {
        let r = self.layout.label;
        canvas.fill_rect(
            i32::from(r.x),
            i32::from(r.y),
            u32::from(r.width),
            u32::from(r.height),
            Color::WHITE,
        );
        let text = env.i18n.format(
            "diagnostics.page",
            &[
                ("page", Arg::Int(i64::from(self.page) + 1)),
                ("pages", Arg::Int(i64::from(pages))),
            ],
        );
        draw_label(
            &mut canvas.clipped(r),
            env,
            r,
            0.0,
            &text,
            Color::BLACK,
            true,
        );
    }

    fn draw_progress(&self, canvas: &mut Canvas<'_>, pages: u32) {
        let r = self.layout.progress;
        let (x, y, w, h) = (
            i32::from(r.x),
            i32::from(r.y),
            u32::from(r.width),
            u32::from(r.height),
        );
        canvas.fill_rect(x, y, w, h, Color::WHITE);
        let border = self.metrics.tenth_mm(3);
        canvas.stroke_rect(x, y, w, h, border, Color::BLACK);
        let inner = w.saturating_sub(2 * border);
        let filled = if pages <= 1 {
            inner
        } else {
            (u64::from(inner) * u64::from(self.page) / u64::from(pages - 1)) as u32
        };
        canvas.fill_rect(
            x + border as i32,
            y + border as i32,
            filled,
            h.saturating_sub(2 * border),
            Color::BLACK,
        );
    }

    /// Left/right step through all boxes (enough for devices with only two
    /// navigation buttons); up/down switch rows.
    fn move_focus(&mut self, action: &Id) -> Option<usize> {
        let focus = self.focus;
        match action {
            a if *a == actions::LEFT => Some((focus + 3) % 4),
            a if *a == actions::RIGHT => Some((focus + 1) % 4),
            a if *a == actions::UP || *a == actions::DOWN => Some((focus + 2) % 4),
            _ => None,
        }
    }

    fn set_focus(&mut self, focus: usize, cx: &mut Cx<'_, '_>) {
        let previous = core::mem::replace(&mut self.focus, focus);
        self.draw_box(cx.canvas(), previous);
        self.draw_box(cx.canvas(), focus);
        // Focus feedback should be quick; ghosting is cleaned up later.
        cx.request_refresh(RefreshMode::Fast);
    }

    fn toggle(&mut self, index: usize, cx: &mut Cx<'_, '_>) {
        self.checked[index] = !self.checked[index];
        self.draw_box(cx.canvas(), index);
        cx.request_refresh(RefreshMode::Partial);
    }
}

impl Screen for DiagnosticsScreen {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let l = self.layout;
        let rect = |r: Rect| {
            (
                i32::from(r.x),
                i32::from(r.y),
                u32::from(r.width),
                u32::from(r.height),
            )
        };

        let (x, y, w, h) = rect(l.frame);
        canvas.stroke_rect(x, y, w, h, self.metrics.tenth_mm(5), Color::BLACK);

        self.draw_header(canvas, env);

        // One stripe per gray level the panel can show (at most 16).
        let levels = env
            .device
            .display
            .format
            .gray_levels()
            .unwrap_or(16)
            .clamp(2, 16) as u32;
        let (x, y, w, mut h) = rect(l.ramp);
        if env.device.display.format.is_color() {
            // Lower half: primary and secondary colours for colour panels.
            h /= 2;
            let hues = [
                Color::rgb(255, 0, 0),
                Color::rgb(255, 255, 0),
                Color::rgb(0, 255, 0),
                Color::rgb(0, 255, 255),
                Color::rgb(0, 0, 255),
                Color::rgb(255, 0, 255),
            ];
            for (i, hue) in hues.into_iter().enumerate() {
                let i = i as u32;
                let x0 = x + (w * i / 6) as i32;
                let x1 = x + (w * (i + 1) / 6) as i32;
                canvas.fill_rect(x0, y + h as i32, (x1 - x0) as u32, h, hue);
            }
        }
        for level in 0..levels {
            let x0 = x + (w * level / levels) as i32;
            let x1 = x + (w * (level + 1) / levels) as i32;
            let gray = (level * 255 / (levels - 1)) as u8;
            canvas.fill_rect(x0, y, (x1 - x0) as u32, h, Color::gray(gray));
        }
        let (x, y, w, h) = rect(l.ramp);
        canvas.stroke_rect(x, y, w, h, self.metrics.tenth_mm(3), Color::BLACK);

        for index in 0..4 {
            self.draw_box(canvas, index);
        }
        self.draw_page_label(canvas, env, Self::pages(env));
        self.draw_progress(canvas, Self::pages(env));
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let pages = Self::pages(cx.env());
        if *action == actions::NEXT_PAGE || *action == actions::PREV_PAGE {
            let page = if *action == actions::NEXT_PAGE {
                (self.page + 1).min(pages - 1)
            } else {
                self.page.saturating_sub(1)
            };
            if page != self.page {
                self.page = page;
                let env = *cx.env();
                self.draw_page_label(cx.canvas(), &env, pages);
                self.draw_progress(cx.canvas(), pages);
                cx.request_refresh(RefreshMode::Partial);
            }
            Handled::Yes
        } else if *action == actions::CONFIRM {
            self.toggle(self.focus, cx);
            Handled::Yes
        } else if *action == RESET {
            self.page = 0;
            self.focus = 0;
            self.checked = [false; 4];
            cx.request_redraw();
            cx.request_refresh(RefreshMode::Full);
            Handled::Yes
        } else if let Some(focus) = self.move_focus(action) {
            if focus != self.focus {
                self.set_focus(focus, cx);
            }
            Handled::Yes
        } else {
            Handled::No
        }
    }

    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        if gesture.kind != GestureKind::Tap {
            return Handled::No;
        }
        let Some(index) = self
            .layout
            .boxes
            .iter()
            .position(|b| b.contains(gesture.start))
        else {
            return Handled::No;
        };
        if index != self.focus {
            self.set_focus(index, cx);
        }
        self.toggle(index, cx);
        Handled::Yes
    }
}

/// Draws one line of UI text vertically centred in `area`, either left
/// aligned (with `padding`) or centred.
fn draw_label(
    canvas: &mut Canvas<'_>,
    env: &Env<'_>,
    area: Rect,
    padding: f32,
    text: &str,
    color: Color,
    centered: bool,
) {
    let Some(font) = env.fonts.role(fonts::UI.as_str()) else {
        return;
    };
    let size = f32::from(area.height) * 0.5;
    let metrics = font.line_metrics(size);
    let baseline = i32::from(area.y)
        + ((f32::from(area.height) + metrics.ascent - metrics.descent) / 2.0) as i32;
    let x = if centered {
        f32::from(area.x) + (f32::from(area.width) - font.measure(size, text)) / 2.0
    } else {
        f32::from(area.x) + padding
    };
    font.draw(canvas, size, x, baseline, text, color);
}

#[cfg(test)]
mod tests {
    use super::*;
    use quireink_i18n::{Catalog, check};

    #[test]
    fn translations_are_complete() {
        let en = Catalog::parse(include_str!("../i18n/en.ini")).unwrap();
        let de = Catalog::parse(include_str!("../i18n/de.ini")).unwrap();
        let issues = check::compare(&en, &de);
        assert!(issues.is_empty(), "{issues:?}");
    }

    #[test]
    fn layout_fits_small_and_large_screens() {
        for (w, h, dpi) in [(480, 800, 220), (1264, 1680, 300), (300, 400, 167)] {
            let l = Layout::new(w, h, Metrics::new(dpi));
            for rect in [l.header, l.ramp, l.progress].iter().chain(&l.boxes) {
                assert!(rect.right() <= u32::from(w) && rect.bottom() <= u32::from(h));
            }
        }
    }
}
