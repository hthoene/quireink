//! A bar chart for e-ink: black bars on a thin base line, a label below
//! each, the value of the highest above it.

use quireink_sdk::ui::{self, UiStyle};
use quireink_sdk::{Canvas, Color, Env, Rect};

/// Bars are never wider than those of a week.
const MIN_SLOTS: usize = 7;

/// Draws `values` as bars in `area` with `labels` below them; the bar
/// `highlight` (e.g. today) is framed. `top` labels the highest bar. A
/// few values keep the width of a week's bars, at the right (the newest).
pub(crate) fn bars(
    canvas: &mut Canvas<'_>,
    env: &Env<'_>,
    style: &UiStyle,
    area: Rect,
    values: &[u32],
    labels: &[&str],
    (highlight, top): (Option<usize>, &str),
) {
    let Some(font) = style.font(env) else {
        return;
    };
    if values.is_empty() {
        return;
    }
    let label_h = (style.small * 1.4) as u16;
    let top_h = label_h;
    let chart = Rect::new(
        area.x,
        area.y + top_h,
        area.width,
        area.height.saturating_sub(label_h + top_h),
    );
    let max = values.iter().copied().max().unwrap_or(0).max(1);
    let slots = values.len().max(MIN_SLOTS);
    let slot = f32::from(chart.width) / slots as f32;
    let shift = slot * (slots - values.len()) as f32;
    let bar_w = (slot * 0.6).max(2.0);
    let base_y = i32::from(chart.y + chart.height);
    let line = style.line.max(1);
    canvas.fill_rect(
        i32::from(chart.x),
        base_y,
        u32::from(chart.width),
        line,
        Color::gray(150),
    );
    // Labels wider than their bar's room: every other one, counted from
    // the last (today).
    let widest = labels
        .iter()
        .map(|l| font.measure(style.small, l))
        .fold(0.0f32, f32::max);
    let every = if widest > slot * 0.9 { 2 } else { 1 };
    for (i, &value) in values.iter().enumerate() {
        let x = f32::from(chart.x) + shift + slot * i as f32 + (slot - bar_w) / 2.0;
        let h = (f32::from(chart.height) * value as f32 / max as f32) as i32;
        if value > 0 {
            // At least visible.
            let h = h.max(line as i32 * 2);
            canvas.fill_rect(x as i32, base_y - h, bar_w as u32, h as u32, Color::BLACK);
            if value == max && !top.is_empty() {
                let w = font.measure(style.small, top);
                let tx = (x + bar_w / 2.0 - w / 2.0)
                    .clamp(f32::from(area.x), f32::from(area.x + area.width) - w);
                let row = Rect::new(area.x, (base_y - h) as u16 - top_h, area.width, top_h);
                ui::draw_label(canvas, &font, style.small, tx, row, top, style.faint(60));
            }
        }
        if highlight == Some(i) {
            let pad = (slot - bar_w) / 4.0;
            canvas.stroke_rect(
                (x - pad) as i32,
                i32::from(chart.y),
                (bar_w + 2.0 * pad) as u32,
                u32::from(chart.height) + line,
                line,
                Color::gray(120),
            );
        }
        let shown = (values.len() - 1 - i) % every == 0;
        if let Some(label) = labels.get(i).filter(|_| shown) {
            let w = font.measure(style.small, label);
            let lx = x + bar_w / 2.0 - w / 2.0;
            let row = Rect::new(area.x, base_y as u16 + line as u16, area.width, label_h);
            let color = if highlight == Some(i) {
                Color::BLACK
            } else {
                style.faint(90)
            };
            ui::draw_label(canvas, &font, style.small, lx, row, label, color);
        }
    }
}
