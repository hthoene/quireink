//! The paged reader.
//!
//! - Shows any document a format plugin can open, page by page, in the
//!   reading font role, with configurable size, spacing, margins,
//!   justification and paragraph indents.
//! - Saves the reading position per book and reopens the last book at
//!   start-up (setting `reader.open-last`).
//! - Builds a page index in the background, so page numbers and total pages
//!   become available without slowing down page turns.
//! - Follows links: a tap, or with buttons *Links* in the menu, which
//!   moves from link to link (on over the pages) until one is confirmed.
//!   *Back* returns to where a link was followed from.
//! - Emits the standard events `core.book-opened` and `core.page-turned`,
//!   so other plugins (home screen, statistics, sync, …) follow the reading.
//!
//! It is bound to the `core.reader` slot; any other plugin can replace it.

#![no_std]

extern crate alloc;

mod aloud;
mod annotations;
mod footnote;
mod index;
mod screen;
mod search;
mod selection;
mod skim;
mod status;
mod toc;

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use quireink_sdk::builtin::{
    actions as core_actions, events, groups, params, setting_sections, settings as core_settings,
    slots,
};
use quireink_sdk::{
    Command, Env, Id, Manifest, Params, Plugin, Priority, RegisterError, Registrar, SDK_VERSION,
    ScreenSpec, SettingKind, SettingSpec, SettingValue, Version,
};

pub const PLUGIN: Id = Id::new_static("reader");
pub const SCREEN: Id = Id::new_static("reader.screen");
/// The table of contents screen; parameters `path`, `section`, `offset`.
pub const TOC_SCREEN: Id = Id::new_static("reader.toc");
/// The bookmarks and highlights of a book; parameter `path`.
pub const ANNOTATIONS_SCREEN: Id = Id::new_static("reader.annotations");
/// The actions offered for selected text, as a menu: plugins add their own
/// with `Registrar::add_menu_entry`, users reorder them in `[menus]`.
pub const SELECTION_MENU: Id = Id::new_static("reader.selection");
/// Emitted when a plugin's action is chosen for selected text, with the
/// parameters `action` (the action's id), `text`, `language`, `path`,
/// `section` and `offset` (the start) and `end-section` and `end-offset`.
/// The plugin whose action it is reacts; e.g. a "Share" or "Translate"
/// action needs no more than a menu entry and a subscription. Afterwards
/// the action itself runs as usual, so any action can be listed.
pub const TEXT_CHOSEN: Id = Id::new_static("reader.text-chosen");
/// Parameters of [`TEXT_CHOSEN`].
pub const ACTION: &str = "action";
pub const TEXT: &str = "text";
pub const END_SECTION: &str = "end-section";
pub const END_OFFSET: &str = "end-offset";
/// Search results for a book; parameters `path` and `text`.
pub const SEARCH_SCREEN: Id = Id::new_static("reader.search-results");
/// The keyboard's replies: what to search for, which page to go to.
const SEARCH_ENTERED: Id = Id::new_static("reader.search-entered");
const PAGE_ENTERED: Id = Id::new_static("reader.page-entered");
/// The keyboard's reply with a note on a highlight.
const NOTE_ENTERED: Id = Id::new_static("reader.note-entered");
/// Font and layout, shown as a panel ([`actions::QUICK_SETTINGS`]), and
/// the submenu "More" of [`MENU`]: menus like any other, so plugins add
/// entries and users redefine them in `[menus]` or the menu editor.
pub const LAYOUT_MENU: Id = Id::new_static("reader.layout");
pub const MORE_MENU: Id = Id::new_static("reader.more");
/// Asks the reader to show a position. Parameters: `path` (the book),
/// `section` and `offset`; with `end-section` and `end-offset` the text up
/// to there is marked (e.g. a search hit). Emitted by the table of contents; bookmark or
/// search plugins can use it too.
pub const GOTO: Id = Id::new_static("reader.goto");
/// Follows a link of the open book: parameters `path` and `href`.
pub const FOLLOW: Id = Id::new_static("reader.follow");
/// The reply when a page has been read aloud; parameter `path`.
const SPOKEN: Id = Id::new_static("reader.spoken");
/// Skimming through the book, an overlay (see `skim`); it asks the
/// reader to move with [`SKIM`] and hears where the book is with
/// [`SKIMMED`].
pub const SKIM_SCREEN: Id = Id::new_static("reader.skim-screen");
const SKIM: Id = Id::new_static("reader.skim");
const SKIMMED: Id = Id::new_static("reader.skimmed");
/// A footnote over the page (see `footnote`).
pub const FOOTNOTE_SCREEN: Id = Id::new_static("reader.footnote");

