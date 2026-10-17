//! Ids of the contributions the kernel itself provides (namespace `core`).
//!
//! Plugins use these to bind screens to the standard slots, to handle the
//! standard navigation actions and to read the global settings.

/// The kernel's own plugin id; it cannot be switched off.
pub const CORE_PLUGIN: crate::Id = crate::Id::new_static("core");

/// Standard actions. Navigation actions have no global handler; screens
/// interpret them. `back`, `home` and `toggle-night-mode` have global
/// handlers, which screens may still override by handling the action.
pub mod actions {
    use crate::Id;

    /// Does nothing: bound to an input, it stops the input from doing
    /// anything there.
    pub const NOTHING: Id = Id::new_static("core.nothing");

    pub const NEXT_PAGE: Id = Id::new_static("core.next-page");
    /// "Forward" in the widest sense: next list entry, increase a value
    /// being edited; screens without such a meaning treat it as the next
    /// page. Lets devices with very few buttons operate everything.
    pub const NEXT: Id = Id::new_static("core.next");
    /// The opposite of [`NEXT`].
    pub const PREVIOUS: Id = Id::new_static("core.previous");
    pub const PREV_PAGE: Id = Id::new_static("core.prev-page");
    pub const UP: Id = Id::new_static("core.up");
    pub const DOWN: Id = Id::new_static("core.down");
    pub const LEFT: Id = Id::new_static("core.left");
    pub const RIGHT: Id = Id::new_static("core.right");
    pub const CONFIRM: Id = Id::new_static("core.confirm");
    pub const MENU: Id = Id::new_static("core.menu");
    pub const BACK: Id = Id::new_static("core.back");
    pub const HOME: Id = Id::new_static("core.home");
    pub const TOGGLE_NIGHT_MODE: Id = Id::new_static("core.toggle-night-mode");
    /// Opens the library slot.
    pub const LIBRARY: Id = Id::new_static("core.library");
    /// Opens the settings slot.
    pub const SETTINGS: Id = Id::new_static("core.settings");
    /// Saves what the screen shows as a picture, in `Screenshots` beside
    /// the library ([`Command::Screenshot`](crate::Command::Screenshot)).
    pub const SCREENSHOT: Id = Id::new_static("core.screenshot");
    /// Opens the tools menu ([`menus::TOOLS`](super::menus::TOOLS)).
    pub const TOOLS: Id = Id::new_static("core.tools");
    /// Puts the device to sleep: shows the sleep screen (slot
    /// [`SLEEP`](super::slots::SLEEP)), then the shell sleeps until it is
    /// woken (see [`Env::sleep`](crate::Env)). Where the system sleeps on
    /// its own (Android), nothing sends it.
    pub const SLEEP: Id = Id::new_static("core.sleep");
    /// Opens the quick panel from the top of the screen: the menu
    /// [`QUICK`](super::menus::QUICK) as a panel of controls (night mode,
    /// light, turning the screen, …). A swipe down from the top edge
    /// opens it.
    pub const QUICK_PANEL: Id = Id::new_static("core.quick-panel");
    /// Refreshes the whole screen with a flash, clearing what e-ink left
    /// of earlier pictures (ghosting).
    pub const REFRESH_SCREEN: Id = Id::new_static("core.refresh-screen");
}

/// Standard slots.
pub mod slots {
    use crate::Id;

