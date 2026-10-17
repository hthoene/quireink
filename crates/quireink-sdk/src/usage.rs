//! What the device did since QuireInk started, as the kernel counts it
//! ([`Env::usage`](crate::Env::usage)): for plugins that relate the
//! battery's use to it. Refreshing the screen takes much of an e-reader's
//! power; the processor being awake much of the rest.

use alloc::vec::Vec;

use quireink_platform::{Rect, RefreshMode};

use crate::id::Id;

/// Counts since QuireInk started (on a device whose sleep cuts the power,
/// since it last woke).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Usage {
    /// Refreshes of the screen asked for, by mode: fast, partial, full.
    pub refreshes: [u32; 3],
    /// Pixels refreshed, all refreshes together.
    pub pixels: u64,
    /// Button presses and releases, taps and swipes handled.
    pub inputs: u32,
    /// Times the device went to sleep (or the app to the background).
    pub sleeps: u32,
    /// Time the panel took for its refreshes, where the shell says
    /// (the kernel's `panel_busy`).
    pub panel_ms: u64,
    /// What each plugin took (`core`: the kernel itself), as of the last
    /// input or tick.
    pub plugins: Vec<PluginUsage>,
}

/// What a plugin took.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginUsage {
    pub plugin: Id,
    /// Time its code ran (its screens drawing and handling input, its
    /// handlers), in microseconds, where the device has a clock for it.
    pub busy_us: u64,
    /// Times its code was called.
    pub calls: u32,
    /// Time one of its screens was on top, in milliseconds.
    pub shown_ms: u64,
    /// Refreshes while one of its screens was on top: fast, partial, full.
    pub refreshes: [u32; 3],
    /// Pixels refreshed then.
    pub pixels: u64,
    /// Bytes of storage its code read and wrote.
    pub read: u64,
    pub written: u64,
}

impl Default for PluginUsage {
    fn default() -> Self {
        Self {
            plugin: Id::new_static("core"),
            busy_us: 0,
            calls: 0,
            shown_ms: 0,
            refreshes: [0; 3],
            pixels: 0,
            read: 0,
            written: 0,
        }
    }
}

impl Usage {
    /// Counts a refresh of `area` in `mode`.
    pub fn refreshed(&mut self, mode: RefreshMode, area: Rect) {
        let index = match mode {
            RefreshMode::Fast => 0,
            RefreshMode::Partial => 1,
            RefreshMode::Full => 2,
        };
        self.refreshes[index] = self.refreshes[index].saturating_add(1);
        self.pixels = self
            .pixels
            .saturating_add(u64::from(area.width) * u64::from(area.height));
    }

    /// Refreshes of every mode together.
    pub fn total_refreshes(&self) -> u32 {
        self.refreshes
            .iter()
            .fold(0u32, |a, &b| a.saturating_add(b))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refreshes_counted_by_mode_and_area() {
        let mut usage = Usage::default();
        usage.refreshed(RefreshMode::Fast, Rect::new(0, 0, 10, 10));
        usage.refreshed(RefreshMode::Full, Rect::new(5, 5, 480, 800));
        usage.refreshed(RefreshMode::Full, Rect::new(0, 0, 1, 1));
        assert_eq!(usage.refreshes, [1, 0, 2]);
        assert_eq!(usage.pixels, 100 + 480 * 800 + 1);
        assert_eq!(usage.total_refreshes(), 3);
    }
}