/// Settings of the reader.
pub mod settings {
    use quireink_sdk::Id;

    /// Font size in points.
    pub const FONT_SIZE: Id = Id::new_static("reader.font-size");
    /// Darker text: glyphs widened by up to this many steps.
    pub const FONT_WEIGHT: Id = Id::new_static("reader.font-weight");
    /// Space between words in percent of the font's.
    pub const WORD_SPACING: Id = Id::new_static("reader.word-spacing");
    /// Distance between baselines in percent of the font size.
    pub const LINE_SPACING: Id = Id::new_static("reader.line-spacing");
    /// Page margins in millimetres.
    pub const MARGIN: Id = Id::new_static("reader.margin");
    pub const JUSTIFY: Id = Id::new_static("reader.justify");
    /// Indent first lines instead of spacing paragraphs apart.
    pub const INDENT: Id = Id::new_static("reader.indent");
    /// Reopen the last book at start-up.
    pub const OPEN_LAST: Id = Id::new_static("reader.open-last");
    /// Notes shown over the page ([`FOOTNOTES_POPUP`]) or jumped to
    /// ([`FOOTNOTES_JUMP`]).
    pub const FOOTNOTES: Id = Id::new_static("reader.footnotes");
    pub const FOOTNOTES_POPUP: &str = "reader.footnotes.popup";
    pub const FOOTNOTES_JUMP: &str = "reader.footnotes.jump";
    /// Hyphenate words at line ends, in the book's language.
    pub const HYPHENATE: Id = Id::new_static("reader.hyphenate");
    /// Apply the book's own styles (an EPUB's CSS).
    pub const PUBLISHER_STYLES: Id = Id::new_static("reader.publisher-styles");
    /// A tap on the top right corner sets or removes a bookmark.
    pub const CORNER_BOOKMARK: Id = Id::new_static("reader.corner-bookmark");
    /// How fast books are read aloud, in percent of the voice's speed.
    pub const ALOUD_SPEED: Id = Id::new_static("reader.aloud-speed");
    /// The sentence read aloud is underlined.
    pub const ALOUD_HIGHLIGHT: Id = Id::new_static("reader.aloud-highlight");
    /// Reading aloud turns the pages; else it stops at the page's end.
    pub const ALOUD_TURN: Id = Id::new_static("reader.aloud-turn-pages");
    /// Reading aloud stops by itself after a while (a sleep timer).
    pub const ALOUD_TIMER: Id = Id::new_static("reader.aloud-timer");
    /// How highlights look: [`HIGHLIGHT_MARKER`] (gray) or
    /// [`HIGHLIGHT_UNDERLINE`].
    pub const HIGHLIGHT_STYLE: Id = Id::new_static("reader.highlight-style");
    pub const HIGHLIGHT_MARKER: &str = "reader.highlight-style.marker";
    pub const HIGHLIGHT_UNDERLINE: &str = "reader.highlight-style.underline";
    /// Seconds between pages turned by themselves ([`actions::AUTO_TURN`]).
    pub const AUTO_TURN_SECONDS: Id = Id::new_static("reader.auto-turn-seconds");
}

/// The reader's menu (`core.menu` on the reader screen). Any plugin can
/// add entries; users can redefine it in `[menus]`.
pub const MENU: Id = Id::new_static("reader.menu");

/// Actions of the reader.
pub mod actions {
    use quireink_sdk::Id;