    /// The start screen. The default is a dashboard, but any screen can be
    /// bound here: a file browser, a bookshelf, a single "continue" button.
    pub const HOME: Id = Id::new_static("core.home");
    /// Shows a book. Opened with the parameter [`PATH`](super::params::PATH).
    pub const READER: Id = Id::new_static("core.reader");
    /// Browses the library.
    pub const LIBRARY: Id = Id::new_static("core.library");
    /// Shows a menu (an overlay). Opened with the parameter
    /// [`MENU`](super::params::MENU).
    pub const MENU: Id = Id::new_static("core.menu");
    /// Shows and edits settings; with the parameter
    /// [`ID`](super::params::ID), the settings of that group, e.g. a
    /// widget's own (see [`widgets`](crate::widgets)).
    pub const SETTINGS: Id = Id::new_static("core.settings");
    /// Looks up a word (an overlay): opened with the parameters
    /// [`WORD`](super::params::WORD) and, if known,
    /// [`LANGUAGE`](super::params::LANGUAGE). The standard dictionary
    /// binds here; translators or encyclopedias can too.
    pub const LOOKUP: Id = Id::new_static("core.lookup");
    /// Looks a word up online (an overlay), with the same parameters as
    /// [`LOOKUP`]: an encyclopedia, an online dictionary, a translator.
    /// Dictionaries offer it beside their own entries when a screen is
    /// bound here ([`Env::has_slot`](crate::Env::has_slot)) and there is
    /// a network.
    pub const LOOKUP_ONLINE: Id = Id::new_static("core.lookup-online");
    /// Shows a menu as a panel of controls for quick changes (an overlay):
    /// parameter [`MENU`](super::params::MENU). The reader opens its
    /// font and layout menu this way.
    pub const PANEL: Id = Id::new_static("core.panel");
    /// Asks for text (usually an overlay with an on-screen keyboard).
    /// Parameters: [`TITLE`](super::params::TITLE) (shown above the
    /// field, already translated), [`TEXT`](super::params::TEXT) (the
    /// initial text), [`MODE`](super::params::MODE) and
    /// [`REPLY`](super::params::REPLY). When the text is confirmed the
    /// screen closes and emits the event named by `reply` with all of its
    /// parameters, `text` set to what was entered, so the asking plugin
    /// can pass along anything it needs to know (e.g. which setting). Bind
    /// another screen here for a different keyboard, handwriting or voice.
    pub const KEYBOARD: Id = Id::new_static("core.keyboard");
    /// Asks for text typed on another device, e.g. a phone that opens an
    /// address shown as a QR code: handy for passwords and keys. Same
    /// parameters as [`KEYBOARD`]; the text comes back as the event named
    /// by [`REPLY`](super::params::REPLY). The keyboard offers it where a
    /// screen is bound here and there is a network.
    pub const REMOTE_KEYBOARD: Id = Id::new_static("core.remote-keyboard");
    /// Shows a web page: parameter [`URL`](super::params::URL), and
    /// [`ID`](super::params::ID) different for every request (opening a
    /// slot with the parameters it is open with only brings it forward).
    /// The reader opens links out of a book this way, if a browser is
    /// bound here ([`Env::has_slot`](crate::Env::has_slot)).
    pub const BROWSER: Id = Id::new_static("core.browser");
    /// Chooses a folder, browsing the device's folders (with a way to make
    /// a new one). Parameters: [`ID`](super::params::ID), a setting the
    /// folder is written to, or [`REPLY`](super::params::REPLY), an event
    /// emitted with all of the parameters and [`PATH`](super::params::PATH)
    /// set to the folder; [`TEXT`](super::params::TEXT), where to start.
    pub const CHOOSE_FOLDER: Id = Id::new_static("core.choose-folder");
    /// Shows what is known of a book: parameter [`PATH`](super::params::PATH).
    pub const BOOK_INFO: Id = Id::new_static("core.book-info");
    /// Shows a short message ([`Command::Notify`](crate::Command::Notify))
    /// over whatever is open, for a few seconds: parameter
    /// [`TEXT`](super::params::TEXT). It gets no input and is closed by
    /// the kernel; bind another screen here for messages that look
    /// different. Its [`region`](crate::Screen::region) is where it shows.
    pub const NOTICE: Id = Id::new_static("core.notice");
    /// Translates text (an overlay): parameters
    /// [`TEXT`](super::params::TEXT) and, if known,
    /// [`LANGUAGE`](super::params::LANGUAGE) (the text's). The reader
    /// offers it for selected text when a screen is bound here and there
    /// is a network; the `translate` plugin binds DeepL, other services
    /// can take its place.
    pub const TRANSLATE: Id = Id::new_static("core.translate");
    /// Shown while the device sleeps (see
    /// [`actions::SLEEP`](super::actions::SLEEP)): a cover, a picture, a
    /// message. It closes itself when the device wakes
    /// ([`events::RESUME`](super::events::RESUME)).
    pub const SLEEP: Id = Id::new_static("core.sleep");
}

