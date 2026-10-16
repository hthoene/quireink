use alloc::borrow::Cow;
use core::fmt;

use crate::action::ActionSpec;
use crate::event::EventHandler;
use crate::fonts::FontSpec;
use crate::formats::FormatSpec;
use crate::hyphenation::HyphenationSpec;
use crate::id::Id;
use crate::menus::{MenuEntry, MenuSection};
use crate::screen::ScreenSpec;
use crate::settings::{SettingGroupSpec, SettingSpec};
use crate::status::{StatusItemSpec, StatusLineSpec};
use crate::version::Version;

/// Static information about a plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    /// Plugin id; also the namespace of all its contributions.
    pub id: Id,
    /// Human-readable name in English. Plugins translate it with the key
    /// `<id>.plugin.name`.
    pub name: Cow<'static, str>,
    /// What the plugin does, one English sentence for plugin managers.
    /// Plugins translate it with the key `<id>.plugin.description`.
    pub description: Cow<'static, str>,
    pub version: Version,
    /// The SDK version the plugin was built against.
    pub sdk: Version,
}

/// A QuireInk plugin.
///
/// `register` is called once at start-up. It adds the plugin's contributions;
/// the kernel decides afterwards which ones are active (user configuration
/// and priorities may override them).
pub trait Plugin {
    fn manifest(&self) -> Manifest;

    fn register(&self, registrar: &mut dyn Registrar) -> Result<(), RegisterError>;
}

/// Priority of a default binding. Higher wins; an explicit user setting
/// always wins over any priority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Priority(pub i16);

impl Priority {
    /// Used only when nothing else is available (diagnostics, placeholders).
    pub const FALLBACK: Priority = Priority(-100);
    /// The normal priority of first-party defaults.
    pub const DEFAULT: Priority = Priority(0);
    /// For plugins whose purpose is to replace a default.
    pub const PREFERRED: Priority = Priority(100);
}

/// A named UI role (home, library, reader, …) bound to a screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotSpec {
    pub id: Id,
    /// Translation key of the slot's display name.
    pub title: Cow<'static, str>,
}

/// The interface through which a plugin adds contributions.
///
/// Every contribution id must be in the plugin's namespace (`<plugin>.…`).
/// Bindings and key maps may refer to contributions of other plugins.
pub trait Registrar {
    /// The id of the plugin being registered.
    fn plugin(&self) -> &Id;

    fn add_action(&mut self, action: ActionSpec) -> Result<(), RegisterError>;

    fn add_screen(&mut self, screen: ScreenSpec) -> Result<(), RegisterError>;

    fn add_slot(&mut self, slot: SlotSpec) -> Result<(), RegisterError>;

    /// Proposes `screen` for `slot`. The binding with the highest priority
    /// wins unless the user configured the slot explicitly.
    fn bind_slot(&mut self, slot: Id, screen: Id, priority: Priority) -> Result<(), RegisterError>;

    fn add_setting(&mut self, setting: SettingSpec) -> Result<(), RegisterError>;

