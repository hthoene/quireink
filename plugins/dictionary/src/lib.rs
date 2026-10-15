//! The dictionary: looks up a word in the StarDict dictionaries of a
//! folder and shows the definitions in a panel over the lower part of the
//! screen.
//!
//! Bound to the `core.lookup` slot, which the reader opens for a word
//! (long press, or *Look up a word* in its menu). Dictionaries are folders
//! with `.ifo`, `.idx`, `.dict(.dz)` and optional `.syn` files, as used by
//! KOReader, GoldenDict and many e-readers; they are found anywhere below
//! the dictionary folder. The first lookup prepares a small index for each
//! dictionary in the background; lookups then take a few milliseconds and
//! little memory (see `quireink-stardict`).
//!
//! Buttons: next/previous page or up/down scroll long definitions, confirm
//! or back closes. Touch: tap the right part to scroll, the left part to go
//! back, outside to close.

#![no_std]

extern crate alloc;

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::{actions, groups, params, slots};
use quireink_sdk::ui::{self, UiStyle};
use quireink_sdk::{
    Arg, Canvas, Color, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Manifest, Params,
    Plugin, Priority, Rect, RefreshMode, RegisterError, Registrar, SDK_VERSION, Screen, ScreenSpec,
    SettingKind, SettingSpec, SettingValue, Storage, Version,
};
use quireink_stardict::{Builder, Dictionary};

pub const PLUGIN: Id = Id::new_static("dictionary");
/// The panel with the definitions; parameters `word` and `language`.
pub const POPUP: Id = Id::new_static("dictionary.popup");
/// The folder searched for dictionaries.
pub const FOLDER: Id = Id::new_static("dictionary.folder");
/// The folder chooser of the `files` plugin, if installed.
const CHOOSE_FOLDER: Id = Id::new_static("files.choose-folder");

/// Bytes of an index read per background step.
const STEP_BYTES: usize = 64 * 1024;
/// Folders below the dictionary folder searched for dictionaries.
const MAX_DEPTH: usize = 3;

/// The dictionary plugin.
#[derive(Debug, Default)]
pub struct DictionaryPlugin;

impl Plugin for DictionaryPlugin {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: PLUGIN,
            name: Cow::Borrowed("Dictionary"),
            description: Cow::Borrowed("Looks up words in StarDict dictionaries."),
            version: Version::new(0, 1, 0),
            sdk: SDK_VERSION,
        }
    }

    fn register(&self, r: &mut dyn Registrar) -> Result<(), RegisterError> {
        r.add_screen(ScreenSpec {
            id: POPUP,
            title: Cow::Borrowed("dictionary.title"),
            factory: Box::new(|env, params| Box::new(Popup::new(env, params))),
        })?;
        r.bind_slot(slots::LOOKUP, POPUP, Priority::DEFAULT)?;
        r.add_setting(
            SettingSpec::new(
                FOLDER,
                "dictionary.setting.folder",
                SettingKind::Text,
                SettingValue::Text(Cow::Borrowed("dictionaries")),
            )
            .group(groups::READING)
            .section("dictionary.section")
            .editor(CHOOSE_FOLDER)
            .description("dictionary.setting.folder.help"),
        )?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/en.ini")))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/de.ini")))?;
        Ok(())
    }
}

/// The `.ifo` files below `folder`, sorted.
fn find_dictionaries(storage: &dyn Storage, folder: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut folders = alloc::vec![(folder.to_string(), 0usize)];
    while let Some((dir, depth)) = folders.pop() {
        let _ = storage.list(&dir, &mut |entry| {
            if entry.name.starts_with('.') {
                return;
            }
            let path = if dir.is_empty() {
                entry.name.to_string()
            } else {
                format!("{dir}/{}", entry.name)
            };
            if entry.is_dir && depth < MAX_DEPTH {
                folders.push((path, depth + 1));
            } else if !entry.is_dir && entry.name.ends_with(".ifo") {
                found.push(path);
            }
        });
    }
    found.sort_unstable();
    found
}