/// Standard menus. Plugins add entries with
/// [`Registrar::add_menu_entry`](crate::Registrar::add_menu_entry); users can
/// redefine any menu in `[menus]`.
pub mod menus {
    use crate::Id;

    /// The menu shown when a screen has no menu of its own: short and
    /// always the same (home, library, tools, night mode, settings).
    pub const MAIN: Id = Id::new_static("core.main");
    /// The tools plugins bring (statistics, catalogs, files, transfers,
    /// …): plugins add their entries here rather than to [`MAIN`], so the
    /// main menu stays short however many there are. Also the home
    /// screen's grid of tools.
    pub const TOOLS: Id = Id::new_static("core.tools");
    /// Extra pages on the main settings page, below the setting groups
    /// (e.g. the plugin manager). Entries are actions.
    pub const SETTINGS: Id = Id::new_static("core.settings-pages");
    /// The quick panel ([`actions::QUICK_PANEL`](super::actions::QUICK_PANEL)):
    /// switches and controls for quick changes, from every plugin.
    pub const QUICK: Id = Id::new_static("core.quick");
}

/// Standard parameter names.
pub mod params {
    /// A storage path, e.g. of the book to open.
    pub const PATH: &str = "path";
    /// The menu to show in the menu slot.
    pub const MENU: &str = "menu";
    /// An id, e.g. of the setting in `core.setting-changed`.
    pub const ID: &str = "id";
    /// A title, e.g. of a book or of what a keyboard asks for.
    pub const TITLE: &str = "title";
    /// Text, e.g. entered on the keyboard.
    pub const TEXT: &str = "text";
    /// What kind of text a keyboard asks for: `text` (the default),
    /// `number` or `password` (shown as dots).
    pub const MODE: &str = "mode";
    /// The event a screen emits with its result, e.g. the keyboard's text.
    pub const REPLY: &str = "reply";
    /// A position in a document: section and offset (see
    /// `document::Locator`).
    pub const SECTION: &str = "section";
    pub const OFFSET: &str = "offset";
    /// Reading progress in percent, e.g. `12.5`.
    pub const PROGRESS: &str = "progress";
    /// The page (from 1) and the number of pages, once a reader has
    /// counted them.
    pub const PAGE: &str = "page";
    pub const PAGES: &str = "pages";
    /// A word, e.g. to look up.
    pub const WORD: &str = "word";
    /// A BCP 47 language tag, e.g. of the book a word is from.
    pub const LANGUAGE: &str = "language";
    /// A web address, e.g. for [`slots::BROWSER`](super::slots::BROWSER).
    pub const URL: &str = "url";
    /// An action to run, e.g. when the title of a menu bar is tapped.
    pub const ACTION: &str = "action";
    /// Entries a menu leaves out this time, as ids separated by commas:
    /// what does not apply where it is opened (e.g. searching a comic).
    pub const HIDE: &str = "hide";
    /// Where something was before, e.g. a moved book
    /// ([`events::BOOK_MOVED`](super::events::BOOK_MOVED)).
    pub const FROM: &str = "from";
}

/// Events emitted by the kernel.
pub mod events {
    use crate::Id;

