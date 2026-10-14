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
    /// bound here — a file browser, a bookshelf, a single "continue" button.
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
    /// A book's title.
    pub const TITLE: &str = "title";
    /// Reading progress in percent, e.g. `12.5`.
    pub const PROGRESS: &str = "progress";
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
    /// [`PROGRESS`](super::params::PROGRESS).
    pub const BOOK_OPENED: Id = Id::new_static("core.book-opened");
    /// The reader moved to another page; same parameters as
    /// [`BOOK_OPENED`].
    pub const PAGE_TURNED: Id = Id::new_static("core.page-turned");
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
}

/// The name of the key map layer that applies everywhere.
pub const GLOBAL_KEYMAP_LAYER: &str = "global";
