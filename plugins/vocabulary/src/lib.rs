//! Vocabulary: the words looked up while reading, each with its sentence
//! and book, to go over later.
//!
//! The reader announces every word looked up
//! ([`events::WORD_LOOKED_UP`]); this plugin keeps it (setting
//! `vocabulary.collect`, on at first), newest first. *Vocabulary* in the
//! main menu lists them: a tap looks the word up again, holding it offers
//! to go to its place in the book or to delete it. The last row exports
//! all words for flash card programs such as Anki (word, sentence, book,
//! separated by tabs) into the `Notes` folder beside the library.
//!
//! Only the visible page of words is ever in memory.

#![no_std]

extern crate alloc;

mod store;

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::builtin::{actions, events, groups, menus, params, slots};
use quireink_sdk::ui::choice::{self, Choice};
use quireink_sdk::ui::{self, ListChange, ListView, UiStyle};
use quireink_sdk::widgets;
use quireink_sdk::{
    ActionSpec, Arg, Canvas, Command, Cx, Env, Gesture, GestureKind, Handled, Id, Manifest,
    MenuEntry, Params, Plugin, Rect, RefreshMode, RegisterError, Registrar, SDK_VERSION, Screen,
    ScreenSpec, SettingKind, SettingSpec, SettingValue, Version,
};

use store::Word;

pub const PLUGIN: Id = Id::new_static("vocabulary");
/// Opens the list of words.
pub const OPEN: Id = Id::new_static("vocabulary.open");
/// Setting: keep the words looked up.
pub const COLLECT: Id = Id::new_static("vocabulary.collect");
const SCREEN: Id = Id::new_static("vocabulary.list");
const CHOICE: Id = Id::new_static("vocabulary.choice");
const CHOSEN: Id = Id::new_static("vocabulary.chosen");
const OPTION_LABELS: &str = "vocabulary.option.";
/// The reader's event for going to a place in the open book.
const READER_GOTO: Id = Id::new_static("reader.goto");
/// The dictionary's settings (see the `dictionary` plugin).
const DICTIONARY_SECTION: &str = "dictionary.section";
/// The folder exports go to, beside the library (as the reader's notes).
const NOTES: &str = "Notes";
const EXPORT_NAME: &str = "Vocabulary.tsv";

/// The vocabulary plugin.
#[derive(Debug, Default)]
pub struct Vocabulary;

impl Plugin for Vocabulary {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: PLUGIN,
            name: Cow::Borrowed("Vocabulary"),
            description: Cow::Borrowed(
                "Keeps the words you look up, with their sentences, to go over later.",
            ),
            version: Version::new(0, 1, 0),
            sdk: SDK_VERSION,
        }
    }

    fn register(&self, r: &mut dyn Registrar) -> Result<(), RegisterError> {
        r.add_setting(
            SettingSpec::new(
                COLLECT,
                "vocabulary.setting.collect",
                SettingKind::Bool,
                SettingValue::Bool(true),
            )
            .group(groups::READING)
            .section(DICTIONARY_SECTION)
            .description("vocabulary.setting.collect.help"),
        )?;
        r.subscribe(
            events::WORD_LOOKED_UP,
            Box::new(|env, params, _| record(env, params)),
        )?;
        r.add_screen(ScreenSpec {
            id: SCREEN,
            title: Cow::Borrowed("vocabulary.title"),
            factory: Box::new(|env, _| Box::new(WordsScreen::new(env))),
        })?;
        r.add_screen(ScreenSpec {
            id: CHOICE,
            title: Cow::Borrowed("vocabulary.title"),
            factory: Box::new(|env, params| Box::new(Choice::new(env, params))),
        })?;
        r.add_action(ActionSpec {
            id: OPEN,
            title: Cow::Borrowed("vocabulary.title"),
            handler: Some(Box::new(|_: &Env<'_>, out: &mut Vec<Command>| {
                out.push(Command::OpenScreen(SCREEN, Params::new()));
            })),
        })?;
        r.add_menu_entry(MenuEntry {
            menu: menus::TOOLS,
            item: OPEN,
            order: 36,
        })?;
        r.add_icon(OPEN, Cow::Borrowed("vocabulary"))?;
        r.add_setting_group(widgets::settings_group(
            &WORD_WIDGET,
            "vocabulary.widget.word",
        ))?;
        r.add_setting(widgets::align_setting(&WORD_WIDGET, widgets::Align::Left))?;
        r.add_widget(widgets::WidgetSpec {
            id: WORD_WIDGET,
            title: Cow::Borrowed("vocabulary.widget.word"),
            height: Box::new(word_height),
            draw: Box::new(draw_word),
            signature: Some(Box::new(|env| {
                word_of_the_day(env).map_or(String::new(), |w| w.word)
            })),
            targets: Some(Box::new(|_, area| {
                alloc::vec![widgets::WidgetTarget {
                    area,
                    command: Command::RunAction(OPEN),
                }]
            })),
        })?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/en.ini")))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/de.ini")))?;
        Ok(())
    }
}