    /// After the home screen has been opened at start-up. Plugins use it to
    /// e.g. reopen the last book.
    pub const STARTED: Id = Id::new_static("core.started");
    /// After a setting changed; parameter [`ID`](super::params::ID).
    pub const SETTING_CHANGED: Id = Id::new_static("core.setting-changed");
    /// A book was opened for reading. Every reader plugin emits it, so home
    /// screens, statistics or sync plugins work with any reader. Parameters
    /// [`PATH`](super::params::PATH), [`TITLE`](super::params::TITLE),
    /// [`PROGRESS`](super::params::PROGRESS) and the position,
    /// [`SECTION`](super::params::SECTION) and
    /// [`OFFSET`](super::params::OFFSET). Not for documents in hidden
    /// folders ([`files::is_hidden`](crate::files::is_hidden)), e.g. web
    /// pages: they are no books.
    pub const BOOK_OPENED: Id = Id::new_static("core.book-opened");
    /// The reader moved to another page; same parameters as
    /// [`BOOK_OPENED`].
    pub const PAGE_TURNED: Id = Id::new_static("core.page-turned");
    /// The book read was closed (the reader left, or another book
    /// opened); same parameters as [`BOOK_OPENED`], where it was left.
    pub const BOOK_CLOSED: Id = Id::new_static("core.book-closed");
    /// The last page of a book was reached, once each time it is opened;
    /// same parameters as [`BOOK_OPENED`].
    pub const BOOK_FINISHED: Id = Id::new_static("core.book-finished");
    /// A book file was moved or renamed: parameters
    /// [`FROM`](super::params::FROM) (the old path) and
    /// [`PATH`](super::params::PATH) (the new one). Plugins that keep data
    /// by a book's path move it along (progress, notes, statistics).
    pub const BOOK_MOVED: Id = Id::new_static("core.book-moved");
    /// Books came into the library or left it (received, downloaded,
    /// copied, deleted): views of the library read it again. Plugins that
    /// change the library emit it ([`library::changed`](crate::library::changed));
    /// a move is [`BOOK_MOVED`].
    pub const LIBRARY_CHANGED: Id = Id::new_static("core.library-changed");
    /// The bookmarks or highlights of a book changed
    /// ([`annotations`](crate::annotations)): parameter
    /// [`PATH`](super::params::PATH). Readers show them anew, syncs pass
    /// them on.
    pub const ANNOTATIONS_CHANGED: Id = Id::new_static("core.annotations-changed");
    /// A word of a book was looked up: parameters
    /// [`WORD`](super::params::WORD), [`LANGUAGE`](super::params::LANGUAGE),
    /// [`TEXT`](super::params::TEXT) (its sentence), the book's
    /// [`PATH`](super::params::PATH) and [`TITLE`](super::params::TITLE),
    /// and where the word is ([`SECTION`](super::params::SECTION),
    /// [`OFFSET`](super::params::OFFSET)).
    pub const WORD_LOOKED_UP: Id = Id::new_static("core.word-looked-up");
    /// A menu was redefined or reset; parameter [`ID`](super::params::ID).
    pub const MENU_CHANGED: Id = Id::new_static("core.menu-changed");
    /// The device goes to sleep or the app to the background: a moment to
    /// save or send what is pending.
    pub const SUSPEND: Id = Id::new_static("core.suspend");
    /// Back from [`SUSPEND`].
    pub const RESUME: Id = Id::new_static("core.resume");
    /// The battery or the time zone changed; status lines redraw.
    pub const STATUS_CHANGED: Id = Id::new_static("core.status-changed");
}

/// Status items of the kernel (see [`status`](crate::status)).
pub mod status_items {
    use crate::Id;

    /// The time, as set in `core.clock-format`.
    pub const CLOCK: Id = Id::new_static("core.status.clock");
    /// The battery's charge.
    pub const BATTERY: Id = Id::new_static("core.status.battery");
    /// Today in words ([`time::date_text`](crate::time::date_text)).
    pub const DATE: Id = Id::new_static("core.status.date");
}

/// Standard screens.
pub mod screens {
    use crate::Id;

