//! Wikipedia: a word looked up online, the short summary of its article,
//! in the language of the book (else of QuireInk). Bound to the slot
//! `core.lookup-online`, so the dictionary offers it beside its own
//! entries when there is a network; another plugin (a translator, an
//! online dictionary) can take its place.
//!
//! The summary shows over the page like a dictionary entry: the page
//! buttons or taps scroll it, confirm or a tap at its end closes it.

#![no_std]

extern crate alloc;

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::{actions, params, slots};
use quireink_sdk::ui::{self, UiStyle};
use quireink_sdk::web::encode;
use quireink_sdk::{
    Canvas, Color, Command, Cx, Env, Gesture, GestureKind, Handled, HttpRequest, Id, Manifest,
    Params, Plugin, Priority, Rect, RefreshMode, RegisterError, Registrar, SDK_VERSION, Screen,
    ScreenSpec, Version, http_params, json,
};

pub const PLUGIN: Id = Id::new_static("wikipedia");
const SCREEN: Id = Id::new_static("wikipedia.lookup");
/// The answer to the summary request.
const ANSWERED: Id = Id::new_static("wikipedia.answered");

/// The wiki's language for `language` (`en-GB` → `en`).
fn wiki(language: &str) -> String {
    let code: String = language
        .split(['-', '_'])
        .next()
        .unwrap_or("en")
        .chars()
        .filter(char::is_ascii_alphabetic)
        .collect::<String>()
        .to_lowercase();
    if code.is_empty() {
        String::from("en")
    } else {
        code
    }
}

/// The address of the summary of `word`'s article.
fn summary_url(language: &str, word: &str) -> String {
    let title = encode(word.trim()).replace('+', "_");
    format!(
        "https://{}.wikipedia.org/api/rest_v1/page/summary/{title}",
        wiki(language)
    )
}

/// The Wikipedia plugin.
#[derive(Debug, Default)]
pub struct Wikipedia;

impl Plugin for Wikipedia {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: PLUGIN,
            name: Cow::Borrowed("Wikipedia"),
            description: Cow::Borrowed("Looks words up online, beside the dictionaries."),
            version: Version::new(0, 1, 0),
            sdk: SDK_VERSION,
        }
    }

    fn register(&self, r: &mut dyn Registrar) -> Result<(), RegisterError> {
        r.add_screen(ScreenSpec {
            id: SCREEN,
            title: Cow::Borrowed("wikipedia.title"),
            factory: Box::new(|env, params| Box::new(Summary::new(env, params))),
        })?;
        r.bind_slot(slots::LOOKUP_ONLINE, SCREEN, Priority::DEFAULT)?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/en.ini")))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/de.ini")))?;
        Ok(())
    }
}

struct Summary {
    style: UiStyle,
    region: Rect,
    word: String,
    language: String,
    /// The summary or a message, and its lines as byte ranges.
    text: String,
    lines: Vec<(usize, usize)>,
    top: usize,
    asked: bool,
}

impl Summary {
    fn new(env: &Env<'_>, params: &Params) -> Self {
        let style = UiStyle::new(env);
        let display = env.device.display;
        let top = display.height * 9 / 20;
        let language = params
            .get(params::LANGUAGE)
            .map_or_else(|| env.i18n.language().to_string(), ToString::to_string);
        let mut summary = Self {
            style,
            region: Rect::new(0, top, display.width, display.height - top),
            word: params.get(params::WORD).unwrap_or_default().to_string(),
            language,
            text: String::new(),
            lines: Vec::new(),
            top: 0,
            asked: false,
        };
        let loading = env.i18n.tr("wikipedia.loading").to_string();
        summary.set_text(env, loading);
        summary
    }

    fn set_text(&mut self, env: &Env<'_>, text: String) {
        self.lines.clear();
        self.top = 0;
        if let Some(font) = self.style.font(env) {
            let width = f32::from(self.region.width) - 2.0 * self.style.margin as f32;
            let mut start = 0;
            for paragraph in text.split('\n') {
                for range in font.wrap(self.style.text, paragraph, width) {
                    self.lines.push((start + range.start, start + range.end));
                }
                start += paragraph.len() + 1;
            }
        }
        self.text = text;
    }

    fn header(&self) -> Rect {
        Rect::new(
            self.region.x,
            self.region.y,
            self.region.width,
            self.style.row as u16,
        )
    }

    fn line_height(&self, env: &Env<'_>) -> f32 {
        self.style.font(env).map_or(self.style.text * 1.3, |f| {
            f.line_metrics(self.style.text).line_height
        })
    }

    fn rows(&self, env: &Env<'_>) -> usize {
        let height =
            f32::from(self.region.height) - f32::from(self.header().height) - self.style.gap as f32;
        ((height / self.line_height(env)) as usize).max(1)
    }

