//! The emulated device: kernel + panel + clock, independent of any window.

use std::time::Instant as StdInstant;

use quireink_core::{Effects, Kernel, KernelError};
use quireink_graphics::Framebuffer;
use quireink_platform::{InputEvent, Instant};

use crate::budget;
use crate::panel::Panel;
use crate::profiles::Profile;
use quireink_fs::DirStorage;

/// A running device. All kernel calls go through here, so they are
/// counted against the profile's heap budget.
#[derive(Debug)]
pub struct Device {
    pub profile: &'static Profile,
    pub kernel: Kernel,
    /// What a restart needs to build the kernel again.
    user_config: Option<String>,
    root: std::path::PathBuf,
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
        let root = storage.root().to_path_buf();
        let kernel = build_kernel(profile, user_config.as_deref(), storage)?;
        let display = profile.device.display;
        let controller = Framebuffer::new(display.width, display.height, display.format)
            .map_err(KernelError::Graphics)?;
        let mut device = Self {
            profile,
            kernel,
            user_config,
            root,
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
        if effects.restart() {
            return self.restart();
        }
        if let Some((region, mode)) = effects.refresh() {
            let (kernel, controller) = (&mut self.kernel, &mut self.controller);
            budget::track(|| kernel.render(&mut |top, rows| controller.write_rows(top, rows)));
            self.panel.refresh(self.controller.view(), region, mode);
        }
        self.wake_at = effects.wake_at();
        self.report_warnings();
    }

    /// Builds a new kernel, as the device would after a reboot.
    fn restart(&mut self) {
        let storage = match DirStorage::new(&self.root) {
            Ok(storage) => storage,
            Err(e) => return eprintln!("restart: {}: {e}", self.root.display()),
        };
        // The old kernel goes first, as on the device: a minimal one (one
        // frame row, no plugins) holds its place meanwhile.
        let profile = self.profile;
        let kernel = &mut self.kernel;
        let swapped = budget::track(|| {
            let placeholder = Kernel::builder(profile.device).frame_rows(1).build()?;
            drop(std::mem::replace(kernel, placeholder));
            Ok::<(), KernelError>(())
        });
        if let Err(e) = swapped {
            return eprintln!("restart: {e}");
        }
        match build_kernel(self.profile, self.user_config.as_deref(), storage) {
            Ok(new) => {
                let kernel = &mut self.kernel;
                budget::track(|| drop(std::mem::replace(kernel, new)));
                self.report_warnings();
                self.start();
            }
            Err(e) => eprintln!("restart: {e}"),
        }
    }

    fn report_warnings(&mut self) {
        budget::track(|| {
            for warning in self.kernel.take_warnings() {
                eprintln!("warning: {warning}");
            }
        });
    }
}

/// The standard app for `profile`, counted against its heap budget.
fn build_kernel(
    profile: &'static Profile,
    user_config: Option<&str>,
    storage: DirStorage,
) -> Result<Kernel, KernelError> {
    budget::track(|| {
        let fonts = if profile.font_packs {
            quireink_app::Fonts::Packs
        } else {
            quireink_app::Fonts::Outline
        };
        let builder = Kernel::builder(profile.device)
            .storage(storage)
            .glyph_cache(profile.glyph_cache);
        let mut builder = quireink_app::install(builder, fonts);
        if let Some(rows) = profile.frame_rows {
            builder = builder.frame_rows(rows);
        }
        for (id, value) in profile.settings {
            builder = builder.setting_default(id, value);
        }
        if let Some(config) = user_config {
            builder = builder.user_config(config);
        }
        builder.build()
    })
}
