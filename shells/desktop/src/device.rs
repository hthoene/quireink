//! The emulated device: kernel + panel + clock, independent of any window.

use std::time::Instant as StdInstant;

use quireink_core::{Effects, Kernel, KernelError};
use quireink_graphics::Framebuffer;
use quireink_platform::{InputEvent, Instant};

use crate::budget;
use crate::panel::Panel;
use crate::profiles::Profile;
use crate::storage::DirStorage;

/// A running device. All kernel calls go through here, so they are
/// counted against the profile's heap budget.
#[derive(Debug)]
pub struct Device {
    pub profile: &'static Profile,
    pub kernel: Kernel,
    /// The panel controller's frame memory: the kernel writes changed rows
    /// into it, possibly band by band. Not part of the heap budget, as on
    /// the device.
    controller: Framebuffer,
    pub panel: Panel,
    /// When the kernel wants to be ticked.
    pub wake_at: Option<Instant>,
    clock: Clock,
}

/// Either real time (interactive) or simulated time (headless, tests).
#[derive(Debug)]
enum Clock {
    Real(StdInstant),
    Simulated(u64),
}

impl Device {
    pub fn new(
        profile: &'static Profile,
        user_config: Option<String>,
        storage: DirStorage,
        simulated_time: bool,
    ) -> Result<Self, KernelError> {
        budget::set_limit(profile.heap_budget);
        let kernel = budget::track(|| {
            let mut builder = Kernel::builder(profile.device)
                .storage(storage)
                .glyph_cache(profile.glyph_cache)
                .plugin(quireink_plugin_format_txt::TxtFormat)
                .plugin(quireink_plugin_format_epub::EpubFormat)
                .plugin(quireink_plugin_reader::Reader)
                .plugin(quireink_plugin_files::Files)
                .plugin(quireink_plugin_home::Home)
                .plugin(quireink_plugin_menu::Menu)
                .plugin(quireink_plugin_settings::SettingsPlugin)
                .plugin(quireink_plugin_diagnostics::Diagnostics);
            if let Some(rows) = profile.frame_rows {
                builder = builder.frame_rows(rows);
            }
            builder = if profile.font_packs {
                builder.plugin(quireink_plugin_fonts_default::DefaultFontPacks)
            } else {
                builder.plugin(quireink_plugin_fonts_default::DefaultFonts)
            };
            for (id, value) in profile.settings {
                builder = builder.setting_default(id, value);
            }
            // Copy the configuration inside the tracked scope so that its
            // allocation and release are both accounted for.
            if let Some(config) = user_config.as_deref() {
                builder = builder.user_config(config);
            }
            builder.build()
        })?;
        let display = profile.device.display;
        let controller = Framebuffer::new(display.width, display.height, display.format)
            .map_err(KernelError::Graphics)?;
        let mut device = Self {
            profile,
            kernel,
            controller,
            panel: Panel::new(display.width, display.height, display.color),
            wake_at: None,
            clock: if simulated_time {
                Clock::Simulated(0)
            } else {
                Clock::Real(StdInstant::now())
            },
        };
        device.report_warnings();
        Ok(device)
    }

    pub fn now(&self) -> Instant {
        match self.clock {
            Clock::Real(start) => Instant(start.elapsed().as_millis() as u64),
            Clock::Simulated(ms) => Instant(ms),
        }
    }

    /// Advances simulated time; no-op for real time.
    pub fn advance(&mut self, millis: u64) {
        if let Clock::Simulated(ms) = &mut self.clock {
            *ms += millis;
        }
    }

    pub fn start(&mut self) {
        let now = self.now();
        let effects = budget::track(|| self.kernel.start(now));
        self.execute(effects);
    }

    pub fn input(&mut self, event: InputEvent) {
        let now = self.now();
        let effects = budget::track(|| self.kernel.handle(now, event));
        self.execute(effects);
    }

    /// Ticks the kernel if a requested wake-up is due.
    pub fn tick_if_due(&mut self) {
        let now = self.now();
        if self.wake_at.is_some_and(|at| at <= now) {
            let effects = budget::track(|| self.kernel.tick(now));
            self.execute(effects);
        }
    }

    /// Whether `name` is a button of this device.
    pub fn has_button(&self, name: &str) -> bool {
        self.profile.buttons.contains(&name)
    }

    fn execute(&mut self, effects: Effects) {
        if let Some((region, mode)) = effects.refresh() {
            let (kernel, controller) = (&mut self.kernel, &mut self.controller);
            budget::track(|| kernel.render(&mut |top, rows| controller.write_rows(top, rows)));
            self.panel.refresh(self.controller.view(), region, mode);
        }
        self.wake_at = effects.wake_at();
        self.report_warnings();
    }

    fn report_warnings(&mut self) {
        budget::track(|| {
            for warning in self.kernel.take_warnings() {
                eprintln!("warning: {warning}");
            }
        });
    }
}
