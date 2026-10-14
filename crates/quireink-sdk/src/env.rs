use core::fmt;

use quireink_i18n::Translator;
use quireink_platform::{DeviceInfo, Instant, Storage, SystemStatus};

use crate::fonts::Fonts;
use crate::formats::Formats;
use crate::hyphenation::Hyphenation;
use crate::menus::Menus;
use crate::plugins::Plugins;
use crate::settings::Settings;
use crate::status::StatusItems;

/// Read-only view of the environment a plugin runs in.
#[derive(Clone, Copy)]
pub struct Env<'a> {
    pub device: &'a DeviceInfo,
    /// Time of the event being handled.
    pub now: Instant,
    pub metrics: Metrics,
    pub settings: &'a Settings,
    pub i18n: &'a Translator,
    pub fonts: &'a Fonts,
    pub formats: &'a Formats,
    /// Hyphenation patterns by language.
    pub hyphenation: &'a Hyphenation,
    pub menus: &'a Menus,
    /// Every plugin with its state, for plugin managers.
    pub plugins: &'a Plugins,
    /// The time of day and the battery, as far as the device knows them
    /// (see [`time`](crate::time) for the local time).
    pub status: &'a SystemStatus,
    /// Items for status lines, from every plugin.
    pub status_items: &'a StatusItems,
    /// The device's storage. Plugins keep their own files below
    /// [`Env::plugin_dir`].
    pub storage: &'a dyn Storage,
}

impl fmt::Debug for Env<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Env")
            .field("device", &self.device.id)
            .field("now", &self.now)
            .finish_non_exhaustive()
    }
}

impl Env<'_> {
    /// The directory where plugin `plugin` keeps its data, e.g.
    /// `.quireink/plugins/reader`.
    pub fn plugin_dir(plugin: &str) -> alloc::string::String {
        alloc::format!(".quireink/plugins/{plugin}")
    }
}

/// Converts physical lengths to pixels for the current screen, so layouts
/// look the same on low- and high-density displays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Metrics {
    pub dpi: u16,
}

impl Metrics {
    pub const fn new(dpi: u16) -> Self {
        Self { dpi }
    }

    /// Pixels for `mm` millimetres (rounded, at least 1 for non-zero input).
    pub const fn mm(&self, mm: u32) -> u32 {
        self.tenth_mm(mm * 10)
    }

    /// Pixels for a font size in typographic points (1/72 inch).
    pub fn pt(&self, points: f32) -> f32 {
        points * f32::from(self.dpi) / 72.0
    }

    /// Pixels for `tenths` tenths of a millimetre.
    pub const fn tenth_mm(&self, tenths: u32) -> u32 {
        let px = (tenths * self.dpi as u32 + 127) / 254;
        if px == 0 && tenths > 0 { 1 } else { px }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_millimetres() {
        let metrics = Metrics::new(254);
        assert_eq!(metrics.mm(1), 10);
        assert_eq!(metrics.tenth_mm(5), 5);
        assert_eq!(Metrics::new(100).tenth_mm(1), 1);
        assert_eq!(Metrics::new(100).mm(0), 0);
    }
}