    /// Shown when a slot has no screen bound to it.
    pub const PLACEHOLDER: Id = Id::new_static("core.placeholder");
    /// Chooses the status items a setting lists (see
    /// [`ui::items`](crate::ui::items)): name it as the setting's editor.
    pub const STATUS_ITEMS: Id = Id::new_static("core.status-items");
    /// Chooses the status lines a setting lists.
    pub const STATUS_LINES: Id = Id::new_static("core.status-lines");
}

/// Global settings.
pub mod settings {
    use crate::Id;

    /// UI language tag, e.g. `en` or `de`.
    pub const LANGUAGE: Id = Id::new_static("core.language");
    /// Draw everything inverted.
    pub const NIGHT_MODE: Id = Id::new_static("core.night-mode");
    /// How far in from the top and the bottom edge a swipe begins to count
    /// as one in from the edge, in millimetres (0: none).
    pub const EDGE_ENDS: Id = Id::new_static("core.edge-ends");
    /// The same at the left and right edges.
    pub const EDGE_SIDES: Id = Id::new_static("core.edge-sides");
    /// Say what the buttons move to (a row, a button), where the device
    /// speaks: for using QuireInk without seeing the screen.
    pub const SPEAK_FOCUS: Id = Id::new_static("core.speak-focus");
    /// Partial refreshes between two full refreshes; `0` disables full
    /// refreshes for ghosting removal.
    pub const FULL_REFRESH_INTERVAL: Id = Id::new_static("core.full-refresh-interval");
    /// Whether another screen opening flashes the panel (a full refresh,
    /// no traces of the last one, but slow on some panels); else it counts
    /// as a page turn towards [`FULL_REFRESH_INTERVAL`].
    pub const FLASH_SCREENS: Id = Id::new_static("core.flash-screens");
    /// Minutes without use after which the device goes to sleep by itself
    /// (`0`: never), where QuireInk puts it to sleep
    /// ([`Env::sleep`](crate::Env)). Turning pages, also by themselves,
    /// counts as use; reading aloud, downloads and servers keep it awake.
    pub const SLEEP_AFTER: Id = Id::new_static("core.sleep-after");
    /// How the screen is turned: one of [`ROTATIONS`], where the shell
    /// can turn it (see [`MenuNeed::Rotation`](crate::MenuNeed)).
    pub const ROTATION: Id = Id::new_static("core.rotation");
    /// Whether a row below the screen shows what the buttons under it do,
    /// where a device has buttons in a row there (see
    /// [`MenuNeed::ButtonRow`](crate::MenuNeed)).
    pub const BUTTON_HINTS: Id = Id::new_static("core.button-hints");
    /// The values of [`ROTATION`]: upright, on its side (landscape),
    /// upside down, on its other side.
    pub const ROTATIONS: [&str; 4] = [
        "core.rotation.upright",
        "core.rotation.landscape",
        "core.rotation.upside-down",
        "core.rotation.landscape-flipped",
    ];
    /// The colour black is shown in: one of [`TEXT_COLORS`], where the
    /// shell tints the screen (see [`MenuNeed::Colors`](crate::MenuNeed)).
    /// Everything is drawn in grays; the shell shows black as this colour,
    /// white as [`PAPER_COLOR`] and the grays between them.
    pub const TEXT_COLOR: Id = Id::new_static("core.text-color");
    /// The colour white is shown in: one of [`PAPER_COLORS`].
    pub const PAPER_COLOR: Id = Id::new_static("core.paper-color");
    /// The own text colour, used when [`TEXT_COLOR`] is [`OWN_TEXT`]: a
    /// colour code such as `#5b4636` (see [`parse_rgb`]).
    pub const TEXT_COLOR_OWN: Id = Id::new_static("core.text-color-own");
    /// The own paper colour, used when [`PAPER_COLOR`] is [`OWN_PAPER`].
    pub const PAPER_COLOR_OWN: Id = Id::new_static("core.paper-color-own");
    /// The value of [`TEXT_COLOR`] that takes [`TEXT_COLOR_OWN`].
    pub const OWN_TEXT: &str = "core.color.own";
    /// The value of [`PAPER_COLOR`] that takes [`PAPER_COLOR_OWN`].
    pub const OWN_PAPER: &str = "core.paper.own";
    /// The values of [`TEXT_COLOR`]: dark colours, then one's own.
    pub const TEXT_COLORS: [&str; 9] = [
        "core.color.black",
        "core.color.dark-gray",
        "core.color.gray",
        "core.color.brown",
        "core.color.dark-blue",
        "core.color.dark-green",
        "core.color.burgundy",
        "core.color.violet",
        OWN_TEXT,
    ];
    /// The values of [`PAPER_COLOR`]: light colours, then one's own.
    pub const PAPER_COLORS: [&str; 9] = [
        "core.paper.white",
        "core.paper.cream",
        "core.paper.sepia",
        "core.paper.sand",
        "core.paper.light-gray",
        "core.paper.mint",
        "core.paper.light-blue",
        "core.paper.rose",
        OWN_PAPER,
    ];

