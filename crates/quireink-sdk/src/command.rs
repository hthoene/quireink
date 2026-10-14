use crate::id::Id;
use crate::params::Params;
use crate::settings::SettingValue;
use quireink_platform::RefreshMode;

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
