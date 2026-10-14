use crate::id::Id;
use crate::settings::SettingValue;
use quireink_platform::RefreshMode;

/// A request from a plugin to the kernel.
///
/// Plugins never mutate kernel state directly; commands are queued and
/// applied by the kernel after the plugin returns.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Command {
    /// Shows the screen bound to a slot. If that slot is already open, the
    /// screens above it are closed.
    OpenSlot(Id),
    /// Opens a specific screen on top of the current one.
    OpenScreen(Id),
    /// Closes the current screen (no-op on the root screen).
    Back,
    /// Changes a setting; invalid values are rejected with a warning.
    SetSetting(Id, SettingValue),
    /// Redraws the current screen completely.
    Redraw(RefreshMode),
}
