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
    /// Shows and edits settings.
    pub const SETTINGS: Id = Id::new_static("core.settings");
    /// Looks up a word (an overlay): opened with the parameters
    /// [`WORD`](super::params::WORD) and, if known,
    /// [`LANGUAGE`](super::params::LANGUAGE). The standard dictionary
    /// binds here; translators or encyclopedias can too.
    pub const LOOKUP: Id = Id::new_static("core.lookup");
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
    /// Shows a web page: parameter [`URL`](super::params::URL), and
    /// [`ID`](super::params::ID) different for every request (opening a
    /// slot with the parameters it is open with only brings it forward).
    /// The reader opens links out of a book this way, if a browser is
    /// bound here ([`Env::has_slot`](crate::Env::has_slot)).
    pub const BROWSER: Id = Id::new_static("core.browser");
}

/// Standard menus. Plugins add entries with
/// [`Registrar::add_menu_entry`](crate::Registrar::add_menu_entry); users can
/// redefine any menu in `[menus]`.
pub mod menus {
    use crate::Id;

    /// The menu shown when a screen has no menu of its own.
    pub const MAIN: Id = Id::new_static("core.main");
    /// Extra pages on the main settings page, below the setting groups
    /// (e.g. the plugin manager). Entries are actions.
    pub const SETTINGS: Id = Id::new_static("core.settings-pages");
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
}

/// Standard screens.
pub mod screens {
    use crate::Id;

    /// Shown when a slot has no screen bound to it.
    pub const PLACEHOLDER: Id = Id::new_static("core.placeholder");
}

/// Global settings.
pub mod settings {
    use crate::Id;

    /// UI language tag, e.g. `en` or `de`.
    pub const LANGUAGE: Id = Id::new_static("core.language");
    /// Draw everything inverted.
    pub const NIGHT_MODE: Id = Id::new_static("core.night-mode");
    /// Partial refreshes between two full refreshes; `0` disables full
    /// refreshes for ghosting removal.
    pub const FULL_REFRESH_INTERVAL: Id = Id::new_static("core.full-refresh-interval");
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