/// The word of the day on the home screen.
const WORD_WIDGET: Id = Id::new_static("vocabulary.widget.word");

/// A word to go over today: another one every day.
fn word_of_the_day(env: &Env<'_>) -> Option<Word> {
    let count = store::count(env.storage);
    if count == 0 {
        return None;
    }
    let day = quireink_sdk::time::unix_seconds(env).map_or(0, |s| s / 86_400) as usize;
    store::range(env.storage, day % count, day % count + 1)
        .into_iter()
        .next()
        .map(|(_, word)| word)
}

fn word_height(env: &Env<'_>, _: u16) -> u16 {
    if store::count(env.storage) == 0 {
        return 0;
    }
    let style = UiStyle::new(env);
    let Some(font) = style.font(env) else {
        return 0;
    };
    let small = font.line_metrics(style.small).line_height;
    let title = font.line_metrics(style.title).line_height;
    (small * 3.0 + title) as u16
}

/// "Word of the day", the word, and its sentence (two lines at most).
fn draw_word(canvas: &mut Canvas<'_>, env: &Env<'_>, area: Rect) {
    let style = UiStyle::new(env);
    let (Some(font), Some(word)) = (style.font(env), word_of_the_day(env)) else {
        return;
    };
    let small = font.line_metrics(style.small).line_height as u16;
    let title = font.line_metrics(style.title).line_height as u16;
    let align = widgets::align(env, WORD_WIDGET.as_str());
    let x = |size: f32, text: &str| align.x(area, font.measure(size, text));
    let gray = style.faint(80);
    let row = |y: u16, h: u16| Rect::new(area.x, y, area.width, h);
    let label = env.i18n.tr("vocabulary.widget.word");
    ui::draw_label(
        canvas,
        &font,
        style.small,
        x(style.small, label),
        row(area.y, small),
        label,
        gray,
    );
    let shown = ui::fit(&font, style.title, &word.word, f32::from(area.width));
    ui::draw_label(
        canvas,
        &font,
        style.title,
        x(style.title, &shown),
        row(area.y + small, title),
        &shown,
        quireink_sdk::Color::BLACK,
    );
    let lines = font.wrap(style.small, &word.sentence, f32::from(area.width));
    for (n, range) in lines.into_iter().take(2).enumerate() {
        let y = area.y + small + title + n as u16 * small;
        let line = &word.sentence[range];
        ui::draw_label(
            canvas,
            &font,
            style.small,
            x(style.small, line),
            row(y, small),
            line,
            gray,
        );
    }
}

/// Keeps a word looked up, if wanted.
fn record(env: &Env<'_>, params: &Params) {
    if !env.settings.bool(COLLECT.as_str()) {
        return;
    }
    let word = params.get(params::WORD).unwrap_or_default().trim();
    if word.is_empty() {
        return;
    }
    let number = |key| params.get(key).and_then(|n| n.parse().ok()).unwrap_or(0);
    store::add(
        env.storage,
        &Word {
            word: word.to_string(),
            language: params.get(params::LANGUAGE).unwrap_or_default().to_string(),
            section: number(params::SECTION),
            offset: number(params::OFFSET),
            path: params.get(params::PATH).unwrap_or_default().to_string(),
            title: params.get(params::TITLE).unwrap_or_default().to_string(),
            sentence: params.get(params::TEXT).unwrap_or_default().to_string(),
        },
    );
}