    /// The red, green and blue of a value of [`TEXT_COLOR`] or
    /// [`PAPER_COLOR`] other than one's own.
    pub fn rgb(value: &str) -> Option<[u8; 3]> {
        Some(match value {
            "core.color.black" => [0, 0, 0],
            "core.color.dark-gray" => [51, 51, 51],
            "core.color.gray" => [85, 85, 85],
            "core.color.brown" => [91, 70, 54],
            "core.color.dark-blue" => [28, 42, 68],
            "core.color.dark-green" => [30, 58, 42],
            "core.color.burgundy" => [94, 28, 36],
            "core.color.violet" => [58, 42, 82],
            "core.paper.white" => [255, 255, 255],
            "core.paper.cream" => [250, 246, 236],
            "core.paper.sepia" => [244, 236, 216],
            "core.paper.sand" => [233, 222, 196],
            "core.paper.light-gray" => [232, 232, 232],
            "core.paper.mint" => [226, 240, 230],
            "core.paper.light-blue" => [226, 236, 244],
            "core.paper.rose" => [246, 232, 232],
            _ => return None,
        })
    }

    /// The red, green and blue of a colour code: `#5b4636`, `5b4636` or
    /// the short `#543`; `None` for anything else.
    pub fn parse_rgb(code: &str) -> Option<[u8; 3]> {
        let hex = code.trim().trim_start_matches('#');
        let digit = |i: usize| u8::from_str_radix(hex.get(i..i + 1)?, 16).ok();
        match hex.len() {
            3 => Some([digit(0)? * 17, digit(1)? * 17, digit(2)? * 17]),
            6 => {
                let byte = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
                Some([byte(0)?, byte(2)?, byte(4)?])
            }
            _ => None,
        }
    }
    /// Margin of menus and lists, in millimetres.
    pub const UI_MARGIN: Id = Id::new_static("core.ui-margin");
    /// Height of list rows, in percent of a line of text.
    pub const UI_ROW: Id = Id::new_static("core.ui-row");
    /// Width of lines and borders, in tenths of a millimetre.
    pub const UI_LINES: Id = Id::new_static("core.ui-lines");
    /// Headers dark ([`HEADER_DARK`]) or light ([`HEADER_LIGHT`]).
    pub const UI_HEADER: Id = Id::new_static("core.ui-header");
    pub const HEADER_DARK: &str = "core.header.dark";
    pub const HEADER_LIGHT: &str = "core.header.light";
    /// Corners of highlights, buttons and boxes: one of [`CORNERS`].
    pub const UI_CORNERS: Id = Id::new_static("core.ui-corners");
    /// The values of [`UI_CORNERS`]: square, slightly rounded, round.
    pub const CORNERS: [&str; 3] = [
        "core.corners.square",
        "core.corners.small",
        "core.corners.round",
    ];
    /// Gray text such as hints and notes: as it is, or black for strong
    /// contrast ([`CONTRAST_STRONG`]), e.g. on screens with few grays.
    pub const UI_CONTRAST: Id = Id::new_static("core.ui-contrast");
    pub const CONTRAST_NORMAL: &str = "core.contrast.normal";
    pub const CONTRAST_STRONG: &str = "core.contrast.strong";
    /// How long short messages show, in seconds.
    pub const NOTICE_SECONDS: Id = Id::new_static("core.notice-seconds");
    /// Milliseconds a button or finger must be held for a long press.
    pub const LONG_PRESS_MS: Id = Id::new_static("core.long-press-ms");
    /// Base size of user interface text in points; UI metrics scale with it.
    pub const UI_SIZE: Id = Id::new_static("core.ui-size");
    /// The active key map preset (see [`keymap_presets`](super::keymap_presets)).
    pub const KEYMAP_PRESET: Id = Id::new_static("core.keymap-preset");
    /// Scoped settings changed now apply to the active scope only, e.g.
    /// the open book ("Only this book"); switching it off returns the
    /// scope to the global values.
    pub const SCOPE_ONLY: Id = Id::new_static("core.scope-only");
    /// The font family for reading: [`DEFAULT_FONT`] (the plugins'
    /// choice) or one of the registered families.
    pub const READING_FONT: Id = Id::new_static("core.reading-font");
    /// The value of [`READING_FONT`] for the plugins' choice.
    pub const DEFAULT_FONT: &str = "core.font.default";
    /// `core.clock.24h` or `core.clock.12h`.
    pub const CLOCK_FORMAT: Id = Id::new_static("core.clock-format");
    /// The time zone: `core.tz.auto` (the system's) or one of
    /// [`time::TIME_ZONES`](crate::time::TIME_ZONES).
    pub const TIME_ZONE: Id = Id::new_static("core.time-zone");
    /// A POSIX `TZ` rule that overrides [`TIME_ZONE`] when not empty.
    pub const TIME_ZONE_RULE: Id = Id::new_static("core.time-zone-rule");
}