    pub const FONT_LARGER: Id = Id::new_static("reader.font-larger");
    pub const FONT_SMALLER: Id = Id::new_static("reader.font-smaller");
    pub const TOGGLE_JUSTIFY: Id = Id::new_static("reader.toggle-justify");
    /// Opens the table of contents.
    pub const TOC: Id = Id::new_static("reader.show-toc");
    /// Selects text with the buttons to look it up or highlight it
    /// (touch: long-press a word).
    pub const SELECT: Id = Id::new_static("reader.select-text");
    /// Chooses a link on the page with the buttons and follows it
    /// (touch: tap the link).
    pub const LINKS: Id = Id::new_static("reader.links");
    /// Shows the next of the status lines below the text (above, if there
    /// is none below), as a tap on it does.
    pub const NEXT_STATUS_LINE: Id = Id::new_static("reader.next-status-line");
    /// Shows the next of the status lines above the text.
    pub const NEXT_TOP_STATUS_LINE: Id = Id::new_static("reader.next-top-status-line");
    /// Sets or removes the bookmark of the page.
    pub const TOGGLE_BOOKMARK: Id = Id::new_static("reader.toggle-bookmark");
    /// Lists the book's bookmarks and highlights.
    pub const ANNOTATIONS: Id = Id::new_static("reader.show-annotations");
    /// Searches the book (asks for the text on the keyboard).
    pub const SEARCH: Id = Id::new_static("reader.search");
    /// Opens font and layout as a panel of controls.
    pub const QUICK_SETTINGS: Id = Id::new_static("reader.quick-settings");
    /// Goes to a page number (asks for it on the keyboard).
    pub const GO_TO_PAGE: Id = Id::new_static("reader.go-to-page");
    /// Reads the book aloud from the page shown, turning the pages, until
    /// a tap or a button (where the device can speak).
    pub const READ_ALOUD: Id = Id::new_static("reader.read-aloud");
    /// Items of the selection menu that the reader handles itself.
    pub const LOOK_UP: Id = Id::new_static("reader.selection.look-up");
    /// Highlights the selected text, or removes highlights in it.
    pub const HIGHLIGHT: Id = Id::new_static("reader.selection.highlight");
    /// Skims through the book: a bar with its chapters, chapter by
    /// chapter, back to where one was (an overlay at the bottom).
    pub const SKIM_BOOK: Id = Id::new_static("reader.skim-book");
    /// Turns the pages by themselves every few seconds, or stops it.
    pub const AUTO_TURN: Id = Id::new_static("reader.auto-turn");
    /// Shows the book's information (slot `core.book-info`).
    pub const BOOK_INFO: Id = Id::new_static("reader.book-info");
    /// A note on the selected text (highlighted with it).
    pub const NOTE: Id = Id::new_static("reader.selection.note");
    /// Reads aloud from the selected text on.
    pub const READ_FROM: Id = Id::new_static("reader.selection.read-aloud");
    /// Copies the selected text to the clipboard.
    pub const COPY: Id = Id::new_static("reader.selection.copy");
    /// Moves the end of the selection with the buttons.
    pub const EXTEND: Id = Id::new_static("reader.selection.extend");
    pub const CANCEL: Id = Id::new_static("reader.selection.cancel");
    /// Shows all actions for the selection as a list, when the bar is too
    /// small for them.
    pub const MORE: Id = Id::new_static("reader.selection.more");
}

/// The reader plugin.
#[derive(Debug, Default)]
pub struct Reader;