/// The folder exports go to: `Notes` beside the library folder.
fn notes_dir(env: &Env<'_>) -> String {
    let library = env.settings.text("files.root").trim_matches('/');
    let parent = quireink_sdk::files::parent(library);
    if parent.is_empty() {
        String::from(NOTES)
    } else {
        format!("{parent}/{NOTES}")
    }
}

/// The list of words, newest first, and *Export* below them.
struct WordsScreen {
    style: UiStyle,
    header: ui::Header,
    list: ListView,
    /// Words in all.
    count: usize,
    /// The words of the visible page: their row and number in the file.
    visible: Vec<(usize, usize, Word)>,
}

impl WordsScreen {
    fn new(env: &Env<'_>) -> Self {
        let style = UiStyle::new(env);
        let display = env.device.display;
        let header = ui::Header::new(env, &style, true);
        let top = header.area.height + style.gap as u16;
        let area = Rect::new(0, top, display.width, display.height.saturating_sub(top));
        let mut screen = Self {
            list: ListView::new(area, style.noted_row()).focus_on_keys(env),
            style,
            header,
            count: 0,
            visible: Vec::new(),
        };
        screen.load(env);
        screen
    }

    /// Counts the words and reads the visible page.
    fn load(&mut self, env: &Env<'_>) {
        self.count = store::count(env.storage);
        let len = self.len();
        if self.list.focus >= len {
            self.list.focus = len.saturating_sub(1);
        }
        self.load_page(env);
    }

    fn load_page(&mut self, env: &Env<'_>) {
        let first = self.list.page() * self.list.rows();
        let last = (first + self.list.rows()).min(self.count);
        // Row `r` shows word number `count - 1 - r`.
        let (from, to) = (
            self.count.saturating_sub(last),
            self.count - first.min(self.count),
        );
        self.visible = store::range(env.storage, from, to)
            .into_iter()
            .map(|(n, word)| (self.count - 1 - n, n, word))
            .collect();
    }

    /// The rows: the words, then *Export* if there are any.
    fn len(&self) -> usize {
        match self.count {
            0 => 0,
            n => n + 1,
        }
    }

    fn word(&self, row: usize) -> Option<&(usize, usize, Word)> {
        self.visible.iter().find(|(r, _, _)| *r == row)
    }

    /// A tap or confirm: looks the word up again, or exports.
    fn open(&mut self, row: usize, cx: &mut Cx<'_, '_>) {
        if row == self.count && row > 0 {
            return self.export(cx);
        }
        if let Some((_, _, word)) = self.word(row) {
            cx.command(Command::OpenSlot(
                slots::LOOKUP,
                Params::new()
                    .with(params::WORD, word.word.clone())
                    .with(params::LANGUAGE, word.language.clone()),
            ));
        }
    }

    /// Holding a word: what can be done with it.
    fn offer(&mut self, row: usize, cx: &mut Cx<'_, '_>) {
        let Some((_, n, word)) = self.word(row) else {
            return;
        };
        let mut options = alloc::vec!["look-up"];
        if !word.path.is_empty() && cx.env().storage.exists(&word.path) {
            options.push("go-to");
        }
        options.push("delete");
        cx.command(choice::open(
            CHOICE,
            CHOSEN,
            &word.word,
            &format!("{n}"),
            OPTION_LABELS,
            &options,
        ));
    }

    fn export(&self, cx: &mut Cx<'_, '_>) {
        let env = *cx.env();
        let dir = notes_dir(&env);
        let _ = env.storage.create_dir(&dir);
        let file = format!("{dir}/{EXPORT_NAME}");
        let message = if store::export(env.storage, &file) {
            env.i18n
                .format("vocabulary.exported", &[("path", Arg::Str(&file))])
        } else {
            env.i18n.tr("vocabulary.export-failed").to_string()
        };
        cx.command(Command::Notify(message));
    }

    fn changed(&mut self, change: ListChange, cx: &mut Cx<'_, '_>) {
        if change == ListChange::None {
            return;
        }
        if change == ListChange::Page {
            let env = *cx.env();
            self.load_page(&env);
        }
        cx.request_redraw();
        cx.request_refresh(if change == ListChange::Page {
            RefreshMode::Partial
        } else {
            RefreshMode::Fast
        });
    }
}