/// Key map presets: named sets of bindings users switch between, e.g. for
/// devices with only two buttons. Plugins add more with
/// [`Registrar::add_keymap_preset`](crate::Registrar::add_keymap_preset).
pub mod keymap_presets {
    use crate::Id;

    /// No preset: the plugins' default bindings.
    pub const STANDARD: Id = Id::new_static(STANDARD_NAME);
    /// [`STANDARD`] as a static string, e.g. for setting values.
    pub const STANDARD_NAME: &str = "core.standard";
    /// Everything with "forward" and "back": short press moves, long press
    /// on forward confirms (opens the menu while reading), long press on
    /// back goes back.
    pub const TWO_BUTTONS: Id = Id::new_static("core.two-buttons");
    /// Taps on the left turn forward, on the right back: for the left
    /// hand.
    pub const LEFT_HANDED: Id = Id::new_static("core.left-handed");
}

/// The user's configuration file on the device's storage. Settings changed
/// in the UI are written back to it; comments and order are preserved.
pub const CONFIG_FILE: &str = "quireink.ini";

/// The folder with data plugins (one subfolder each) on the storage,
/// unless the shell names others.
pub const PLUGIN_DIR: &str = "plugins";

/// Font roles.
/// Groups of the generated settings screen; plugins can add their own.
pub mod groups {
    use crate::Id;

    /// Book text: font size, spacing, hyphenation, …
    pub const READING: Id = Id::new_static("core.group.reading");
    /// Screen: night mode, menu size, refresh.
    pub const DISPLAY: Id = Id::new_static("core.group.display");
    /// Buttons and gestures.
    pub const CONTROLS: Id = Id::new_static("core.group.controls");
    /// Books and folders.
    pub const LIBRARY: Id = Id::new_static("core.group.library");
    /// Language and everything about the device.
    pub const SYSTEM: Id = Id::new_static("core.group.system");
    /// How menus and lists look: text size, margins, rows, lines, headers.
    /// Themes (data plugins) set its defaults.
    pub const LOOK: Id = Id::new_static("core.group.look");
    /// Settings whose plugin names no registered group.
    pub const OTHER: Id = Id::new_static("core.group.other");
}

