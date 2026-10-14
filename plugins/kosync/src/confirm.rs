//! The overlay at the bottom that offers to go where another device is,
//! or tells the outcome of *Sync progress*.
//!
//! Opened with [`DEVICE`], [`PERCENTAGE`], [`XPOINTER`] and the book's
//! path for an offer, or with only `text` for a message.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::{actions, params};
use quireink_sdk::document::{Document, Locator};
use quireink_sdk::ui::{self, UiStyle};
use quireink_sdk::{
    Arg, Canvas, Color, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Params, Rect,
    RefreshMode, Screen,
};

/// The other device's name.
pub(crate) const DEVICE: &str = "device";
/// Its progress from 0 to 1.
pub(crate) const PERCENTAGE: &str = "percentage";
/// Its position as KOReader's XPointer (may be empty).
pub(crate) const XPOINTER: &str = "xpointer";

/// Emitted when the user stays, with the book's path and [`PERCENTAGE`]:
/// that position is not offered again.
pub(crate) const DECLINED: Id = Id::new_static("kosync.declined");

/// The reader's event to show a position.
const GOTO: Id = Id::new_static("reader.goto");

#[derive(Debug)]
pub(crate) struct ConfirmScreen {
    style: UiStyle,
    region: Rect,
    text: String,
    /// The buttons' translation keys; the first one goes there.
    buttons: Vec<&'static str>,
    /// An offer (book, XPointer, progress); `None` for a message.
    offer: Option<(String, String, f64)>,
    focus: usize,
}

impl ConfirmScreen {
    pub(crate) fn new(env: &Env<'_>, params: &Params) -> Self {
        let style = UiStyle::new(env);
        let display = env.device.display;
        let percentage: Option<f64> = params.get(PERCENTAGE).and_then(|p| p.parse().ok());
        let (text, buttons, offer) = match (percentage, params.get(params::PATH)) {
            (Some(percentage), Some(path)) => {
                let percent = format!("{:.0}", percentage * 100.0);
                let text = env.i18n.format(
                    "kosync.further",
                    &[
                        ("device", Arg::Str(params.get(DEVICE).unwrap_or("?"))),
                        ("percent", Arg::Str(&percent)),
                    ],
                );
                let xpointer = params.get(XPOINTER).unwrap_or_default().to_string();
                let offer = (path.to_string(), xpointer, percentage);
                (text, Vec::from(["kosync.go", "kosync.stay"]), Some(offer))
            }
            _ => {
                let text = params.get(params::TEXT).unwrap_or_default().to_string();
                (text, Vec::from(["kosync.ok"]), None)
            }
        };
        let height = (style.row * 4) as u16;
        let region = Rect::new(
            0,
            display.height.saturating_sub(height),
            display.width,
            height,
        );
        Self {
            style,
            region,
            text,
            buttons,
            offer,
            focus: 0,
        }
    }

    /// The row with the buttons.
    fn button_row(&self) -> Rect {
        let r = self.region;
        let row = self.style.row as u16;
        Rect::new(
            r.x,
            r.y + r.height - row - self.style.gap as u16,
            r.width,
            row,
        )
    }

    fn button(&self, index: usize) -> Rect {
        let row = self.button_row();
        let (margin, gap) = (self.style.margin as u16, self.style.gap as u16 * 2);
        let count = self.buttons.len() as u16;
        let width = (row.width - 2 * margin - gap * (count - 1)) / count;
        let height = ui::button_height(row);
        let x = row.x + margin + index as u16 * (width + gap);
        Rect::new(x, row.y + (row.height - height) / 2, width, height)
    }

