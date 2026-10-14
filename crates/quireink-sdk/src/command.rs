use crate::id::Id;
use crate::params::Params;
use crate::settings::SettingValue;
use quireink_platform::RefreshMode;

/// An HTTP request of a plugin. When it is answered the kernel emits
/// `reply` with `params` plus `status` (the HTTP status; `0` when the
/// request failed, e.g. without network) and `body` (the response as text).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    /// `GET`, `PUT`, `POST`, …
    pub method: alloc::string::String,
    pub url: alloc::string::String,
    pub headers: alloc::vec::Vec<(alloc::string::String, alloc::string::String)>,
    pub body: Option<alloc::string::String>,
    pub reply: Id,
    pub params: Params,
}

/// Parameters of an HTTP reply event (see [`HttpRequest`]).
pub mod http_params {
    pub const STATUS: &str = "status";
    pub const BODY: &str = "body";
}

/// A request from a plugin to the kernel.
///
/// Plugins never mutate kernel state directly; commands are queued and
/// applied by the kernel after the plugin returns.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Command {
    /// Shows the screen bound to a slot, passing `Params` to it. If that
    /// slot is already open, the screens above it are closed.
    OpenSlot(Id, Params),
    /// Opens a specific screen on top of the current one.
    OpenScreen(Id, Params),
    /// Closes the current screen (no-op on the root screen).
    Back,
    /// Changes a setting; invalid values are rejected with a warning.
    SetSetting(Id, SettingValue),
    /// Redraws the current screen completely.
    Redraw(RefreshMode),
    /// Notifies all subscribers of an event, then every open screen.
    Emit(Id, Params),
    /// Runs an action as if the user had triggered it (e.g. from a menu).
    RunAction(Id),
    /// Switches a plugin on or off in `quireink.ini`; applies after a
    /// restart. The kernel refuses plugins that cannot be switched off
    /// (see [`PluginInfo::can_switch_off`](crate::PluginInfo)).
    SetPluginEnabled(Id, bool),
    /// Makes an HTTP request (see [`HttpRequest`]); the answer comes as
    /// the request's reply event. Check [`Env::network`](crate::Env) first.
    Http(HttpRequest),
    /// Uses the settings stored in the file at this storage path for
    /// [scoped](crate::SettingSpec::scoped) settings, e.g. those of the
    /// book being read, while the screen that sent it stays open; `None`
    /// ends the scope.
    SetScope(Option<alloc::string::String>),
    /// Redefines a menu with exactly these entries (actions, settings or
    /// submenus), saved in `[menus]` of `quireink.ini`; `None` returns to
    /// the entries plugins contribute.
    SetMenu(Id, Option<alloc::vec::Vec<Id>>),
    /// Restarts QuireInk, e.g. to apply switched plugins. The shell builds
    /// a new kernel; the reading position is already saved.
    Restart,
}

impl Command {
    /// Opens a slot without parameters.
    pub fn open_slot(slot: Id) -> Command {
        Command::OpenSlot(slot, Params::new())
    }
}