    /// Adds default key bindings in the QuireInk config format:
    ///
    /// ```ini
    /// [keymap.global]
    /// button.page-forward = core.next-page
    /// long.back = core.home
    /// tap.center = core.menu
    /// ```
    fn add_keymap(&mut self, source: Cow<'static, str>) -> Result<(), RegisterError>;
    /// A stylesheet for books, applied after a book's own (where the
    /// format reads CSS, e.g. EPUB), e.g. for a theme that sets books
    /// apart; see [`Env::stylesheets`](crate::Env).
    fn add_stylesheet(&mut self, css: Cow<'static, str>) -> Result<(), RegisterError>;

    /// Adds a key map preset (same format as [`Registrar::add_keymap`]) that
    /// users can select with the `core.keymap-preset` setting. Its title
    /// is the translation of its id.
    fn add_keymap_preset(&mut self, id: Id, source: Cow<'static, str>)
    -> Result<(), RegisterError>;

    /// Adds a font. Bitmap packs work everywhere; scalable fonts need the
    /// `outline` feature.
    fn add_font(&mut self, font: FontSpec) -> Result<(), RegisterError>;

    /// Adds a font file on the storage (OpenType/TrueType): its family and
    /// style are read from its header, and it is loaded only when its
    /// family is chosen for reading. Needs scalable font support.
    fn add_font_file(&mut self, id: Id, path: alloc::string::String) -> Result<(), RegisterError>;

    /// Proposes a font for a role such as
    /// [`builtin::fonts::UI`](crate::builtin::fonts::UI).
    fn bind_font(&mut self, role: Id, font: Id, priority: Priority) -> Result<(), RegisterError>;

    /// Puts an action into a menu. Menus can be extended by any plugin and
    /// redefined by the user.
    fn add_menu_entry(&mut self, entry: MenuEntry) -> Result<(), RegisterError>;

    /// Adds a heading to a menu; entries ordered after it, up to the next
    /// heading, belong to it.
    fn add_menu_section(&mut self, section: MenuSection) -> Result<(), RegisterError>;

    /// Gives an action or setting an icon by name (see
    /// [`icons`](crate::icons)), shown with it in menus. Any item may get
    /// one, e.g. by an icon theme; the last registered wins.
    fn add_icon(&mut self, item: Id, icon: Cow<'static, str>) -> Result<(), RegisterError>;

    /// Offers an action or setting in menus only on devices that meet
    /// `need`, e.g. reading aloud only where the device can speak.
    fn set_menu_need(&mut self, item: Id, need: crate::MenuNeed) -> Result<(), RegisterError>;

    /// Adds a group for the settings screen.
    fn add_setting_group(&mut self, group: SettingGroupSpec) -> Result<(), RegisterError>;

    /// Adds a file format.
    fn add_format(&mut self, format: FormatSpec) -> Result<(), RegisterError>;

    /// Adds hyphenation patterns for a language.
    fn add_hyphenation(&mut self, spec: HyphenationSpec) -> Result<(), RegisterError>;

    /// Subscribes to an event, e.g.
    /// [`builtin::events::STARTED`](crate::builtin::events::STARTED) or one
    /// defined by another plugin.
    fn subscribe(&mut self, event: Id, handler: EventHandler) -> Result<(), RegisterError>;

    /// Adds an item that status lines can show, e.g. a clock or the
    /// reading time left.
    fn add_status_item(&mut self, item: StatusItemSpec) -> Result<(), RegisterError>;

    /// Adds a filter of book text before it is laid out (see
    /// [`filters`](crate::filters)).
    fn add_text_filter(
        &mut self,
        filter: crate::filters::TextFilterSpec,
    ) -> Result<(), RegisterError>;

    /// Adds a widget, e.g. for the home screen (see
    /// [`widgets`](crate::widgets)).
    fn add_widget(&mut self, widget: crate::widgets::WidgetSpec) -> Result<(), RegisterError>;

    /// Adds a whole status line, drawn by the plugin (see
    /// [`status`](crate::status)).
    fn add_status_line(&mut self, line: StatusLineSpec) -> Result<(), RegisterError>;

    /// Publishes a service for other plugins as `id` (see
    /// [`services`](crate::services)).
    fn add_service(
        &mut self,
        id: Id,
        service: alloc::boxed::Box<dyn core::any::Any>,
    ) -> Result<(), RegisterError>;

    /// Adds `contribution` to `point`, a point another plugin defined
    /// (see [`services`](crate::services)).
    fn contribute(
        &mut self,
        point: Id,
        contribution: Cow<'static, str>,
    ) -> Result<(), RegisterError>;

    /// Adds a translation catalog (see `quireink-i18n`).
    fn add_translations(&mut self, source: Cow<'static, str>) -> Result<(), RegisterError>;

    /// Changes the default of any plugin's setting, written as in
    /// `quireink.ini` (e.g. a theme's text size). Device defaults and the
    /// user's choice still win; unknown settings are reported once all
    /// plugins are registered.
    fn set_setting_default(
        &mut self,
        setting: Id,
        value: Cow<'static, str>,
    ) -> Result<(), RegisterError>;
}

/// Why a contribution was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RegisterError {
    /// The id is not in the plugin's namespace.
    Namespace { id: Id },
    /// An id was registered twice.
    Duplicate { id: Id },
    /// A data contribution could not be parsed.
    Parse {
        line: u32,
        message: Cow<'static, str>,
    },
    /// Something else is wrong with the contribution.
    Invalid { message: Cow<'static, str> },
}

impl fmt::Display for RegisterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RegisterError::Namespace { id } => {
                write!(f, "`{id}` is outside the plugin's namespace")
            }
            RegisterError::Duplicate { id } => write!(f, "`{id}` is already registered"),
            RegisterError::Parse { line, message } => write!(f, "line {line}: {message}"),
            RegisterError::Invalid { message } => f.write_str(message),
        }
    }
}

impl core::error::Error for RegisterError {}