    fn choose(&mut self, index: usize, cx: &mut Cx<'_, '_>) {
        cx.command(Command::Back);
        if index != 0 {
            if let Some((path, _, percentage)) = &self.offer {
                let params = Params::new()
                    .with(params::PATH, path.clone())
                    .with(PERCENTAGE, format!("{percentage}"));
                cx.command(Command::Emit(DECLINED, params));
            }
            return;
        }
        let env = *cx.env();
        let Some((path, xpointer, percentage)) = &self.offer else {
            return;
        };
        let Ok(document) = env.formats.open(env.storage, path) else {
            return;
        };
        let at = Some(xpointer)
            .filter(|x| !x.is_empty())
            .and_then(|x| document.locate_xpointer(env.storage, x))
            .unwrap_or_else(|| at_progress(&*document, *percentage));
        cx.command(Command::Emit(
            GOTO,
            Params::new()
                .with(params::PATH, path.clone())
                .with(params::SECTION, format!("{}", at.section))
                .with(params::OFFSET, format!("{}", at.offset)),
        ));
    }
}

/// The position at `progress` (0..1) of a document, the inverse of
/// [`Document::progress`].
pub(crate) fn at_progress(document: &dyn Document, progress: f64) -> Locator {
    let lengths: Vec<u64> = (0..document.sections())
        .map(|s| u64::from(document.section_len(s)))
        .collect();
    let total: u64 = lengths.iter().sum();
    let mut left = (progress.clamp(0.0, 1.0) * total as f64) as u64;
    for (section, &len) in lengths.iter().enumerate() {
        if left < len {
            return Locator::new(section as u32, left as u32);
        }
        left -= len;
    }
    // The very end: the start of the last section with text.
    let last = lengths.iter().rposition(|&l| l > 0).unwrap_or(0);
    Locator::new(last as u32, 0)
}

impl Screen for ConfirmScreen {
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
        let row = self.button_row();
        let text = Rect::new(r.x, r.y, r.width, row.y - r.y);
        ui::draw_message(canvas, env, &style, text, &self.text);
        let touch = env.device.input.touch;
        for (index, key) in self.buttons.iter().enumerate() {
            // With buttons, the frame shows which one confirm takes.
            let focused = !touch && index == self.focus && self.buttons.len() > 1;
            let rect = self.button(index);
            ui::draw_button(
                canvas,
                env,
                &style,
                rect,
                env.i18n.tr(key),
                index == 0,
                focused,
            );
        }
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let a = action;
        let last = self.buttons.len() - 1;
        let focus = if *a == actions::LEFT || *a == actions::UP || *a == actions::PREVIOUS {
            self.focus.saturating_sub(1)
        } else if *a == actions::RIGHT || *a == actions::DOWN || *a == actions::NEXT {
            (self.focus + 1).min(last)
        } else if *a == actions::CONFIRM {
            self.choose(self.focus, cx);
            return Handled::Yes;
        } else {
            return Handled::No;
        };
        if focus != self.focus {
            self.focus = focus;
            cx.request_redraw();
            cx.request_refresh(RefreshMode::Fast);
        }
        Handled::Yes
    }

    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        if gesture.kind != GestureKind::Tap {
            return Handled::No;
        }
        let tapped = (0..self.buttons.len()).find(|&i| self.button(i).contains(gesture.start));
        if let Some(index) = tapped {
            self.choose(index, cx);
        }
        Handled::Yes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quireink_sdk::Storage;
    use quireink_sdk::document::{Block, DocumentError, Metadata};

    struct Sections(Vec<u32>, Metadata);

    impl Document for Sections {
        fn metadata(&self) -> &Metadata {
            &self.1
        }
        fn sections(&self) -> u32 {
            self.0.len() as u32
        }
        fn section_len(&self, section: u32) -> u32 {
            self.0[section as usize]
        }
        fn block_at(&self, _: &dyn Storage, _: Locator) -> Result<Option<Block>, DocumentError> {
            Ok(None)
        }
    }

    #[test]
    fn progress_maps_back_to_positions() {
        let document = Sections(Vec::from([100, 0, 300]), Metadata::default());
        assert_eq!(at_progress(&document, 0.0), Locator::new(0, 0));
        assert_eq!(at_progress(&document, 0.2), Locator::new(0, 80));
        assert_eq!(at_progress(&document, 0.5), Locator::new(2, 100));
        assert_eq!(at_progress(&document, 1.0), Locator::new(2, 0));
        let at = Locator::new(2, 100);
        assert_eq!(at_progress(&document, f64::from(document.progress(at))), at);
    }
}