impl Plugin for Reader {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: PLUGIN,
            name: Cow::Borrowed("Reader"),
            description: Cow::Borrowed(
                "Shows books page by page, with table of contents and reading settings.",
            ),
            version: Version::new(0, 1, 0),
            sdk: SDK_VERSION,
        }
    }

    fn register(&self, r: &mut dyn Registrar) -> Result<(), RegisterError> {
        r.add_screen(ScreenSpec {
            id: SCREEN,
            title: Cow::Borrowed("reader.title"),
            factory: Box::new(|env, params| Box::new(screen::ReaderScreen::open(env, params))),
        })?;
        r.bind_slot(slots::READER, SCREEN, Priority::DEFAULT)?;
        r.add_screen(ScreenSpec {
            id: TOC_SCREEN,
            title: Cow::Borrowed("reader.toc"),
            factory: Box::new(|env, params| Box::new(toc::TocScreen::open(env, params))),
        })?;
        r.add_action(quireink_sdk::ActionSpec {
            id: actions::TOC,
            title: Cow::Borrowed("reader.toc"),
            handler: None,
        })?;
        r.add_screen(ScreenSpec {
            id: ANNOTATIONS_SCREEN,
            title: Cow::Borrowed("reader.annotations"),
            factory: Box::new(|env, params| {
                Box::new(annotations::AnnotationsScreen::open(env, params))
            }),
        })?;
        status::register(r)?;
        r.add_screen(ScreenSpec {
            id: SKIM_SCREEN,
            title: Cow::Borrowed("reader.skim"),
            factory: Box::new(|env, params| Box::new(skim::SkimScreen::new(env, params))),
        })?;
        r.add_screen(ScreenSpec {
            id: FOOTNOTE_SCREEN,
            title: Cow::Borrowed("reader.title"),
            factory: Box::new(|env, params| Box::new(footnote::FootnoteScreen::new(env, params))),
        })?;
        r.add_screen(ScreenSpec {
            id: SEARCH_SCREEN,
            title: Cow::Borrowed("reader.search"),
            factory: Box::new(|env, params| Box::new(search::SearchScreen::open(env, params))),
        })?;
        for (id, title) in [
            (actions::SEARCH, "reader.search"),
            (actions::GO_TO_PAGE, "reader.go-to-page"),
            (actions::READ_ALOUD, "reader.read-aloud"),
            (actions::SELECT, "reader.select"),
            (actions::LINKS, "reader.links"),
            (actions::NEXT_STATUS_LINE, "reader.next-status-line"),
            (actions::NEXT_TOP_STATUS_LINE, "reader.next-top-status-line"),
            (actions::TOGGLE_BOOKMARK, "reader.toggle-bookmark"),
            (actions::ANNOTATIONS, "reader.annotations"),
            (actions::LOOK_UP, "reader.select.look-up"),
            (actions::HIGHLIGHT, "reader.select.highlight"),
            (actions::COPY, "reader.select.copy"),
            (actions::READ_FROM, "reader.select.read-aloud"),
            (actions::NOTE, "reader.select.note"),
            (actions::BOOK_INFO, "reader.book-info"),
            (actions::AUTO_TURN, "reader.auto-turn"),
            (actions::SKIM_BOOK, "reader.skim"),
            (actions::EXTEND, "reader.select.extend"),
            (actions::CANCEL, "reader.select.cancel"),
            (actions::MORE, "reader.select.more"),
        ] {
            r.add_action(quireink_sdk::ActionSpec {
                id,
                title: Cow::Borrowed(title),
                handler: None,
            })?;
        }
        // The bar for selected text; plugins add to it.
        for (item, order) in [
            (actions::LOOK_UP, 10),
            (actions::HIGHLIGHT, 20),
            (actions::NOTE, 22),
            (actions::READ_FROM, 25),
            (actions::COPY, 30),
            (actions::EXTEND, 80),
            (actions::CANCEL, 90),
        ] {
            r.add_menu_entry(quireink_sdk::MenuEntry {
                menu: SELECTION_MENU,
                item,
                order,
            })?;
        }

        let int = |id, key: &'static str, (min, max, default), unit: &'static str| {
            SettingSpec::new(
                id,
                key,
                SettingKind::Int { min, max },
                SettingValue::Int(default),
            )
            .group(groups::READING)
            .unit(unit)
            // Each book may have its own.
            .scoped()
        };
        let flag = |id, key: &'static str, default| {
            SettingSpec::new(id, key, SettingKind::Bool, SettingValue::Bool(default))
                .group(groups::READING)
        };
        for setting in [
            int(
                settings::FONT_SIZE,
                "reader.setting.font-size",
                (6, 40, 11),
                "core.unit.pt",
            )
            .section(setting_sections::FONT)
            .description("reader.setting.font-size.help"),
            int(
                settings::LINE_SPACING,
                "reader.setting.line-spacing",
                (100, 250, 135),
                "core.unit.percent",
            )
            .step(5)
            .section(setting_sections::PAGE)
            .description("reader.setting.line-spacing.help"),
            int(
                settings::FONT_WEIGHT,
                "reader.setting.font-weight",
                (0, 4, 0),
                "reader.unit.weight",
            )
            .section(setting_sections::FONT)
            .description("reader.setting.font-weight.help"),
            int(
                settings::WORD_SPACING,
                "reader.setting.word-spacing",
                (50, 200, 100),
                "core.unit.percent",
            )
            .step(10)
            .section(setting_sections::FONT)
            .description("reader.setting.word-spacing.help"),
            int(
                settings::MARGIN,
                "reader.setting.margin",
                (0, 30, 4),
                "core.unit.mm",
            )
            .section(setting_sections::PAGE)
            .description("reader.setting.margin.help"),
            flag(settings::JUSTIFY, "reader.setting.justify", true)
                .scoped()
                .section(setting_sections::PAGE)
                .description("reader.setting.justify.help"),
            flag(settings::HYPHENATE, "reader.setting.hyphenate", true)
                .scoped()
                .section(setting_sections::PAGE)
                .description("reader.setting.hyphenate.help"),
            flag(
                settings::PUBLISHER_STYLES,
                "reader.setting.publisher-styles",
                true,
            )
            .scoped()
            .section(setting_sections::PAGE)
            .description("reader.setting.publisher-styles.help"),
            flag(settings::INDENT, "reader.setting.indent", true)
                .advanced()
                .scoped()
                .section(setting_sections::PAGE)
                .description("reader.setting.indent.help"),
            flag(settings::OPEN_LAST, "reader.setting.open-last", true)
                .section(setting_sections::BEHAVIOUR)
                .description("reader.setting.open-last.help"),
            SettingSpec::new(
                settings::FOOTNOTES,
                "reader.setting.footnotes",
                SettingKind::Choice(alloc::vec![
                    Cow::Borrowed(settings::FOOTNOTES_POPUP),
                    Cow::Borrowed(settings::FOOTNOTES_JUMP),
                ]),
                SettingValue::Text(Cow::Borrowed(settings::FOOTNOTES_POPUP)),
            )
            .group(groups::READING)
            .section(setting_sections::BEHAVIOUR)
            .description("reader.setting.footnotes.help"),
            flag(
                settings::CORNER_BOOKMARK,
                "reader.setting.corner-bookmark",
                true,
            )
            .group(groups::CONTROLS)
            .description("reader.setting.corner-bookmark.help"),
            SettingSpec::new(
                settings::HIGHLIGHT_STYLE,
                "reader.setting.highlight-style",
                SettingKind::Choice(alloc::vec![
                    Cow::Borrowed(settings::HIGHLIGHT_MARKER),
                    Cow::Borrowed(settings::HIGHLIGHT_UNDERLINE),
                ]),
                SettingValue::Text(Cow::Borrowed(settings::HIGHLIGHT_MARKER)),
            )
            .group(groups::READING)
            .section(setting_sections::PAGE)
            .description("reader.setting.highlight-style.help"),
            SettingSpec::new(
                settings::AUTO_TURN_SECONDS,
                "reader.setting.auto-turn-seconds",
                SettingKind::Int { min: 5, max: 600 },
                SettingValue::Int(30),
            )
            .group(groups::READING)
            .unit("core.unit.seconds")
            .step(5)
            .section(setting_sections::BEHAVIOUR)
            .description("reader.setting.auto-turn-seconds.help"),
            SettingSpec::new(
                settings::ALOUD_SPEED,
                "reader.setting.aloud-speed",
                SettingKind::Int { min: 50, max: 300 },
                SettingValue::Int(100),
            )
            .group(groups::READING)
            .unit("core.unit.percent")
            .step(10)
            .section("reader.section.aloud")
            .description("reader.setting.aloud-speed.help"),
            flag(
                settings::ALOUD_HIGHLIGHT,
                "reader.setting.aloud-highlight",
                true,
            )
            .section("reader.section.aloud")
            .description("reader.setting.aloud-highlight.help"),
            flag(
                settings::ALOUD_TURN,
                "reader.setting.aloud-turn-pages",
                true,
            )
            .section("reader.section.aloud")
            .description("reader.setting.aloud-turn-pages.help"),
            SettingSpec::new(
                settings::ALOUD_TIMER,
                "reader.setting.aloud-timer",
                SettingKind::Choice(
                    aloud::TIMER_CHOICES
                        .iter()
                        .map(|c| Cow::Borrowed(*c))
                        .collect(),
                ),
                SettingValue::Text(Cow::Borrowed(aloud::TIMER_CHOICES[0])),
            )
            .group(groups::READING)
            .section("reader.section.aloud")
            .description("reader.setting.aloud-timer.help"),
        ] {
            r.add_setting(setting)?;
        }

        let font_step = |step: i64| -> quireink_sdk::ActionHandler {
            Box::new(move |env: &Env<'_>, out: &mut Vec<Command>| {
                let size = env.settings.int(settings::FONT_SIZE.as_str()) + step;
                out.push(Command::SetSetting(
                    settings::FONT_SIZE,
                    SettingValue::Int(size.clamp(6, 40)),
                ));
            })
        };
        let handlers: [(Id, &'static str, quireink_sdk::ActionHandler); 3] = [
            (
                actions::FONT_LARGER,
                "reader.action.font-larger",
                font_step(1),
            ),
            (
                actions::FONT_SMALLER,
                "reader.action.font-smaller",
                font_step(-1),
            ),
            (
                actions::TOGGLE_JUSTIFY,
                "reader.action.toggle-justify",
                Box::new(|env: &Env<'_>, out: &mut Vec<Command>| {
                    let justify = env.settings.bool(settings::JUSTIFY.as_str());
                    out.push(Command::SetSetting(
                        settings::JUSTIFY,
                        SettingValue::Bool(!justify),
                    ));
                }),
            ),
        ];
        for (id, title, handler) in handlers {
            r.add_action(quireink_sdk::ActionSpec {
                id,
                title: Cow::Borrowed(title),
                handler: Some(handler),
            })?;
        }
        // A submenu is an action named like its menu: the menu shows it as
        // a submenu, and bound to a key it opens the menu.
        r.add_action(quireink_sdk::ActionSpec {
            id: MORE_MENU,
            title: Cow::Borrowed("reader.more"),
            handler: Some(Box::new(|_: &Env<'_>, out: &mut Vec<Command>| {
                out.push(Command::OpenSlot(
                    slots::MENU,
                    Params::new().with(params::MENU, String::from(MORE_MENU.as_str())),
                ));
            })),
        })?;
        // Font and layout as a panel of controls: changes show at once.
        r.add_action(quireink_sdk::ActionSpec {
            id: actions::QUICK_SETTINGS,
            title: Cow::Borrowed("reader.layout"),
            handler: Some(Box::new(|_: &Env<'_>, out: &mut Vec<Command>| {
                out.push(Command::OpenSlot(
                    slots::PANEL,
                    Params::new().with(params::MENU, String::from(LAYOUT_MENU.as_str())),
                ));
            })),
        })?;
        // A tap at the bottom of a page opens it.
        r.add_keymap(Cow::Borrowed(
            "[keymap.reader.screen]\ntap.bottom = reader.quick-settings\n",
        ))?;
        // A short menu; settings in it show their state and change in
        // place.
        let entries = [
            (MENU, actions::TOC, 10),
            (MENU, actions::SEARCH, 20),
            (MENU, actions::READ_ALOUD, 30),
            (MENU, actions::ANNOTATIONS, 40),
            (MENU, actions::QUICK_SETTINGS, 50),
            (MENU, core_settings::NIGHT_MODE, 60),
            (MENU, MORE_MENU, 70),
            (LAYOUT_MENU, core_settings::SCOPE_ONLY, 1),
            (LAYOUT_MENU, core_settings::READING_FONT, 5),
            (LAYOUT_MENU, settings::FONT_SIZE, 10),
            (LAYOUT_MENU, settings::FONT_WEIGHT, 15),
            (LAYOUT_MENU, settings::LINE_SPACING, 20),
            (LAYOUT_MENU, settings::MARGIN, 30),
            (LAYOUT_MENU, settings::JUSTIFY, 40),
            (LAYOUT_MENU, settings::HYPHENATE, 50),
            (LAYOUT_MENU, settings::PUBLISHER_STYLES, 60),
            (MORE_MENU, actions::TOGGLE_BOOKMARK, 10),
            (MORE_MENU, actions::SKIM_BOOK, 15),
            (MORE_MENU, actions::AUTO_TURN, 17),
            (MORE_MENU, actions::SELECT, 20),
            (MORE_MENU, actions::LINKS, 25),
            (MORE_MENU, core_actions::HOME, 40),
            (MORE_MENU, core_actions::SETTINGS, 50),
        ];
        for (menu, item, order) in entries {
            r.add_menu_entry(quireink_sdk::MenuEntry { menu, item, order })?;
        }
        // Short menus: reading aloud where the device speaks; choosing
        // text and links with the buttons where there is no touch screen.
        for item in [
            actions::READ_ALOUD,
            actions::READ_FROM,
            settings::ALOUD_SPEED,
            settings::ALOUD_HIGHLIGHT,
            settings::ALOUD_TURN,
            settings::ALOUD_TIMER,
        ] {
            r.set_menu_need(item, quireink_sdk::MenuNeed::Speech)?;
        }
        for item in [actions::SELECT, actions::LINKS] {
            r.set_menu_need(item, quireink_sdk::MenuNeed::NoTouch)?;
        }
        for (item, icon) in [
            (actions::TOC, "list"),
            (actions::LINKS, "link"),
            (actions::SEARCH, "search"),
            (actions::GO_TO_PAGE, "hash"),
            (actions::ANNOTATIONS, "bookmarks"),
            (actions::QUICK_SETTINGS, "typography"),
            (MORE_MENU, "dots"),
            (actions::TOGGLE_BOOKMARK, "bookmark"),
            (actions::SELECT, "highlight"),
            (actions::READ_ALOUD, "volume"),
            (actions::AUTO_TURN, "clock"),
            (actions::SKIM_BOOK, "adjustments-horizontal"),
        ] {
            r.add_icon(item, Cow::Borrowed(icon))?;
        }

        r.subscribe(events::STARTED, Box::new(reopen_last_book))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/en.ini")))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/de.ini")))?;
        Ok(())
    }
}

/// Where the reader remembers the last book.
fn state_path() -> String {
    alloc::format!("{}/state.ini", Env::plugin_dir(PLUGIN.as_str()))
}

fn reopen_last_book(env: &Env<'_>, _: &Params, out: &mut Vec<Command>) {
    if !env.settings.bool(settings::OPEN_LAST.as_str()) {
        return;
    }
    let Ok(data) = quireink_sdk::read_file(env.storage, &state_path(), 4096) else {
        return;
    };
    let Ok(text) = core::str::from_utf8(&data) else {
        return;
    };
    let last = quireink_sdk::config::parse(text)
        .filter_map(Result::ok)
        .find(|e| e.section == "reader" && e.key == "last")
        .map(|e| String::from(e.value.as_str()));
    if let Some(path) = last
        && env.storage.size(&path).is_ok()
    {
        out.push(Command::OpenSlot(
            slots::READER,
            Params::new().with(params::PATH, path),
        ));
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