/// Sections of the settings' main page, with the setting groups and the
/// pages in [`menus::SETTINGS`] placed by their order: reading 1 to 49,
/// the device 51 to 199, customising QuireInk from 201.
pub mod settings_sections {
    use crate::Id;

    /// Text, status bars, library, statistics, synchronisation.
    pub const READING: Id = Id::new_static("core.settings-section.reading");
    /// Display, controls, language and the system.
    pub const DEVICE: Id = Id::new_static("core.settings-section.device");
    /// Menus, plugins, advanced settings.
    pub const CUSTOMISE: Id = Id::new_static("core.settings-section.customise");
}

/// Parts of setting groups shared by plugins, in the order shown: a
/// setting names one with
/// [`SettingSpec::section`](crate::SettingSpec::section). Other parts
/// (any translation key) follow these.
pub mod setting_sections {
    /// The reading font, its size and weight.
    pub const FONT: &str = "core.setting-section.font";
    /// How a page is set: spacing, margins, justification, hyphenation.
    pub const PAGE: &str = "core.setting-section.page";
    /// What happens when reading: books opened again, bookmarks.
    pub const BEHAVIOUR: &str = "core.setting-section.behaviour";

    /// The standard parts in their order.
    pub const ORDER: [&str; 3] = [FONT, PAGE, BEHAVIOUR];
}

/// Sections of the main menu ([`menus::MAIN`]): plugins put their
/// entries in one by their order (reading 1 to 29, tools 31 to 79,
/// system 81 to 99).
pub mod sections {
    use crate::Id;

    /// Home, library and the like.
    pub const READING: Id = Id::new_static("core.section.reading");
    /// What plugins add: files, statistics, transfers, catalogs.
    pub const TOOLS: Id = Id::new_static("core.section.tools");
    /// Settings and the device.
    pub const SYSTEM: Id = Id::new_static("core.section.system");
}

pub mod fonts {
    use crate::Id;

    /// Menus, labels and other user interface text.
    pub const UI: Id = Id::new_static("core.font-ui");
    /// Book text.
    pub const READING: Id = Id::new_static("core.font-reading");
    /// Bold book text; readers fall back to [`READING`] without it.
    pub const READING_BOLD: Id = Id::new_static("core.font-reading-bold");
    /// Italic book text; readers fall back to [`READING`] without it.
    pub const READING_ITALIC: Id = Id::new_static("core.font-reading-italic");
    /// Bold italic book text; readers fall back to [`READING_BOLD`].
    pub const READING_BOLD_ITALIC: Id = Id::new_static("core.font-reading-bold-italic");
    /// Icons (see [`icons`](crate::icons)); without a font here, menus
    /// show text only.
    pub const ICONS: Id = Id::new_static("core.font-icons");
}

/// The name of the key map layer that applies everywhere.
pub const GLOBAL_KEYMAP_LAYER: &str = "global";

#[cfg(test)]
mod tests {
    use super::settings::*;

    #[test]
    fn every_colour_but_ones_own_is_known() {
        for value in TEXT_COLORS.iter().chain(&PAPER_COLORS) {
            let own = *value == OWN_TEXT || *value == OWN_PAPER;
            assert_eq!(rgb(value).is_none(), own, "{value}");
        }
    }

    #[test]
    fn colour_codes() {
        assert_eq!(parse_rgb("#5b4636"), Some([0x5b, 0x46, 0x36]));
        assert_eq!(parse_rgb(" F4ECD8 "), Some([0xf4, 0xec, 0xd8]));
        assert_eq!(parse_rgb("#543"), Some([0x55, 0x44, 0x33]));
        for wrong in ["", "#", "#12345", "#ggg", "blue", "#1234567"] {
            assert_eq!(parse_rgb(wrong), None, "{wrong}");
        }
    }
}
