use alloc::borrow::Cow;
use core::fmt;

use crate::action::ActionSpec;
use crate::event::EventHandler;
use crate::fonts::FontSpec;
use crate::formats::FormatSpec;
use crate::id::Id;
use crate::menus::MenuEntry;
use crate::screen::ScreenSpec;
use crate::settings::SettingSpec;
use crate::version::Version;

/// Static information about a plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    /// Plugin id; also the namespace of all its contributions.
    pub id: Id,
    /// Human-readable name.
    pub name: Cow<'static, str>,
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

    /// Adds a key map preset (same format as [`Registrar::add_keymap`]) that
    /// users can select with the `core.keymap-preset` setting. Its title
    /// is the translation of its id.
    fn add_keymap_preset(&mut self, id: Id, source: Cow<'static, str>)
    -> Result<(), RegisterError>;

    /// Adds a font. Bitmap packs work everywhere; scalable fonts need the
    /// `outline` feature.
    fn add_font(&mut self, font: FontSpec) -> Result<(), RegisterError>;

    /// Proposes a font for a role such as
    /// [`builtin::fonts::UI`](crate::builtin::fonts::UI).
    fn bind_font(&mut self, role: Id, font: Id, priority: Priority) -> Result<(), RegisterError>;

    /// Puts an action into a menu. Menus can be extended by any plugin and
    /// redefined by the user.
    fn add_menu_entry(&mut self, entry: MenuEntry) -> Result<(), RegisterError>;

    /// Adds a file format.
    fn add_format(&mut self, format: FormatSpec) -> Result<(), RegisterError>;

    /// Subscribes to an event, e.g.
    /// [`builtin::events::STARTED`](crate::builtin::events::STARTED) or one
    /// defined by another plugin.
    fn subscribe(&mut self, event: Id, handler: EventHandler) -> Result<(), RegisterError>;

    /// Adds a translation catalog (see `quireink-i18n`).
    fn add_translations(&mut self, source: Cow<'static, str>) -> Result<(), RegisterError>;
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