    fn scroll(&mut self, env: &Env<'_>, forward: bool, cx: &mut Cx<'_, '_>) -> bool {
        let rows = self.rows(env);
        let top = if forward {
            let next = self.top + rows;
            if next >= self.lines.len() {
                return false;
            }
            next
        } else {
            if self.top == 0 {
                return false;
            }
            self.top.saturating_sub(rows)
        };
        self.top = top;
        cx.request_redraw();
        cx.request_refresh(RefreshMode::Partial);
        true
    }

    /// The summary arrived: its title and text, or why not.
    fn answered(&mut self, env: &Env<'_>, params: &Params) {
        let status: u16 = params
            .get(http_params::STATUS)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let text = match status {
            200..=299 => {
                let members = json::object(params.get(http_params::BODY).unwrap_or_default());
                match json::text(&members, "extract").filter(|t| !t.trim().is_empty()) {
                    Some(extract) => {
                        if let Some(title) = json::text(&members, "title") {
                            self.word = title.to_string();
                        }
                        extract.to_string()
                    }
                    None => env.i18n.tr("wikipedia.not-found").to_string(),
                }
            }
            404 => env.i18n.tr("wikipedia.not-found").to_string(),
            0 => env.i18n.tr("wikipedia.unreachable").to_string(),
            _ => env.i18n.tr("wikipedia.failed").to_string(),
        };
        self.set_text(env, text);
    }
}

impl Screen for Summary {
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
        let header = self.header();
        let margin = style.margin as f32;
        let source = env.i18n.tr("wikipedia.title");
        let source_width = font.measure(style.small, source);
        let right = f32::from(header.width) - margin - source_width;
        ui::draw_label(
            canvas,
            &font,
            style.small,
            right,
            header,
            source,
            style.faint(80),
        );
        let word = ui::fit(&font, style.title, &self.word, right - margin * 2.0);
        ui::draw_label(
            canvas,
            &font,
            style.title,
            margin,
            header,
            &word,
            Color::BLACK,
        );
        let height = self.line_height(env);
        let ascent = font.line_metrics(style.text).ascent;
        let top = f32::from(header.y + header.height) + style.gap as f32 / 2.0;
        let rows = self.rows(env);
        for (n, &(start, end)) in self.lines.iter().skip(self.top).take(rows).enumerate() {
            let baseline = top + height * n as f32 + ascent;
            font.draw(
                canvas,
                style.text,
                margin,
                baseline as i32,
                &self.text[start..end],
                Color::BLACK,
            );
        }
    }

    fn on_show(&mut self, cx: &mut Cx<'_, '_>) {
        if self.asked {
            return;
        }
        self.asked = true;
        let env = *cx.env();
        if !env.network {
            let text = env.i18n.tr("wikipedia.no-network").to_string();
            self.set_text(&env, text);
            cx.request_redraw();
            return;
        }
        let mut request =
            HttpRequest::new("GET", &summary_url(&self.language, &self.word), ANSWERED);
        request.headers.push((
            String::from("User-Agent"),
            String::from("QuireInk/0.1 (e-reader)"),
        ));
        request
            .headers
            .push((String::from("Accept"), String::from("application/json")));
        cx.command(Command::Http(request));
    }

    fn on_event(&mut self, event: &Id, params: &Params, cx: &mut Cx<'_, '_>) {
        if *event == ANSWERED {
            let env = *cx.env();
            self.answered(&env, params);
            cx.request_redraw();
            cx.request_refresh(RefreshMode::Partial);
        }
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let env = *cx.env();
        let a = action;
        if [
            actions::NEXT_PAGE,
            actions::DOWN,
            actions::RIGHT,
            actions::NEXT,
        ]
        .contains(a)
        {
            self.scroll(&env, true, cx);
        } else if [
            actions::PREV_PAGE,
            actions::UP,
            actions::LEFT,
            actions::PREVIOUS,
        ]
        .contains(a)
        {
            self.scroll(&env, false, cx);
        } else if *a == actions::CONFIRM {
            cx.command(Command::Back);
        } else {
            return Handled::No;
        }
        Handled::Yes
    }

    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        if gesture.kind != GestureKind::Tap || !self.region.contains(gesture.start) {
            return Handled::No;
        }
        let env = *cx.env();
        let forward = gesture.start.x > i32::from(self.region.width) / 3;
        if !self.scroll(&env, forward, cx) && forward {
            cx.command(Command::Back);
        }
        Handled::Yes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quireink_i18n::{Catalog, check};

    #[test]
    fn addresses() {
        assert_eq!(
            summary_url("de-DE", "Hund"),
            "https://de.wikipedia.org/api/rest_v1/page/summary/Hund"
        );
        assert_eq!(
            summary_url("en", "white rabbit"),
            "https://en.wikipedia.org/api/rest_v1/page/summary/white_rabbit"
        );
        assert_eq!(wiki(""), "en");
    }

    #[test]
    fn translations_are_complete() {
        let en = Catalog::parse(include_str!("../i18n/en.ini")).unwrap();
        let de = Catalog::parse(include_str!("../i18n/de.ini")).unwrap();
        let issues = check::compare(&en, &de);
        assert!(issues.is_empty(), "{issues:?}");
    }
}
