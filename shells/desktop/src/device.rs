//! The emulated device: kernel + panel + clock, independent of any window.

use std::time::Instant as StdInstant;

use quireink_core::{Effects, Kernel, KernelError, SpeechCall};
use quireink_graphics::Framebuffer;
use quireink_platform::{Battery, InputEvent, Instant, SystemStatus, WallClock};

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
    /// HTTP requests in flight.
    pub http: crate::http::Http,
    /// Plugins' servers.
    pub servers: quireink_serve::Servers,
    /// The text being spoken (emulated): its number and when it ends.
    speech: Option<(u32, Instant)>,
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
            http: crate::http::Http::new(),
            servers: quireink_serve::Servers::new(|| {}),
            speech: None,
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
        let status = self.status(now);
        let effects = budget::track(|| self.kernel.set_status(now, status));
        self.execute(effects);
        let effects = budget::track(|| self.kernel.start(now));
        self.execute(effects);
    }

    /// The emulated clock and battery: the computer's time and time zone
    /// (with simulated time a fixed one, so screenshots repeat) and a
    /// battery at 80 %.
    fn status(&self, now: Instant) -> SystemStatus {
        let clock = match self.clock {
            Clock::Real(_) => {
                let local = chrono::Local::now();
                WallClock {
                    at: now,
                    unix_millis: local.timestamp_millis(),
                    utc_offset_minutes: Some(local.offset().local_minus_utc() / 60),
                }
            }
            // 14 October 2026, 10:41 in Berlin.
            Clock::Simulated(_) => WallClock {
                at: now,
                unix_millis: 1_791_967_260_000,
                utc_offset_minutes: Some(120),
            },
        };
        SystemStatus {
            clock: Some(clock),
            battery: Some(Battery {
                percent: 80,
                charging: false,
            }),
        }
    }

    pub fn input(&mut self, event: InputEvent) {
        let now = self.now();
        let effects = budget::track(|| self.kernel.handle(now, event));
        self.execute(effects);
    }

    /// Delivers HTTP answers that have arrived; with `wait`, waits that
    /// long for the requests still in flight.
    pub fn poll_http(&mut self, wait: Option<std::time::Duration>) {
        while self.http.in_flight > 0 {
            let Some(response) = self.http.next(wait) else {
                break;
            };
            let now = self.now();
            let effects = budget::track(|| self.kernel.http_response(now, response));
            self.execute(effects);
        }
    }

    /// Passes requests to plugins' servers on.
    pub fn poll_servers(&mut self) {
        while let Some(event) = self.servers.next() {
            let now = self.now();
            let effects = budget::track(|| self.kernel.server_event(now, event));
            self.execute(effects);
        }
        let kernel = &self.kernel;
        self.servers
            .pump(|path, at, buffer| budget::track(|| kernel.read_served(path, at, buffer).ok()));
    }

    /// When something is due next: a wake-up the kernel asked for, the end
    /// of speech.
    pub fn next_wake(&self) -> Option<Instant> {
        let speech = self.speech.map(|(_, end)| end);
        match (self.wake_at, speech) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Ticks the kernel if a requested wake-up is due.
    pub fn tick_if_due(&mut self) {
        self.poll_http(None);
        self.poll_servers();
        let now = self.now();
        if let Some((id, end)) = self.speech
            && end <= now
        {
            self.speech = None;
            let effects = budget::track(|| self.kernel.speech_done(now, id, true));
            self.execute(effects);
        }
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
        for call in self.kernel.take_requests() {
            self.http.start(call);
        }
        for call in self.kernel.take_server_calls() {
            self.servers.apply(call);
        }
        for call in self.kernel.take_speech() {
            self.speak(call);
        }
        self.report_warnings();
    }

    /// Speech, emulated: the text is logged and takes as long as it would
    /// to speak, at about 160 words a minute.
    fn speak(&mut self, call: SpeechCall) {
        match call {
            SpeechCall::Say {
                id,
                text,
                language,
                rate,
            } => {
                let words = text.split_whitespace().count() as u64;
                let millis = words * 375 * 100 / u64::from(rate.max(1));
                let start: String = text.chars().take(60).collect();
                eprintln!("speech ({language}, {rate} %, {millis} ms): {start}");
                self.speech = Some((id, self.now().add_millis(millis)));
            }
            SpeechCall::Stop => {
                eprintln!("speech stopped");
                self.speech = None;
            }
        }
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
        let mut builder = Kernel::builder(profile.device)
            .storage(storage)
            .glyph_cache(profile.glyph_cache)
            .network(true)
            .speech(profile.speech);
        // Scalable fonts only where the emulated device renders them.
        if !profile.font_packs {
            builder = builder.font_dir("fonts");
        }
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