impl Screen for WordsScreen {
    fn draw(&mut self, canvas: &mut Canvas<'_>, env: &Env<'_>) {
        let style = self.style;
        let len = self.len();
        self.header.draw(
            canvas,
            env,
            &style,
            env.i18n.tr("vocabulary.title"),
            None,
            self.list.paging(len),
        );
        if len == 0 {
            ui::draw_message(
                canvas,
                env,
                &style,
                self.list.area,
                env.i18n.tr("vocabulary.empty"),
            );
            return;
        }
        let count = self.count;
        let visible = &self.visible;
        self.list
            .draw(canvas, &style, len, |canvas, row, rect, focused| {
                if row == count {
                    let title = env.i18n.tr("vocabulary.export");
                    let note = env.i18n.tr("vocabulary.export.help");
                    ui::draw_noted_row(canvas, env, &style, rect, focused, title, "›", note);
                    return;
                }
                let Some((_, _, word)) = visible.iter().find(|(r, _, _)| *r == row) else {
                    return;
                };
                let note = if word.sentence.is_empty() {
                    word.title.as_str()
                } else {
                    word.sentence.as_str()
                };
                ui::draw_noted_row(canvas, env, &style, rect, focused, &word.word, "", note);
            });
    }

    fn on_action(&mut self, action: &Id, cx: &mut Cx<'_, '_>) -> Handled {
        let len = self.len();
        let rows = self.list.rows() as isize;
        let a = action;
        let delta = if [actions::UP, actions::LEFT, actions::PREVIOUS].contains(a) {
            -1
        } else if [actions::DOWN, actions::RIGHT, actions::NEXT].contains(a) {
            1
        } else if *a == actions::NEXT_PAGE {
            rows
        } else if *a == actions::PREV_PAGE {
            -rows
        } else if *a == actions::CONFIRM {
            self.open(self.list.focus, cx);
            return Handled::Yes;
        } else if *a == actions::MENU {
            self.offer(self.list.focus, cx);
            return Handled::Yes;
        } else {
            return Handled::No;
        };
        let change = self.list.move_focus(delta, len);
        self.changed(change, cx);
        Handled::Yes
    }

    fn on_gesture(&mut self, gesture: Gesture, cx: &mut Cx<'_, '_>) -> Handled {
        let env = *cx.env();
        let len = self.len();
        if gesture.kind == GestureKind::Tap {
            let paging = self.list.paging(len);
            if let Some(action) = self.header.tap(&env, &self.style, gesture.start, paging) {
                cx.command(Command::RunAction(action));
                return Handled::Yes;
            }
        }
        let Some(row) = self.list.hit(gesture.start, len) else {
            return Handled::No;
        };
        self.list.focus = row;
        match gesture.kind {
            GestureKind::Tap => self.open(row, cx),
            GestureKind::LongPress => self.offer(row, cx),
            _ => return Handled::No,
        }
        Handled::Yes
    }

    fn on_event(&mut self, event: &Id, params: &Params, cx: &mut Cx<'_, '_>) {
        if *event != CHOSEN {
            return;
        }
        let env = *cx.env();
        let Some(n) = params
            .get(params::PATH)
            .and_then(|n| n.parse::<usize>().ok())
        else {
            return;
        };
        let Some((_, _, word)) = self.visible.iter().find(|(_, i, _)| *i == n).cloned() else {
            return;
        };
        match params.get(choice::OPTION).unwrap_or_default() {
            "look-up" => cx.command(Command::OpenSlot(
                slots::LOOKUP,
                Params::new()
                    .with(params::WORD, word.word.clone())
                    .with(params::LANGUAGE, word.language.clone()),
            )),
            "go-to" => {
                // The book, then the word's place in it.
                cx.command(Command::OpenSlot(
                    slots::READER,
                    Params::new().with(params::PATH, word.path.clone()),
                ));
                cx.command(Command::Emit(
                    READER_GOTO,
                    Params::new()
                        .with(params::PATH, word.path.clone())
                        .with(params::SECTION, format!("{}", word.section))
                        .with(params::OFFSET, format!("{}", word.offset)),
                ));
            }
            "delete" => {
                store::remove(env.storage, n);
                self.load(&env);
                cx.request_redraw();
                cx.request_refresh(RefreshMode::Partial);
            }
            _ => {}
        }
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