fn fnv(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, b| {
        (hash ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// What the panel shows.
#[derive(Debug)]
enum State {
    /// Building the dictionaries' indexes: the builders still to run, and
    /// the bytes done and in total.
    Preparing(Vec<Builder>, u64, u64),
    /// The text to show (definitions or a message).
    Ready,
}

struct Popup {
    word: String,
    dictionaries: Vec<Dictionary>,
    state: State,
    /// The definitions (or a message), and its lines as byte ranges.
    text: String,
    lines: Vec<(usize, usize)>,
    /// First line shown.
    top: usize,
    style: UiStyle,
    region: Rect,
}

impl core::fmt::Debug for Popup {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Popup")
            .field("word", &self.word)
            .finish_non_exhaustive()
    }
}

impl Popup {
    fn new(env: &Env<'_>, params: &Params) -> Self {
        let style = UiStyle::new(env);
        let display = env.device.display;
        let top = display.height * 9 / 20;
        let mut popup = Self {
            word: params.get(params::WORD).unwrap_or_default().to_string(),
            dictionaries: Vec::new(),
            state: State::Ready,
            text: String::new(),
            lines: Vec::new(),
            top: 0,
            style,
            region: Rect::new(0, top, display.width, display.height - top),
        };
        let folder = env.settings.text(FOLDER.as_str()).trim_matches('/');
        let cache_dir = Env::plugin_dir(PLUGIN.as_str());
        for ifo in find_dictionaries(env.storage, folder) {
            let cache = format!("{cache_dir}/{:016x}", fnv(&ifo));
            if let Ok(dictionary) = Dictionary::open(env.storage, &ifo, &cache) {
                popup.dictionaries.push(dictionary);
            }
        }
        if popup.dictionaries.is_empty() {
            let message = env
                .i18n
                .format("dictionary.none", &[("folder", Arg::Str(folder))]);
            popup.set_text(env, message);
            return popup;
        }
        let builders: Vec<Builder> = popup
            .dictionaries
            .iter()
            .flat_map(|d| d.builders(env.storage))
            .collect();
        if builders.is_empty() {
            popup.look_up(env);
        } else {
            let total = builders.iter().map(|b| b.progress().1).sum();
            popup.state = State::Preparing(builders, 0, total);
        }
        popup
    }

    /// Looks the word up in every dictionary.
    fn look_up(&mut self, env: &Env<'_>) {
        let several = self.dictionaries.len() > 1;
        let mut text = String::new();
        for dictionary in &self.dictionaries {
            let Ok(found) = dictionary.lookup(env.storage, &self.word) else {
                continue;
            };
            if found.is_empty() {
                continue;
            }
            if several {
                if !text.is_empty() {
                    text.push_str("\n\n");
                }
                text.push_str(&format!("({})", dictionary.info.name));
            }
            for definition in found {
                if !text.is_empty() {
                    text.push_str("\n\n");
                }
                text.push_str(&definition.text);
            }
        }
        if text.is_empty() {
            text = env
                .i18n
                .format("dictionary.not-found", &[("word", Arg::Str(&self.word))]);
        }
        self.state = State::Ready;
        self.set_text(env, text);
    }

    /// Takes `text` and wraps it into lines.
    fn set_text(&mut self, env: &Env<'_>, text: String) {
        self.lines.clear();
        self.top = 0;
        if let Some(font) = self.style.font(env) {
            let width = f32::from(self.region.width) - 2.0 * self.style.margin as f32;
            let mut start = 0;
            for paragraph in text.split('\n') {
                if paragraph.is_empty() {
                    self.lines.push((start, start));
                }
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

    /// Lines that fit below the header.
    fn rows(&self, env: &Env<'_>) -> usize {
        let header = self.header();
        let height =
            f32::from(self.region.height) - f32::from(header.height) - self.style.gap as f32;
        ((height / self.line_height(env)) as usize).max(1)
    }

    /// Scrolls by a page; `false` if there is nothing more that way.
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
}

impl Screen for Popup {
    fn region(&self, _: &Env<'_>) -> Option<Rect> {
        Some(self.region)
    }

    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        let r = self.region;
        ui::fill(canvas, r, Color::WHITE);
        let line = (style.line * 2).max(2);
        canvas.fill_rect(
            i32::from(r.x),
            i32::from(r.y),
            u32::from(r.width),
            line,
            Color::BLACK,
        );
        let Some(font) = style.font(env) else {
            return;
        };
        // The word, and where in the definitions the page is.
        let header = self.header();
        let rows = self.rows(env);
        let note = match &self.state {
            State::Preparing(_, done, total) => env.i18n.format(
                "dictionary.preparing",
                &[("percent", Arg::Int((done * 100 / (*total).max(1)) as i64))],
            ),
            State::Ready if self.lines.len() > rows => env.i18n.format(
                "dictionary.page",
                &[
                    ("page", Arg::Int((self.top / rows + 1) as i64)),
                    ("pages", Arg::Int(self.lines.len().div_ceil(rows) as i64)),
                ],
            ),
            State::Ready => String::new(),
        };
        let margin = style.margin as f32;
        let note_width = font.measure(style.small, &note);
        let right = f32::from(header.width) - margin - note_width;
        ui::draw_label(
            canvas,
            &font,
            style.small,
            right,
            header,
            &note,
            Color::gray(80),
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
        if matches!(self.state, State::Preparing(..)) {
            return;
        }
        let height = self.line_height(env);
        let ascent = font.line_metrics(style.text).ascent;
        let top = f32::from(header.y + header.height) + style.gap as f32 / 2.0;
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
        if matches!(self.state, State::Preparing(..)) {
            let now = cx.env().now;
            cx.wake_at(now);
        }
    }

    /// Prepares the dictionaries a step at a time, then looks up the word.
    fn on_timer(&mut self, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        let State::Preparing(builders, done, _) = &mut self.state else {
            return;
        };
        let finished = match builders.last_mut() {
            Some(builder) => {
                let before = builder.progress().0;
                let finished = builder.step(env.storage, STEP_BYTES);
                *done += builder.progress().0 - before;
                match finished {
                    Ok(true) => {
                        builders.pop();
                        builders.is_empty()
                    }
                    Ok(false) => false,
                    // A damaged dictionary: skip it.
                    Err(_) => {
                        builders.pop();
                        builders.is_empty()
                    }
                }
            }
            None => true,
        };
        if finished {
            self.look_up(&env);
        } else {
            cx.wake_at(env.now.add_millis(1));
        }
        cx.request_redraw();
        cx.request_refresh(RefreshMode::Fast);
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
        // At the end, a tap closes the panel.
        if !self.scroll(&env, forward, cx) && forward {
            cx.command(Command::Back);
        }
        Handled::Yes
    }
}

#[cfg(test)]
mod tests {
    use quireink_i18n::{Catalog, check};

    #[test]
    fn translations_are_complete() {
        let en = Catalog::parse(include_str!("../i18n/en.ini")).unwrap();
        let de = Catalog::parse(include_str!("../i18n/de.ini")).unwrap();
        let issues = check::compare(&en, &de);
        assert!(issues.is_empty(), "{issues:?}");
    }
}
