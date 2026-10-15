//! The QuireInk kernel.
//!
//! The kernel hosts plugins and connects them: it keeps the registries of
//! contributions, resolves slots to screens, turns raw input into gestures
//! and actions through layered key maps, applies plugin commands and decides
//! how the e-ink panel is refreshed. It contains no product UI: even its own
//! contributions are registered as the `core` plugin.
//!
//! The kernel is *sans-I/O*: shells feed it [`InputEvent`]s and time, and
//! execute the returned [`Effects`]. It never blocks, spawns threads or
//! talks to an operating system, so it runs unchanged on microcontrollers,
//! Linux, Android and in the browser.
//!
//! Drawing is separate from event handling: after a call whose effects
//! contain a refresh, the shell calls [`Kernel::render`], which draws what
//! changed and hands it over as rows of pixels: the whole region at once,
//! or band by band on devices that cannot hold a full frame in RAM
//! ([`KernelBuilder::frame_rows`]). The panel controller keeps the frame.
//!
//! ```ignore
//! let mut kernel = Kernel::builder(device).plugin(MyPlugin).build()?;
//! let mut effects = kernel.start(clock.now());
//! loop {
//!     if let Some((region, mode)) = effects.refresh() {
//!         kernel.render(&mut |top, rows| panel.write(top, rows));
//!         panel.refresh(region, mode);
//!     }
//!     arm_timer(effects.wake_at());
//!     effects = match wait_for_input_or_timer() {
//!         Input(event) => kernel.handle(clock.now(), event),
//!         Timer => kernel.tick(clock.now()),
//!     };
//! }
//! ```

#![no_std]

extern crate alloc;

mod core_plugin;
mod data_plugin;
mod input;
mod keymap;
mod net;
mod refresh;
mod registry;
#[cfg(test)]
mod tests;

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;
use core::mem;

use quireink_graphics::{Framebuffer, GraphicsError};
use quireink_i18n::Translator;
use quireink_platform::{
    Color, DeviceInfo, FrameView, InputEvent, Instant, NoStorage, PointerEvent, PointerPhase, Rect,
    RefreshMode, Storage, SystemStatus,
};
use quireink_sdk::builtin::{
    CONFIG_FILE, GLOBAL_KEYMAP_LAYER, events, keymap_presets, params as builtin_params,
    settings as core_settings, slots,
};
use quireink_sdk::{
    Actions, Command, Cx, Env, EventHandler, Fonts, Formats, GestureKind, Handled, Hyphenation, Id,
    Manifest, Menus, Metrics, Output, Params, Plugin, PluginInfo, PluginState, Plugins,
    RegisterError, RequiredFor, SDK_VERSION, Screen, Settings, StatusItems,
};

use crate::core_plugin::{CORE, CorePlugin};
use crate::input::{Recognized, Recognizer, Thresholds};
use crate::keymap::{InputKey, Keymap, Origin, Zone};
pub use crate::net::{
    HttpCall, HttpResponse, MAX_FILE, MAX_RESPONSE, ServedFile, ServerCall, ServerEvent, SpeechCall,
};
use crate::refresh::RefreshPolicy;
use crate::registry::{PluginRegistrar, Registry, Staging};

pub use quireink_sdk as sdk;

/// Commands may trigger further commands; this bounds runaway loops.
const MAX_COMMAND_ROUNDS: usize = 16;
/// Default memory for glyphs rasterized at runtime.
const DEFAULT_GLYPH_CACHE: usize = 32 * 1024;

/// Something the shell has to do after an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Effect {
    /// Transfer `region` of [`Kernel::frame`] to the panel and refresh it.
    Refresh { region: Rect, mode: RefreshMode },
    /// Call [`Kernel::tick`] at (or soon after) this time.
    WakeAt(Instant),
    /// Build a new kernel, e.g. to apply switched plugins.
    Restart,
    /// Sleep once the frame is shown (the sleep screen): call
    /// [`Kernel::suspend`], and [`Kernel::resume`] on waking.
    Sleep,
}

/// The effects of one kernel call. Refreshes are already coalesced: there
/// is at most one per call.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Effects {
    refresh: Option<(Rect, RefreshMode)>,
    wake_at: Option<Instant>,
    restart: bool,
    sleep: bool,
}

impl Effects {
    pub fn refresh(&self) -> Option<(Rect, RefreshMode)> {
        self.refresh
    }

    pub fn wake_at(&self) -> Option<Instant> {
        self.wake_at
    }

    /// Whether the shell should build a new kernel (see [`Effect::Restart`]).
    pub fn restart(&self) -> bool {
        self.restart
    }

    /// Whether the shell should sleep once the frame is shown (see
    /// [`Effect::Sleep`]).
    pub fn sleep(&self) -> bool {
        self.sleep
    }

    pub fn is_empty(&self) -> bool {
        self.refresh.is_none() && self.wake_at.is_none() && !self.restart && !self.sleep
    }

    pub fn iter(&self) -> impl Iterator<Item = Effect> {
        let refresh = self
            .refresh
            .map(|(region, mode)| Effect::Refresh { region, mode });
        refresh
            .into_iter()
            .chain(self.wake_at.map(Effect::WakeAt))
            .chain(self.restart.then_some(Effect::Restart))
            .chain(self.sleep.then_some(Effect::Sleep))
    }
}

/// Where a warning comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum WarningSource {
    Plugin(Id),
    DeviceKeymap,
    UserConfig,
    Runtime,
}

/// A non-fatal problem. QuireInk keeps running with sensible defaults and
/// reports what it ignored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    pub source: WarningSource,
    pub line: Option<u32>,
    pub message: String,
}

impl fmt::Display for Warning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.source {
            WarningSource::Plugin(id) => write!(f, "plugin `{id}`")?,
            WarningSource::DeviceKeymap => f.write_str("device key map")?,
            WarningSource::UserConfig => f.write_str("user configuration")?,
            WarningSource::Runtime => f.write_str("runtime")?,
        }
        if let Some(line) = self.line {
            write!(f, ", line {line}")?;
        }
        write!(f, ": {}", self.message)
    }
}

/// Errors that prevent the kernel from starting.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum KernelError {
    Graphics(GraphicsError),
}

impl fmt::Display for KernelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KernelError::Graphics(error) => write!(f, "cannot create framebuffer: {error}"),
        }
    }
}

impl core::error::Error for KernelError {}

/// Configures and creates a [`Kernel`].
pub struct KernelBuilder {
    device: DeviceInfo,
    plugins: Vec<Box<dyn Plugin>>,
    /// Plugins shipped switched off, until the user switches them on.
    off: Vec<String>,
    device_keymap: Option<Cow<'static, str>>,
    user_config: Option<String>,
    glyph_cache: usize,
    network: bool,
    system_clipboard: bool,
    speech: bool,
    sleep: bool,
    frame_rows: Option<u16>,
    storage: Option<Box<dyn Storage>>,
    setting_defaults: Vec<(String, String)>,
    plugin_dirs: Vec<String>,
    font_dirs: Vec<String>,
}

impl fmt::Debug for KernelBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KernelBuilder")
            .field("device", &self.device.id)
            .field("plugins", &self.plugins.len())
            .finish_non_exhaustive()
    }
}

impl KernelBuilder {
    /// Adds a plugin. Plugins registered later win priority ties.
    pub fn plugin(mut self, plugin: impl Plugin + 'static) -> Self {
        self.plugins.push(Box::new(plugin));
        self
    }

    /// Adds a plugin that is off until the user switches it on (e.g. in
    /// the plugin manager), for features not everyone wants.
    pub fn plugin_off(mut self, plugin: impl Plugin + 'static) -> Self {
        self.off.push(String::from(plugin.manifest().id.as_str()));
        self.plugin(plugin)
    }

    /// Key bindings specific to the device (e.g. its physical buttons).
    pub fn device_keymap(mut self, source: impl Into<Cow<'static, str>>) -> Self {
        self.device_keymap = Some(source.into());
        self
    }

    /// The user's configuration file (`quireink.ini`). It overrides
    /// settings, slot bindings and key maps, and can disable plugins.
    pub fn user_config(mut self, source: impl Into<String>) -> Self {
        self.user_config = Some(source.into());
        self
    }

    /// The device's storage (books, settings, plugin data). Without it the
    /// kernel runs with an empty, read-only storage.
    pub fn storage(mut self, storage: impl Storage + 'static) -> Self {
        self.storage = Some(Box::new(storage));
        self
    }

    /// Changes the default of a setting for this device, e.g. the
    /// two-button key map preset on a device with only two buttons. Users
    /// can still override it.
    pub fn setting_default(mut self, id: &str, value: &str) -> Self {
        self.setting_defaults
            .push((String::from(id), String::from(value)));
        self
    }

    /// A folder on the storage with font files (OpenType/TrueType) to
    /// read with; they appear as one plugin, "font folder". Only for
    /// devices that render scalable fonts.
    pub fn font_dir(mut self, dir: impl Into<String>) -> Self {
        self.font_dirs.push(dir.into());
        self
    }

    /// A folder on the storage with data plugins (see `data_plugin`), one
    /// subfolder each. Without any, `plugins` is used.
    pub fn plugin_dir(mut self, dir: impl Into<String>) -> Self {
        self.plugin_dirs.push(dir.into());
        self
    }

    /// The shell makes HTTP requests (see [`Kernel::take_requests`]);
    /// without, plugins' requests fail at once.
    pub fn network(mut self, available: bool) -> Self {
        self.network = available;
        self
    }

    /// The shell reads the system's clipboard when plugins ask for its
    /// text (see [`Kernel::wants_clipboard`]); without, only text copied
    /// in QuireInk is pasted.
    pub fn system_clipboard(mut self, available: bool) -> Self {
        self.system_clipboard = available;
        self
    }

    /// The shell speaks text aloud (see [`Kernel::take_speech`]); without,
    /// plugins' speech fails at once.
    pub fn speech(mut self, available: bool) -> Self {
        self.speech = available;
        self
    }

    /// The shell sleeps after showing QuireInk's sleep screen (see
    /// [`Effect::Sleep`]); without, the system has its own (Android).
    pub fn sleep(mut self, available: bool) -> Self {
        self.sleep = available;
        self
    }

    /// Memory for glyphs rasterized at runtime (default 32 KiB). Bitmap font
    /// packs do not use it.
    pub fn glyph_cache(mut self, bytes: usize) -> Self {
        self.glyph_cache = bytes;
        self
    }

    /// Draws frames in bands of `rows` rows instead of holding the whole
    /// screen in RAM: the framebuffer shrinks to `rows / height` of its
    /// size, while each band costs one more pass over the visible screens.
    /// For devices whose panel controller keeps the frame (e.g. the X4).
    pub fn frame_rows(mut self, rows: u16) -> Self {
        self.frame_rows = Some(rows);
        self
    }

    pub fn build(self) -> Result<Kernel, KernelError> {
        let display = self.device.display;
        let rows = self.frame_rows.unwrap_or(display.height);
        let framebuffer = Framebuffer::banded(display.width, display.height, rows, display.format)
            .map_err(KernelError::Graphics)?;
        let mut kernel = Kernel {
            shared: Shared {
                device: self.device,
                settings: Settings::new(),
                translator: Translator::new(),
                fonts: Fonts::new(self.glyph_cache),
                formats: Formats::new(),
                hyphenation: Hyphenation::new(),
                menus: Menus::new(),
                actions: Actions::new(),
                plugins: Plugins::new(),
                status: SystemStatus::default(),
                status_items: StatusItems::new(),
                widgets: quireink_sdk::widgets::Widgets::new(),
                text_filters: quireink_sdk::filters::TextFilters::new(),
                network: self.network,
                clipboard: String::new(),
                system_clipboard: false,
                speech: self.speech,
                sleep: self.sleep,
                slots: Vec::new(),
                font_dirs: Vec::new(),
                secrets: Default::default(),
                storage: self.storage.unwrap_or_else(|| Box::new(NoStorage)),
                keymap: Keymap::default(),
            },
            pending: None,
            unrendered: None,
            framebuffer,
            registry: Registry::default(),
            keymap_sources: Vec::new(),
            keymap_presets: Vec::new(),
            subscriptions: Vec::new(),
            screen_timer: None,
            notice: None,
            scheduled: Vec::new(),
            recognizer: Recognizer::default(),
            captured: Vec::new(),
            stack: Vec::new(),
            refresh: RefreshPolicy::default(),
            output: Output::default(),
            warnings: Vec::new(),
            restart: false,
            sleep: false,
            screenshot: None,
            last_activity: Instant(0),
            scope: None,
            requests: Vec::new(),
            copied: None,
            clipboard_reads: Vec::new(),
            server_calls: Vec::new(),
            servers: Vec::new(),
            next_server: 0,
            request_bodies: Vec::new(),
            incoming: Vec::new(),
            reads_clipboard: self.system_clipboard,
            speech_calls: Vec::new(),
            speaking: Vec::new(),
            next_speech: 0,
            awaiting: Vec::new(),
            next_request: 0,
            plugin_defaults: Vec::new(),
            now: Instant::default(),
        };
        let (touch, speech) = (kernel.shared.device.input.touch, kernel.shared.speech);
        kernel.shared.menus.set_device(touch, speech);
        kernel.shared.menus.set_sleep(kernel.shared.sleep);

        // Passwords and keys.
        if let Ok(data) = quireink_sdk::read_file(
            kernel.shared.storage.as_ref(),
            quireink_sdk::secrets::FILE,
            64 * 1024,
        ) {
            let text = String::from_utf8_lossy(&data);
            kernel.shared.secrets = quireink_sdk::secrets::Secrets::decode(&text);
        }

        // The configuration file on the device, then the shell's overrides.
        let stored = read_config(kernel.shared.storage.as_ref());
        let configs: Vec<&str> = [stored.as_deref(), self.user_config.as_deref()]
            .into_iter()
            .flatten()
            .collect();
        let mut disabled: Vec<String> = configs
            .iter()
            .flat_map(|c| plugin_switches(c, false))
            .collect();
        // Shipped off, unless switched on.
        let on: Vec<String> = configs
            .iter()
            .flat_map(|c| plugin_switches(c, true))
            .collect();
        disabled.extend(self.off.into_iter().filter(|id| !on.contains(id)));
        let core: Box<dyn Plugin> = Box::new(CorePlugin);
        for plugin in core::iter::once(core).chain(self.plugins) {
            kernel.add_plugin(plugin.as_ref(), &disabled);
        }
        // Data plugins from the storage, after the built-in ones.
        let dirs = if self.plugin_dirs.is_empty() {
            alloc::vec![String::from(quireink_sdk::builtin::PLUGIN_DIR)]
        } else {
            self.plugin_dirs
        };
        for dir in &dirs {
            let mut problems = Vec::new();
            let found =
                data_plugin::discover(kernel.shared.storage.as_ref(), dir, &mut |problem| {
                    problems.push(problem);
                });
            for problem in problems {
                kernel.warn(WarningSource::Runtime, None, problem);
            }
            for plugin in found {
                kernel.add_plugin(&plugin, &disabled);
            }
        }
        if !self.font_dirs.is_empty()
            && let Some(plugin) =
                data_plugin::font_folder(kernel.shared.storage.as_ref(), &self.font_dirs)
        {
            kernel.add_plugin(&plugin, &disabled);
        }
        kernel.shared.font_dirs = self.font_dirs;
        kernel.find_required_plugins();

        if let Some(source) = self.device_keymap {
            for problem in kernel.shared.keymap.load(&source, Origin::Device, true) {
                kernel.warn(
                    WarningSource::DeviceKeymap,
                    Some(problem.line),
                    problem.message.into_owned(),
                );
            }
            kernel.keymap_sources.push((Origin::Device, source));
        }
        let mut presets: Vec<Cow<'static, str>> =
            alloc::vec![Cow::Borrowed(keymap_presets::STANDARD_NAME)];
        presets.extend(
            kernel
                .keymap_presets
                .iter()
                .map(|(id, _)| Cow::Owned(id.as_str().to_string())),
        );
        kernel
            .shared
            .settings
            .set_choices(core_settings::KEYMAP_PRESET.as_str(), presets);
        let mut families: Vec<Cow<'static, str>> =
            alloc::vec![Cow::Borrowed(core_settings::DEFAULT_FONT)];
        families.extend(
            kernel
                .shared
                .fonts
                .families()
                .into_iter()
                .map(|f| Cow::Owned(f.to_string())),
        );
        kernel
            .shared
            .settings
            .set_choices(core_settings::READING_FONT.as_str(), families);
        // Defaults from plugins (e.g. themes), then the device's.
        for (plugin, id, value) in mem::take(&mut kernel.plugin_defaults) {
            if let Err(error) = kernel
                .shared
                .settings
                .set_default_from_text(id.as_str(), &value)
            {
                kernel.warn(
                    WarningSource::Plugin(plugin),
                    None,
                    format!("default for `{id}`: {error}"),
                );
            }
        }
        for (id, value) in &self.setting_defaults {
            if let Err(error) = kernel.shared.settings.set_default_from_text(id, value) {
                kernel.warn(
                    WarningSource::Runtime,
                    None,
                    format!("device default `{id}`: {error}"),
                );
            }
        }
        for source in &configs {
            kernel.apply_user_config(source);
        }
        kernel.set_menu_titles();
        kernel.apply_reading_font();
        kernel.validate();
        kernel.rebuild_keymap();
        let shared = &mut kernel.shared;
        shared
            .translator
            .set_language(shared.settings.text(core_settings::LANGUAGE.as_str()));
        kernel.framebuffer.set_inverted(
            kernel
                .shared
                .settings
                .bool(core_settings::NIGHT_MODE.as_str()),
        );
        kernel.shrink_to_fit();
        kernel.shared.slots = kernel.registry.bound_slots();
        Ok(kernel)
    }
}

/// A screen on the navigation stack.
/// A request to a plugin's server whose body is still coming.
struct Incoming {
    request: u32,
    server: u32,
    method: String,
    target: String,
    /// Where the body goes.
    path: String,
}

struct ActiveScreen {
    id: Id,
    /// The slot it was opened for, if any.
    slot: Option<Id>,
    /// What it was opened with, e.g. the book.
    params: Params,
    screen: Box<dyn Screen>,
}

/// The QuireInk kernel. See the [crate documentation](crate).
pub struct Kernel {
    shared: Shared,
    framebuffer: Framebuffer,
    registry: Registry,
    /// Every key map source, to rebuild the key map when the preset changes.
    keymap_sources: Vec<(Origin, Cow<'static, str>)>,
    keymap_presets: Vec<(Id, Cow<'static, str>)>,
    subscriptions: Vec<(Id, EventHandler)>,
    /// When the active screen wants `on_timer`.
    screen_timer: Option<Instant>,
    /// Events plugins asked for at a time ([`Command::Schedule`]).
    scheduled: Vec<(Instant, Id, Params)>,
    /// The short message showing ([`Command::Notify`]) and until when.
    notice: Option<(Box<dyn Screen>, Instant)>,
    /// The part of the screen to redraw at the end of the current call.
    pending: Option<Rect>,
    /// What changed but has not been passed to [`Kernel::render`] yet.
    unrendered: Option<Rect>,
    recognizer: Recognizer,
    /// Pointer ids captured by the active screen.
    captured: Vec<u8>,
    stack: Vec<ActiveScreen>,
    refresh: RefreshPolicy,
    output: Output,
    warnings: Vec<Warning>,
    /// A restart was requested (see [`Command::Restart`]).
    restart: bool,
    /// Sleep was requested (see [`Command::Sleep`]).
    sleep: bool,
    /// A screenshot to save with the next frame drawn: its path.
    screenshot: Option<String>,
    /// When the device was last used (input, a page turned), for sleeping
    /// by itself ([`settings::SLEEP_AFTER`](core_settings::SLEEP_AFTER)).
    last_activity: Instant,
    /// The active settings scope: its file and the screen that set it.
    scope: Option<(String, Option<usize>)>,
    /// HTTP requests for the shell to make.
    requests: Vec<HttpCall>,
    /// Text copied, for the shell to put on the system's clipboard.
    copied: Option<String>,
    /// What the shell is to do for plugins' servers.
    server_calls: Vec<ServerCall>,
    /// Servers running: number for the shell, id, event, and how many
    /// screens were open when it started (it stops once fewer are).
    servers: Vec<(u32, Id, Id, usize)>,
    next_server: u32,
    /// Bodies of requests not answered yet: request, file.
    request_bodies: Vec<(u32, String)>,
    /// Requests whose body is still coming.
    incoming: Vec<Incoming>,
    /// Requests for the clipboard's text, waiting for the shell: reply
    /// event and parameters.
    clipboard_reads: Vec<(Id, Params)>,
    /// The shell reads the system's clipboard (see
    /// [`KernelBuilder::system_clipboard`]).
    reads_clipboard: bool,
    /// What the shell is to do for speech.
    speech_calls: Vec<SpeechCall>,
    /// The texts being spoken, in order: number for the shell, reply event
    /// and parameters, and how many screens were open when it started
    /// (speech stops once fewer are).
    speaking: Vec<(u32, Id, Params, usize)>,
    next_speech: u32,
    /// Requests made and not answered yet: id, reply event, parameters
    /// and where to save the body.
    awaiting: Vec<(u32, Id, Params, Option<String>)>,
    next_request: u32,
    /// Setting defaults from plugins, applied once all are registered:
    /// `(plugin, setting, value)`.
    plugin_defaults: Vec<(Id, Id, Cow<'static, str>)>,
    now: Instant,
}

impl fmt::Debug for Kernel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Kernel")
            .field("device", &self.shared.device.id)
            .field("plugins", &self.shared.plugins.len())
            .field(
                "stack",
                &self.stack.iter().map(|s| &s.id).collect::<Vec<_>>(),
            )
            .finish_non_exhaustive()
    }
}

impl Kernel {
    pub fn builder(device: DeviceInfo) -> KernelBuilder {
        KernelBuilder {
            device,
            plugins: Vec::new(),
            off: Vec::new(),
            device_keymap: None,
            user_config: None,
            glyph_cache: DEFAULT_GLYPH_CACHE,
            network: false,
            system_clipboard: false,
            speech: false,
            sleep: false,
            frame_rows: None,
            storage: None,
            setting_defaults: Vec::new(),
            plugin_dirs: Vec::new(),
            font_dirs: Vec::new(),
        }
    }

    /// Opens the home slot and emits [`events::STARTED`]. Call once after
    /// building.
    pub fn start(&mut self, now: Instant) -> Effects {
        self.now = now;
        self.last_activity = now;
        self.output.commands.push(Command::open_slot(slots::HOME));
        self.output
            .commands
            .push(Command::Emit(events::STARTED, Params::new()));
        self.finish()
    }

    /// Handles an input event.
    pub fn handle(&mut self, now: Instant, event: InputEvent) -> Effects {
        self.now = now;
        self.last_activity = now;
        match event {
            InputEvent::Button { name, state } => {
                if let Some(recognized) = self.recognizer.button(name, state, now) {
                    self.recognized(recognized);
                }
            }
            InputEvent::Pointer(pointer) => self.pointer(pointer),
            _ => {}
        }
        self.finish()
    }

    /// Tells the kernel the time of day and the battery's charge. Shells
    /// call it at start, when the clock is set or its time zone changes,
    /// and when the battery changes (checking about once a minute is
    /// plenty). Status lines redraw when what they show changed
    /// ([`events::STATUS_CHANGED`]).
    pub fn set_status(&mut self, now: Instant, status: SystemStatus) -> Effects {
        self.now = now;
        let old = self.shared.status;
        self.shared.status = status;
        let offset = |s: &SystemStatus| s.clock.map(|c| c.utc_offset_minutes);
        // A new wall clock reading alone changes nothing visible.
        let jumped = match (old.clock, status.clock) {
            (Some(a), Some(b)) => (a.unix_millis_at(now) - b.unix_millis_at(now)).abs() > 30_000,
            (a, b) => a.is_some() != b.is_some(),
        };
        if old.battery != status.battery || offset(&old) != offset(&status) || jumped {
            self.output
                .commands
                .push(Command::Emit(events::STATUS_CHANGED, Params::new()));
        }
        self.finish()
    }

    /// When the device is to go to sleep by itself, where it does.
    fn sleep_due(&self) -> Option<Instant> {
        let minutes = self
            .shared
            .settings
            .int(core_settings::SLEEP_AFTER.as_str());
        (self.shared.sleep && minutes > 0)
            .then(|| self.last_activity.add_millis(minutes as u64 * 60_000))
    }

    /// Advances time, e.g. when a requested [`Effect::WakeAt`] is due.
    pub fn tick(&mut self, now: Instant) -> Effects {
        self.now = now;
        if self.sleep_due().is_some_and(|at| at <= now) {
            self.last_activity = now;
            // Busy: reading aloud, downloading, serving. Else asleep.
            if self.speaking.is_empty() && self.awaiting.is_empty() && self.servers.is_empty() {
                self.output
                    .commands
                    .push(Command::RunAction(quireink_sdk::builtin::actions::SLEEP));
            }
        }
        let thresholds = self.thresholds();
        for recognized in self.recognizer.tick(now, &thresholds) {
            self.recognized(recognized);
        }
        if self.screen_timer.is_some_and(|at| at <= now) {
            self.screen_timer = None;
            self.with_top(|screen, cx| {
                screen.on_timer(cx);
                Handled::Yes
            });
        }
        if self.notice.as_ref().is_some_and(|(_, until)| *until <= now) {
            self.end_notice();
        }
        while let Some(due) = self.scheduled.iter().position(|(at, ..)| *at <= now) {
            let (_, event, params) = self.scheduled.remove(due);
            self.output.commands.push(Command::Emit(event, params));
        }
        self.finish()
    }

    /// Releases memory reserved while plugins registered: on small devices
    /// every kilobyte counts, and registries are not modified afterwards.
    fn shrink_to_fit(&mut self) {
        self.registry.shrink_to_fit();
        self.keymap_sources.shrink_to_fit();
        self.keymap_presets.shrink_to_fit();
        self.subscriptions.shrink_to_fit();
        self.shared.plugins.shrink_to_fit();
        self.warnings.shrink_to_fit();
        let shared = &mut self.shared;
        shared.settings.shrink_to_fit();
        shared.menus.shrink_to_fit();
        shared.actions.shrink_to_fit();
        shared.fonts.shrink_to_fit();
        shared.formats.shrink_to_fit();
        shared.hyphenation.shrink_to_fit();
        shared.status_items.shrink_to_fit();
        shared.widgets.shrink_to_fit();
        shared.text_filters.shrink_to_fit();
        shared.translator.shrink_to_fit();
    }

    /// Draws everything that changed since the last call and passes it to
    /// `out` as full-width rows of pixels, top to bottom: `out(top, rows)`
    /// with `top` the screen row of the first one. Call it after each
    /// [`start`](Kernel::start), [`handle`](Kernel::handle) or
    /// [`tick`](Kernel::tick) whose effects contain a refresh, then refresh
    /// the panel.
    pub fn render(&mut self, out: &mut dyn FnMut(u16, FrameView<'_>)) {
        let Some(region) = self.unrendered.take() else {
            return;
        };
        // A screenshot is written as the frame is drawn, a band at a time.
        let bounds = self.framebuffer.bounds();
        let shot = self.screenshot.take().filter(|_| region == bounds);
        let mut writer = shot.as_deref().and_then(|path| {
            quireink_sdk::image::PngWriter::new(
                self.shared.storage.as_ref(),
                path,
                u32::from(bounds.width),
                u32::from(bounds.height),
            )
            .ok()
        });
        let mut gray = Vec::new();
        let rows = self.framebuffer.rows();
        let mut top = region.y - region.y % rows;
        while u32::from(top) < region.bottom() {
            self.framebuffer.set_top(top);
            if let Some(part) = region.intersection(&self.framebuffer.band()) {
                self.redraw(part);
                let view = self.framebuffer.view_rows(part);
                if let (Some(writer), Some(path)) = (writer.as_mut(), shot.as_deref()) {
                    for y in 0..view.height {
                        gray.clear();
                        gray.extend((0..view.width).map(|x| {
                            let c = view.pixel(x, y);
                            ((u32::from(c.r) * 77 + u32::from(c.g) * 150 + u32::from(c.b) * 29)
                                >> 8) as u8
                        }));
                        if writer
                            .row(self.shared.storage.as_ref(), path, &gray)
                            .is_err()
                        {
                            break;
                        }
                    }
                }
                out(part.y, view);
            }
            top = top.saturating_add(rows);
            if top == u16::MAX {
                break;
            }
        }
        if let Some(path) = shot {
            let key = if writer.is_some_and(|w| w.is_complete()) {
                "core.screenshot-saved"
            } else {
                "core.screenshot-failed"
            };
            let text = self
                .shared
                .translator
                .format(key, &[("path", quireink_sdk::Arg::Str(&path))]);
            self.output.commands.push(Command::Notify(text));
        }
    }

    /// The framebuffer, for tests of a kernel that holds the whole screen.
    #[cfg(test)]
    pub(crate) fn frame(&self) -> FrameView<'_> {
        self.framebuffer.view()
    }

    pub fn device(&self) -> &DeviceInfo {
        &self.shared.device
    }

    pub fn settings(&self) -> &Settings {
        &self.shared.settings
    }

    pub fn translator(&self) -> &Translator {
        &self.shared.translator
    }

    pub fn fonts(&self) -> &Fonts {
        &self.shared.fonts
    }

    pub fn formats(&self) -> &Formats {
        &self.shared.formats
    }

    /// Every plugin with its state, `core` first.
    pub fn plugins(&self) -> &Plugins {
        &self.shared.plugins
    }

    /// The id of the screen currently shown.
    pub fn active_screen(&self) -> Option<&Id> {
        self.stack.last().map(|s| &s.id)
    }

    /// Returns and clears the collected warnings.
    pub fn take_warnings(&mut self) -> Vec<Warning> {
        mem::take(&mut self.warnings)
    }

    /// Text copied since the last call, for the system's clipboard (see
    /// [`Command::SetClipboard`]). Check after every kernel call.
    pub fn take_clipboard(&mut self) -> Option<String> {
        self.copied.take()
    }

    /// Tells the kernel whether the system's clipboard has text, e.g. when
    /// the app comes back from another one (without reading it, which some
    /// systems announce to the user).
    pub fn set_clipboard_available(&mut self, available: bool) {
        self.shared.system_clipboard = available;
    }

    /// Whether plugins wait for the system clipboard's text: read it and
    /// pass it to [`Kernel::clipboard_text`]. Check after every kernel
    /// call.
    pub fn wants_clipboard(&self) -> bool {
        !self.clipboard_reads.is_empty()
    }

    /// The system clipboard's text, for the plugins that asked for it;
    /// `None` if it could not be read (the text copied here is used).
    pub fn clipboard_text(&mut self, now: Instant, text: Option<String>) -> Effects {
        self.now = now;
        if let Some(text) = text {
            self.shared.clipboard = clipped(&text);
        }
        for (reply, params) in mem::take(&mut self.clipboard_reads) {
            self.reply_clipboard(reply, params);
        }
        self.finish()
    }

    /// What to do for speech: say a text, stop. Check after every kernel
    /// call; report the end of each text with [`Kernel::speech_done`].
    pub fn take_speech(&mut self) -> Vec<SpeechCall> {
        mem::take(&mut self.speech_calls)
    }

    /// Whether a text is being spoken, e.g. to keep watching for its end.
    pub fn speaking(&self) -> bool {
        !self.speaking.is_empty()
    }

    /// Text `id` of a [`SpeechCall::Say`] was spoken (`spoken`), or could
    /// not be.
    pub fn speech_done(&mut self, now: Instant, id: u32, spoken: bool) -> Effects {
        self.now = now;
        if let Some(at) = self.speaking.iter().position(|(n, ..)| *n == id) {
            let (_, reply, params, _) = self.speaking.remove(at);
            self.reply_speech(reply, params, spoken);
        }
        self.finish()
    }

    /// Emits the reply to a [`Command::Speak`].
    fn reply_speech(&mut self, reply: Id, params: Params, spoken: bool) {
        let params = if spoken {
            params
        } else {
            params.with(quireink_sdk::speech_params::FAILED, "1")
        };
        self.output.commands.push(Command::Emit(reply, params));
    }

    /// Starts speaking for a plugin, or tells it that it cannot.
    fn speak(&mut self, speech: quireink_sdk::Speech) {
        if !self.shared.speech {
            return self.reply_speech(speech.reply, speech.params, false);
        }
        let id = self.next_speech;
        self.next_speech = self.next_speech.wrapping_add(1);
        if !speech.queue {
            self.speaking.clear();
        }
        self.speaking
            .push((id, speech.reply, speech.params, self.stack.len()));
        self.speech_calls.push(SpeechCall::Say {
            id,
            text: speech.text,
            language: speech.language,
            rate: speech.rate,
            queue: speech.queue,
        });
    }

    /// Stops what closed screens started: servers, speech.
    fn stop_for_closed_screens(&mut self) {
        self.stop_servers_of_closed_screens();
        if self
            .speaking
            .iter()
            .any(|(.., screens)| *screens > self.stack.len())
        {
            self.speaking.clear();
            self.speech_calls.push(SpeechCall::Stop);
        }
    }

    /// Stops server `at` and tells its plugin.
    fn stop_server(&mut self, at: usize) {
        let (number, id, events, _) = self.servers.remove(at);
        self.server_calls.push(ServerCall::Stop { server: number });
        let params = Params::new()
            .with(quireink_sdk::builtin::params::ID, id.as_str().to_string())
            .with(
                quireink_sdk::server_params::KIND,
                quireink_sdk::server_params::STOPPED,
            );
        self.output.commands.push(Command::Emit(events, params));
    }

    /// Stops the servers of screens that were closed.
    fn stop_servers_of_closed_screens(&mut self) {
        while let Some(at) = self
            .servers
            .iter()
            .position(|(.., screens)| *screens > self.stack.len())
        {
            self.stop_server(at);
        }
    }

    /// Starts serving for a plugin, or tells it why not.
    fn serve(&mut self, server: Id, port: u16, events: Id) {
        if let Some(at) = self.servers.iter().position(|(_, id, ..)| *id == server) {
            let (number, ..) = self.servers.remove(at);
            self.server_calls.push(ServerCall::Stop { server: number });
        }
        if !self.shared.network {
            let params = Params::new()
                .with(
                    quireink_sdk::builtin::params::ID,
                    server.as_str().to_string(),
                )
                .with(
                    quireink_sdk::server_params::KIND,
                    quireink_sdk::server_params::FAILED,
                )
                .with(quireink_sdk::server_params::REASON, "no network");
            self.output.commands.push(Command::Emit(events, params));
            return;
        }
        self.next_server = self.next_server.wrapping_add(1);
        let number = self.next_server;
        self.servers
            .push((number, server, events, self.stack.len()));
        self.server_calls.push(ServerCall::Listen {
            server: number,
            port,
        });
    }

    /// Passes a plugin's answer to the shell; a file to send goes in
    /// parts (see [`Kernel::read_served`]).
    fn respond(&mut self, response: quireink_sdk::ServerResponse) {
        let mut status = response.status;
        let mut body = response.body;
        let mut file = None;
        if let Some(path) = response.body_from {
            match self.shared.storage.size(&path) {
                Ok(size) => file = Some(ServedFile { path, size }),
                Err(_) => {
                    status = 404;
                    body = b"not found".to_vec();
                }
            }
        }
        // The request's body is done with, unless the plugin moved it.
        if let Some(at) = self
            .request_bodies
            .iter()
            .position(|(r, _)| *r == response.request)
        {
            let (_, path) = self.request_bodies.remove(at);
            let _ = self.shared.storage.remove(&path);
        }
        self.server_calls.push(ServerCall::Respond {
            request: response.request,
            status,
            content_type: response.content_type,
            headers: response.headers,
            body,
            file,
        });
    }

    /// Reads part of a file being sent as an answer
    /// ([`ServerCall::Respond`] with a `file`), from `at` into `buffer`;
    /// returns how much was read, 0 at the end.
    pub fn read_served(
        &self,
        path: &str,
        at: u64,
        buffer: &mut [u8],
    ) -> Result<usize, quireink_platform::StorageError> {
        self.shared.storage.read_at(path, at, buffer)
    }

    /// Tells the plugin of server `server` about a complete request.
    fn pass_request(
        &mut self,
        server: u32,
        request: u32,
        method: String,
        target: String,
        body: Option<String>,
    ) {
        use quireink_sdk::server_params as p;
        let Some((_, id, events, _)) = self.servers.iter().find(|(n, ..)| *n == server) else {
            if let Some(path) = body {
                self.forget_body(request, &path);
            }
            return;
        };
        let mut params = Params::new()
            .with(quireink_sdk::builtin::params::ID, id.as_str().to_string())
            .with(p::KIND, p::REQUEST)
            .with(p::REQUEST, format!("{request}"))
            .with(p::METHOD, method)
            .with(p::TARGET, target);
        if let Some(path) = body {
            params = params.with(p::BODY_FILE, path);
        }
        self.output
            .commands
            .push(Command::Emit(events.clone(), params));
    }

    /// Removes the body of `request`, e.g. after the connection was lost.
    fn forget_body(&mut self, request: u32, path: &str) {
        self.request_bodies.retain(|(r, _)| *r != request);
        let _ = self.shared.storage.remove(path);
    }

    /// Answers a request without its plugin, e.g. when its body could not
    /// be saved.
    fn refuse(&mut self, request: u32, status: u16, reason: &str) {
        self.server_calls.push(ServerCall::Respond {
            request,
            status,
            content_type: String::from("text/plain"),
            headers: Vec::new(),
            body: reason.as_bytes().to_vec(),
            file: None,
        });
    }

    /// Answers a [`Command::ReadClipboard`] with the text known here.
    fn reply_clipboard(&mut self, reply: Id, params: Params) {
        let text = self.shared.clipboard.clone();
        self.output.commands.push(Command::Emit(
            reply,
            params.with(quireink_sdk::builtin::params::TEXT, text),
        ));
    }

    /// What to do for plugins' servers: listen, stop, answer requests
    /// (report with [`Kernel::server_event`]). Check after every kernel
    /// call.
    pub fn take_server_calls(&mut self) -> Vec<ServerCall> {
        mem::take(&mut self.server_calls)
    }

    /// Whether plugins serve, e.g. to keep polling for requests.
    pub fn serving(&self) -> bool {
        !self.servers.is_empty()
    }

    /// What happened with a server: started, failed, a request.
    pub fn server_event(&mut self, now: Instant, event: ServerEvent) -> Effects {
        use quireink_sdk::server_params as p;
        self.now = now;
        let events_of = |servers: &[(u32, Id, Id, usize)], number: u32| {
            servers
                .iter()
                .find(|(n, ..)| *n == number)
                .map(|(_, server, events, _)| (server.clone(), events.clone()))
        };
        match event {
            ServerEvent::Started { server, address } => {
                if let Some((id, events)) = events_of(&self.servers, server) {
                    let params = Params::new()
                        .with(quireink_sdk::builtin::params::ID, id.as_str().to_string())
                        .with(p::KIND, p::STARTED)
                        .with(p::ADDRESS, address);
                    self.output.commands.push(Command::Emit(events, params));
                }
            }
            ServerEvent::Failed { server, reason } => {
                if let Some((id, events)) = events_of(&self.servers, server) {
                    self.servers.retain(|(n, ..)| *n != server);
                    let params = Params::new()
                        .with(quireink_sdk::builtin::params::ID, id.as_str().to_string())
                        .with(p::KIND, p::FAILED)
                        .with(p::REASON, reason);
                    self.output.commands.push(Command::Emit(events, params));
                }
            }
            ServerEvent::Request {
                server,
                request,
                method,
                target,
                body: false,
            } => self.pass_request(server, request, method, target, None),
            ServerEvent::Request {
                server,
                request,
                method,
                target,
                body: true,
            } => {
                // The body is saved as it comes, then the plugin is told.
                let path = format!(".quireink/tmp/request-{request}");
                let _ = self.shared.storage.create_dir(".quireink/tmp");
                if self.shared.storage.write(&path, &[]).is_ok() {
                    self.request_bodies.push((request, path.clone()));
                    self.incoming.push(Incoming {
                        request,
                        server,
                        method,
                        target,
                        path,
                    });
                } else {
                    self.refuse(request, 507, "cannot save the body");
                }
            }
            ServerEvent::Body {
                request,
                data,
                last,
            } => {
                let Some(at) = self.incoming.iter().position(|i| i.request == request) else {
                    return self.finish();
                };
                if self
                    .shared
                    .storage
                    .append(&self.incoming[at].path, &data)
                    .is_err()
                {
                    let incoming = self.incoming.remove(at);
                    self.forget_body(request, &incoming.path);
                    self.refuse(request, 507, "the storage is full");
                } else if last {
                    let i = self.incoming.remove(at);
                    self.pass_request(i.server, request, i.method, i.target, Some(i.path));
                }
            }
            ServerEvent::Closed { request } => {
                self.incoming.retain(|i| i.request != request);
                let body = self.request_bodies.iter().find(|(r, _)| *r == request);
                if let Some(path) = body.map(|(_, path)| path.clone()) {
                    self.forget_body(request, &path);
                }
            }
        }
        self.finish()
    }

    /// Returns the HTTP requests to make; answer each with
    /// [`Kernel::http_response`]. Check after every kernel call.
    pub fn take_requests(&mut self) -> Vec<HttpCall> {
        mem::take(&mut self.requests)
    }

    /// Delivers the answer to a request from [`Kernel::take_requests`].
    pub fn http_response(&mut self, now: Instant, response: HttpResponse) -> Effects {
        self.now = now;
        if let Some(at) = self.awaiting.iter().position(|(id, ..)| *id == response.id) {
            let (_, reply, params, save_to) = self.awaiting.remove(at);
            let mut status = format!("{}", response.status);
            let mut params = match save_to {
                // Answers that are files: written where asked (a failure
                // has its reason as the body).
                Some(path) if response.status != 0 => {
                    if self.shared.storage.write(&path, &response.body).is_err() {
                        status = String::from("0");
                    }
                    params
                }
                _ => {
                    let body = String::from_utf8_lossy(&response.body).into_owned();
                    params.with(quireink_sdk::http_params::BODY, body)
                }
            };
            params = params.with(quireink_sdk::http_params::STATUS, status);
            if let Some(etag) = response.etag {
                params = params.with(quireink_sdk::http_params::ETAG, etag);
            }
            if let Some(content_type) = response.content_type {
                params = params.with(quireink_sdk::http_params::CONTENT_TYPE, content_type);
            }
            if let Some(url) = response.url {
                params = params.with(quireink_sdk::http_params::FINAL_URL, url);
            }
            self.output.commands.push(Command::Emit(reply, params));
        }
        self.finish()
    }

    /// The device goes to sleep or the app to the background; plugins hear
    /// [`events::SUSPEND`].
    pub fn suspend(&mut self, now: Instant) -> Effects {
        self.now = now;
        self.output
            .commands
            .push(Command::Emit(events::SUSPEND, Params::new()));
        self.finish()
    }

    /// Back from [`Kernel::suspend`].
    pub fn resume(&mut self, now: Instant) -> Effects {
        self.now = now;
        self.last_activity = now;
        self.output
            .commands
            .push(Command::Emit(events::RESUME, Params::new()));
        self.finish()
    }

    // ---- Registration -------------------------------------------------

    /// Registers `plugin`, or only lists it if the user switched it off.
    fn add_plugin(&mut self, plugin: &dyn Plugin, disabled: &[String]) {
        let manifest = plugin.manifest();
        if manifest.id != CORE && disabled.iter().any(|id| id == manifest.id.as_str()) {
            self.register_off(plugin, manifest);
        } else {
            self.register_plugin(plugin, manifest);
        }
    }

    fn register_plugin(&mut self, plugin: &dyn Plugin, manifest: Manifest) {
        let id = manifest.id.clone();
        let source = || WarningSource::Plugin(id.clone());
        if id.as_str() != id.namespace() {
            return self.warn(source(), None, "plugin ids must not contain '.'".into());
        }
        if self.shared.plugins.get(id.as_str()).is_some() {
            return self.warn(
                source(),
                None,
                "a plugin with this id is already loaded".into(),
            );
        }
        if !SDK_VERSION.satisfies(&manifest.sdk) {
            let message = format!("requires SDK {}, this is {}", manifest.sdk, SDK_VERSION);
            self.warn(source(), None, message);
            return self.add_plugin_info(manifest, PluginState::Failed, true);
        }

        let mut staging = Staging::default();
        let result = plugin
            .register(&mut PluginRegistrar::new(id.clone(), &mut staging))
            .and_then(|()| self.merge(&id, staging));
        match result {
            Ok(()) => self.add_plugin_info(manifest, PluginState::Running, true),
            Err(error) => {
                let line = match error {
                    RegisterError::Parse { line, .. } => Some(line),
                    _ => None,
                };
                self.warn(source(), line, format!("not loaded: {error}"));
                self.add_plugin_info(manifest, PluginState::Failed, true);
            }
        }
    }

    /// Lists a plugin the user switched off. Only its translations are
    /// loaded, so plugin managers can show its name and description in the
    /// user's language.
    fn register_off(&mut self, plugin: &dyn Plugin, manifest: Manifest) {
        let id = manifest.id.clone();
        if id.as_str() != id.namespace() || self.shared.plugins.get(id.as_str()).is_some() {
            return;
        }
        let mut staging = Staging::default();
        if SDK_VERSION.satisfies(&manifest.sdk)
            && plugin
                .register(&mut PluginRegistrar::new(id, &mut staging))
                .is_ok()
        {
            for catalog in staging.catalogs.drain(..) {
                self.shared.translator.add(catalog);
            }
        }
        self.add_plugin_info(manifest, PluginState::Off, false);
    }

    fn add_plugin_info(&mut self, manifest: Manifest, state: PluginState, enabled: bool) {
        self.shared.plugins.push(PluginInfo {
            manifest,
            state,
            enabled,
            required_for: RequiredFor::default(),
        });
    }

    /// Gives the menus the labels of the actions and settings they list,
    /// and only those: every label costs memory.
    fn set_menu_titles(&mut self) {
        let referenced: Vec<Id> = self.shared.menus.referenced_items().cloned().collect();
        for id in referenced {
            let title = match self.shared.actions.get(id.as_str()) {
                Some(action) => Some(action.title.clone()),
                None => self
                    .shared
                    .settings
                    .specs()
                    .find(|spec| spec.id == id)
                    .map(|spec| spec.title.clone()),
            };
            if let Some(title) = title {
                self.shared.menus.set_title(id, title);
            }
        }
    }

    /// Marks plugins that alone fill a standard slot or provide all fonts:
    /// switching one of them off would leave the user without e.g.
    /// settings, a menu or text, and no way back.
    fn find_required_plugins(&mut self) {
        for slot in [
            slots::HOME,
            slots::READER,
            slots::LIBRARY,
            slots::MENU,
            slots::SETTINGS,
        ] {
            let sole = {
                let mut providers = self
                    .registry
                    .slot_screens(&slot)
                    .map(Id::namespace)
                    .filter(|plugin| *plugin != CORE.as_str());
                match providers.next() {
                    Some(first) if providers.all(|plugin| plugin == first) => first.to_string(),
                    _ => continue,
                }
            };
            if let Some(info) = self.shared.plugins.get_mut(&sole) {
                info.required_for.insert(&slot);
            }
        }
        // Any font can stand in for any role, but without fonts no text
        // is shown at all.
        let mut owners = self.shared.fonts.ids().map(Id::namespace);
        let sole = match owners.next() {
            Some(first) if owners.all(|plugin| plugin == first) => first.to_string(),
            _ => return,
        };
        if let Some(info) = self.shared.plugins.get_mut(&sole) {
            info.required_for.insert(&quireink_sdk::builtin::fonts::UI);
        }
    }

    /// Merges a plugin's contributions: all of them or none.
    fn merge(&mut self, plugin: &Id, mut staging: Staging) -> Result<(), RegisterError> {
        for setting in &staging.settings {
            if self.shared.settings.get(setting.id.as_str()).is_some() {
                return Err(RegisterError::Duplicate {
                    id: setting.id.clone(),
                });
            }
            if !setting.kind.accepts(&setting.default) {
                return Err(RegisterError::Invalid {
                    message: Cow::Owned(format!("invalid default for `{}`", setting.id)),
                });
            }
        }
        if let Some(font) = staging
            .fonts
            .iter()
            .find(|f| self.shared.fonts.contains(f.id.as_str()))
        {
            return Err(RegisterError::Duplicate {
                id: font.id.clone(),
            });
        }
        if let Some(format) = staging
            .formats
            .iter()
            .find(|f| self.shared.formats.contains(f.id.as_str()))
        {
            return Err(RegisterError::Duplicate {
                id: format.id.clone(),
            });
        }
        if let Some(spec) = staging
            .hyphenation
            .iter()
            .find(|h| self.shared.hyphenation.contains(h.id.as_str()))
        {
            return Err(RegisterError::Duplicate {
                id: spec.id.clone(),
            });
        }
        staging.merge_into(&mut self.registry, &mut self.shared.actions)?;
        for setting in staging.settings.drain(..) {
            // Cannot fail: checked above.
            let _ = self.shared.settings.define(setting);
        }
        for source in staging.keymaps.drain(..) {
            // Validated by the registrar.
            self.shared.keymap.load(&source, Origin::Plugin, true);
            self.keymap_sources.push((Origin::Plugin, source));
        }
        self.keymap_presets.append(&mut staging.keymap_presets);
        for catalog in staging.catalogs.drain(..) {
            self.shared.translator.add(catalog);
        }
        for font in staging.fonts.drain(..) {
            // Cannot fail: checked above.
            let _ = self.shared.fonts.add(font);
        }
        for (id, path) in staging.font_files.drain(..) {
            self.add_font_file(plugin, id, path);
        }
        for (role, font, priority) in staging.font_roles.drain(..) {
            self.shared.fonts.propose(role, font, priority);
        }
        for format in staging.formats.drain(..) {
            // Cannot fail: checked above.
            let _ = self.shared.formats.add(format);
        }
        for group in staging.setting_groups.drain(..) {
            if let Err(error) = self.shared.settings.add_group(group) {
                self.warn(WarningSource::Runtime, None, format!("{error}"));
            }
        }
        for spec in staging.hyphenation.drain(..) {
            // Cannot fail: checked above and by the registrar.
            let _ = self.shared.hyphenation.add(spec);
        }
        self.subscriptions.append(&mut staging.subscriptions);
        self.plugin_defaults.extend(
            staging
                .setting_defaults
                .drain(..)
                .map(|(id, value)| (plugin.clone(), id, value)),
        );
        for entry in staging.menu_entries.drain(..) {
            self.shared.menus.add(entry);
        }
        for section in staging.menu_sections.drain(..) {
            self.shared.menus.add_section(section);
        }
        for (item, icon) in staging.icons.drain(..) {
            self.shared.menus.set_icon(item, icon);
        }
        for (item, need) in staging.menu_needs.drain(..) {
            self.shared.menus.set_need(item, need);
        }
        for item in staging.status_items.drain(..) {
            self.shared.status_items.add(item);
        }
        for line in staging.status_lines.drain(..) {
            self.shared.status_items.add_line(line);
        }
        for widget in staging.widgets.drain(..) {
            self.shared.widgets.add(widget);
        }
        for filter in staging.text_filters.drain(..) {
            self.shared.text_filters.add(filter);
        }
        Ok(())
    }

    fn apply_user_config(&mut self, source: &str) {
        for entry in quireink_config::parse(source) {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    self.warn(
                        WarningSource::UserConfig,
                        Some(error.line),
                        error.to_string(),
                    );
                    continue;
                }
            };
            let line = Some(entry.line);
            let value = entry.value.as_str();
            match entry.section {
                "plugins" => {}
                section if section.starts_with("keymap.") => {}
                "fonts" => match (Id::parse(entry.key), Id::parse(&value)) {
                    (Ok(role), Ok(font)) if self.shared.fonts.contains(font.as_str()) => {
                        self.shared.fonts.bind_user(role, font);
                    }
                    (Ok(_), Ok(font)) => self.warn(
                        WarningSource::UserConfig,
                        line,
                        format!("unknown font `{font}`"),
                    ),
                    _ => self.warn(
                        WarningSource::UserConfig,
                        line,
                        "invalid font binding".into(),
                    ),
                },
                "menus" => {
                    let actions: Result<Vec<Id>, _> = value
                        .split(',')
                        .map(str::trim)
                        .filter(|a| !a.is_empty())
                        .map(Id::parse)
                        .collect();
                    match (Id::parse(entry.key), actions) {
                        (Ok(menu), Ok(actions)) => self.shared.menus.customize(menu, actions),
                        _ => self.warn(WarningSource::UserConfig, line, "invalid menu".into()),
                    }
                }
                "slots" => match (Id::parse(entry.key), Id::parse(&value)) {
                    (Ok(slot), Ok(screen)) => self.registry.bind_user(slot, screen),
                    _ => self.warn(
                        WarningSource::UserConfig,
                        line,
                        "invalid slot binding".into(),
                    ),
                },
                "" => self.warn(
                    WarningSource::UserConfig,
                    line,
                    "settings belong in a [<plugin-id>] section".into(),
                ),
                section => {
                    let id = format!("{section}.{}", entry.key);
                    if let Err(error) = self.shared.settings.set_from_text(&id, &value) {
                        self.warn(WarningSource::UserConfig, line, format!("`{id}`: {error}"));
                    }
                }
            }
        }
        self.keymap_sources
            .push((Origin::User, Cow::Owned(source.to_string())));
        for problem in self.shared.keymap.load(source, Origin::User, false) {
            self.warn(
                WarningSource::UserConfig,
                Some(problem.line),
                problem.message.into_owned(),
            );
        }
    }

    /// Reports references to contributions that do not exist.
    fn validate(&mut self) {
        let mut messages = Vec::new();
        for (slot, screen) in self.registry.user_bindings() {
            if !self.registry.slots.contains_key(slot) {
                messages.push(format!("unknown slot `{slot}`"));
            } else if !self.registry.screens.contains_key(screen) {
                messages.push(format!("unknown screen `{screen}` for slot `{slot}`"));
            }
        }
        let preset_actions = self.preset_actions();
        let mut unknown: Vec<&Id> = self
            .shared
            .keymap
            .actions()
            .chain(self.shared.menus.referenced_items())
            .chain(preset_actions.iter())
            .filter(|id| {
                !self.shared.actions.contains(id.as_str())
                    && self.shared.settings.get(id.as_str()).is_none()
                    && !self.shared.menus.is_section(id.as_str())
            })
            .collect();
        unknown.sort();
        unknown.dedup();
        for action in unknown {
            messages.push(format!("unknown action `{action}` in a key map or menu"));
        }
        for message in messages {
            self.warn(WarningSource::Runtime, None, message);
        }
    }

    // ---- Input --------------------------------------------------------

    fn thresholds(&self) -> Thresholds {
        let metrics = Metrics::new(self.shared.device.display.dpi);
        Thresholds {
            long_press_ms: self
                .shared
                .settings
                .int(core_settings::LONG_PRESS_MS.as_str())
                .max(0) as u64,
            tap_slop: metrics.mm(3),
            swipe_distance: metrics.mm(8),
        }
    }

    fn pointer(&mut self, event: PointerEvent) {
        let captured = self.captured.contains(&event.id);
        if captured || event.phase == PointerPhase::Down {
            let handled = self.with_top(|screen, cx| screen.on_pointer(&event, cx));
            if captured {
                if matches!(event.phase, PointerPhase::Up | PointerPhase::Cancel) {
                    self.captured.retain(|&id| id != event.id);
                }
                return;
            }
            if handled == Handled::Yes {
                self.captured.push(event.id);
                return;
            }
        }
        let thresholds = self.thresholds();
        if let Some(recognized) = self.recognizer.pointer(&event, self.now, &thresholds) {
            self.recognized(recognized);
        }
    }

    fn recognized(&mut self, recognized: Recognized) {
        match recognized {
            Recognized::Button { name, long } => {
                let name = Cow::Borrowed(name);
                self.route_key(if long {
                    InputKey::LongButton(name)
                } else {
                    InputKey::Button(name)
                });
            }
            Recognized::Gesture(gesture) => {
                let overlay = self
                    .stack
                    .len()
                    .checked_sub(1)
                    .and_then(|top| self.overlay(top));
                if let Some(region) = overlay
                    && gesture.kind == GestureKind::Tap
                    && !region.contains(gesture.start)
                {
                    self.output.commands.push(Command::Back);
                    return;
                }
                if self.with_top(|screen, cx| screen.on_gesture(gesture, cx)) == Handled::Yes {
                    return;
                }
                let display = self.shared.device.display;
                let zone = Zone::of(gesture.start, display.width, display.height);
                let key = match gesture.kind {
                    GestureKind::Tap => InputKey::Tap(zone),
                    GestureKind::LongPress => InputKey::LongTap(zone),
                    GestureKind::Swipe(direction) => InputKey::Swipe(direction),
                    _ => return,
                };
                self.route_key(key);
            }
        }
    }

    /// Resolves an input through the key map layers of the active screen,
    /// its slot and the global layer.
    fn route_key(&mut self, key: InputKey) {
        let action = {
            let top = self.stack.last();
            let mut layers: Vec<&str> = Vec::with_capacity(3);
            if let Some(top) = top {
                layers.push(top.id.as_str());
                if let Some(slot) = &top.slot {
                    layers.push(slot.as_str());
                }
            }
            layers.push(GLOBAL_KEYMAP_LAYER);
            self.shared.keymap.resolve(&layers, &key).cloned()
        };
        if let Some(action) = action {
            self.dispatch(action);
        }
    }

    fn dispatch(&mut self, action: Id) {
        if self.with_top(|screen, cx| screen.on_action(&action, cx)) == Handled::Yes {
            return;
        }
        match self.shared.actions.get(action.as_str()) {
            Some(spec) => {
                if let Some(handler) = &spec.handler {
                    let env = self.shared.env(self.now);
                    handler(&env, &mut self.output.commands);
                }
            }
            None => self.warn(
                WarningSource::Runtime,
                None,
                format!("unknown action `{action}`"),
            ),
        }
    }

    /// Runs `f` with the active screen and a context for it.
    fn with_top(&mut self, f: impl FnOnce(&mut dyn Screen, &mut Cx<'_, '_>) -> Handled) -> Handled {
        match self.stack.len().checked_sub(1) {
            Some(top) => self.with_screen(top, f),
            None => Handled::No,
        }
    }

    // ---- Commands and drawing ----------------------------------------

    fn finish(&mut self) -> Effects {
        self.run_commands();
        // A screen that set a scope may have closed.
        self.check_scope_owner();
        self.run_commands();
        if mem::take(&mut self.output.redraw)
            && let Some(top) = self.stack.len().checked_sub(1)
        {
            let region = self.region_of(top);
            self.invalidate(region);
        }
        if let Some(area) = self.output.redraw_area.take()
            && let Some(area) = area.intersection(&self.framebuffer.bounds())
        {
            self.invalidate(area);
        }
        let requested = self.output.refresh.take();
        let mut effects = Effects {
            restart: mem::take(&mut self.restart),
            sleep: mem::take(&mut self.sleep),
            ..Effects::default()
        };
        if let Some(dirty) = self.pending.take() {
            self.unrendered = Some(self.unrendered.map_or(dirty, |u| u.union(&dirty)));
            let interval = self
                .shared
                .settings
                .int(core_settings::FULL_REFRESH_INTERVAL.as_str())
                .clamp(0, i64::from(u32::MAX)) as u32;
            let screen = self.framebuffer.bounds();
            let requested = requested.unwrap_or(RefreshMode::Partial);
            let (mode, region) = self.refresh.decide(requested, dirty, screen, interval);
            effects.refresh = Some((region, mode));
        }
        if let Some(at) = self.output.wake_at.take() {
            self.screen_timer = Some(self.screen_timer.map_or(at, |t| t.min(at)));
        }
        let input = self.recognizer.next_deadline(&self.thresholds());
        let notice = self.notice.as_ref().map(|(_, until)| *until);
        let scheduled = self.scheduled.iter().map(|(at, ..)| *at).min();
        // After a screenshot is drawn, a message says where it went.
        let shot = self.screenshot.is_some().then(|| self.now.add_millis(300));
        effects.wake_at = [
            input,
            self.screen_timer,
            notice,
            scheduled,
            self.sleep_due(),
            shot,
        ]
        .into_iter()
        .flatten()
        .min();
        effects
    }

    fn run_commands(&mut self) {
        for _ in 0..MAX_COMMAND_ROUNDS {
            if self.output.commands.is_empty() {
                return;
            }
            for command in mem::take(&mut self.output.commands) {
                self.apply(command);
            }
        }
        if !self.output.commands.is_empty() {
            self.output.commands.clear();
            self.warn(
                WarningSource::Runtime,
                None,
                "too many chained commands; the rest was dropped".into(),
            );
        }
    }

    fn apply(&mut self, command: Command) {
        match command {
            Command::OpenSlot(slot, params) => {
                let open = self
                    .stack
                    .iter()
                    .position(|s| s.slot.as_ref() == Some(&slot));
                if let Some(index) = open {
                    if params.is_empty() || self.stack[index].params == params {
                        if index + 1 < self.stack.len() {
                            self.close_above(index);
                        }
                        return;
                    }
                    // The slot for something else (e.g. another book):
                    // opened anew in place of the old screen.
                    self.stack.truncate(index);
                    self.stop_for_closed_screens();
                }
                match self.registry.resolve_slot(slot.as_str()).cloned() {
                    Some(screen) => self.open(screen, Some(slot), &params),
                    None => self.warn(
                        WarningSource::Runtime,
                        None,
                        format!("no screen for slot `{slot}`"),
                    ),
                }
            }
            Command::OpenScreen(screen, params) => self.open(screen, None, &params),
            Command::Emit(event, params) => {
                // A page turned, also by itself, is use.
                if event == events::PAGE_TURNED {
                    self.last_activity = self.now;
                }
                let env = self.shared.env(self.now);
                for (_, handler) in self.subscriptions.iter().filter(|(id, _)| *id == event) {
                    handler(&env, &params, &mut self.output.commands);
                }
                for index in 0..self.stack.len() {
                    self.with_screen(index, |screen, cx| {
                        screen.on_event(&event, &params, cx);
                        Handled::Yes
                    });
                }
            }
            Command::RunAction(action) => self.dispatch(action),
            Command::SetPluginEnabled(id, enabled) => self.set_plugin_enabled(&id, enabled),
            Command::Restart => self.restart = true,
            Command::Sleep => self.sleep = self.shared.sleep,
            Command::SetSecret(change) => {
                use quireink_sdk::secrets;
                self.shared.secrets.apply(change);
                let text = self.shared.secrets.encode();
                let storage = self.shared.storage.as_ref();
                let _ = storage.create_dir(".quireink");
                let saved = if self.shared.secrets.is_empty() {
                    storage.remove(secrets::FILE).or(Ok(()))
                } else {
                    storage.write(secrets::FILE, text.as_bytes())
                };
                if let Err(error) = saved {
                    let message = format!("{}: {error}", secrets::FILE);
                    self.warn(WarningSource::Runtime, None, message);
                }
            }
            Command::Screenshot(path) => {
                // The whole screen is drawn anew, and saved as it is.
                self.screenshot = Some(path);
                let bounds = self.framebuffer.bounds();
                self.invalidate(bounds);
            }
            Command::SetClipboard(text) => {
                self.shared.clipboard = clipped(&text);
                self.copied = Some(self.shared.clipboard.clone());
            }
            Command::ReadClipboard(reply, params) if self.reads_clipboard => {
                self.clipboard_reads.push((reply, params));
            }
            Command::ReadClipboard(reply, params) => self.reply_clipboard(reply, params),
            Command::Speak(speech) => self.speak(speech),
            Command::StopSpeaking => {
                if !self.speaking.is_empty() {
                    self.speaking.clear();
                    self.speech_calls.push(SpeechCall::Stop);
                }
            }
            Command::Serve {
                server,
                port,
                events,
            } => self.serve(server, port, events),
            Command::StopServing(server) => {
                if let Some(at) = self.servers.iter().position(|(_, id, ..)| *id == server) {
                    self.stop_server(at);
                }
            }
            Command::Respond(response) => self.respond(response),
            Command::Back => {
                if self.stack.len() > 1 {
                    self.close_above(self.stack.len() - 2);
                }
            }
            Command::SetSetting(id, value) if self.shared.settings.goes_to_scope(id.as_str()) => {
                self.set_in_scope(&id, value);
            }
            Command::SetScope(path) => self.set_scope(path),
            Command::Http(request) => self.request(request),
            Command::SetSetting(id, value) => match self.shared.settings.set(id.as_str(), value) {
                Ok(()) => {
                    self.persist_setting(&id);
                    self.setting_changed(&id);
                    self.output.commands.push(Command::Emit(
                        events::SETTING_CHANGED,
                        Params::new().with(builtin_params::ID, id.as_str().to_string()),
                    ));
                }
                Err(error) => {
                    self.warn(WarningSource::Runtime, None, format!("`{id}`: {error}"));
                }
            },
            Command::Redraw(mode) => {
                self.output.redraw = true;
                self.output.request_refresh(mode);
            }
            Command::SetMenu(menu, entries) => self.set_menu(menu, entries),
            Command::Notify(text) => self.notify(text),
            Command::Bind {
                layer,
                input,
                action,
            } => self.bind(&layer, &input, action),
            Command::Schedule(event, params, at) => {
                self.scheduled.retain(|(_, e, _)| *e != event);
                self.scheduled.push((at, event, params));
            }
            _ => {}
        }
    }

    fn open(&mut self, id: Id, slot: Option<Id>, params: &Params) {
        let Some(spec) = self.registry.screens.get(&id) else {
            return self.warn(
                WarningSource::Runtime,
                None,
                format!("unknown screen `{id}`"),
            );
        };
        let env = self.shared.env(self.now);
        let screen = (spec.factory)(&env, params);
        self.stack.push(ActiveScreen {
            id,
            slot,
            params: params.clone(),
            screen,
        });
        let top = self.stack.len() - 1;
        let overlay = self.overlay(top);
        self.screen_changed(self.region_of(top), overlay.is_some());
    }

    /// Shows `text` with the screen bound to the notice slot until the
    /// notice time is over.
    fn notify(&mut self, text: String) {
        let Some(id) = self.registry.resolve_slot(slots::NOTICE.as_str()).cloned() else {
            return;
        };
        let Some(spec) = self.registry.screens.get(&id) else {
            return;
        };
        let env = self.shared.env(self.now);
        let params = Params::new().with(builtin_params::TEXT, text);
        let screen = (spec.factory)(&env, &params);
        let seconds = self
            .shared
            .settings
            .int(core_settings::NOTICE_SECONDS.as_str())
            .clamp(1, 60) as u32;
        let region = screen.region(&env).unwrap_or(self.framebuffer.bounds());
        self.end_notice();
        self.notice = Some((screen, self.now.add_millis(u64::from(seconds) * 1000)));
        self.invalidate(region);
        self.output.request_refresh(RefreshMode::Fast);
    }

    /// Binds an input for the user (see [`Command::Bind`]).
    fn bind(&mut self, layer: &str, input: &str, action: Option<Id>) {
        let valid_layer = layer == GLOBAL_KEYMAP_LAYER || Id::parse(layer).is_ok();
        if !valid_layer || !keymap::is_input(input) {
            return self.warn(
                WarningSource::Runtime,
                None,
                format!("cannot bind `{input}` in `{layer}`"),
            );
        }
        let section = format!("keymap.{layer}");
        match &action {
            Some(action) => self.save_config(&section, input, action.as_str()),
            None => {
                let storage = self.shared.storage.as_ref();
                let source = read_config(storage).unwrap_or_default();
                let updated = quireink_config::remove(&source, &section, input);
                let _ = storage.write(CONFIG_FILE, updated.as_bytes());
            }
        }
        // The user's bindings as the file has them now.
        let source = read_config(self.shared.storage.as_ref()).unwrap_or_default();
        self.keymap_sources
            .retain(|(origin, _)| *origin != Origin::User);
        self.keymap_sources.push((Origin::User, Cow::Owned(source)));
        self.rebuild_keymap();
    }

    /// Takes the notice away, if one shows.
    fn end_notice(&mut self) {
        let Some((screen, _)) = self.notice.take() else {
            return;
        };
        let env = self.shared.env(self.now);
        let region = screen.region(&env).unwrap_or(self.framebuffer.bounds());
        self.invalidate(region);
        self.output.request_refresh(RefreshMode::Fast);
    }

    /// Closes all screens above `index`.
    fn close_above(&mut self, index: usize) {
        let mut region: Option<Rect> = None;
        let mut overlays_only = true;
        for i in index + 1..self.stack.len() {
            let r = self.region_of(i);
            overlays_only &= self.overlay(i).is_some();
            region = Some(region.map_or(r, |u: Rect| u.union(&r)));
        }
        self.stack.truncate(index + 1);
        self.stop_for_closed_screens();
        if let Some(region) = region {
            self.screen_changed(region, overlays_only);
        }
    }

    /// The region of an overlay, `None` for full screens.
    fn overlay(&self, index: usize) -> Option<Rect> {
        let env = self.shared.env(self.now);
        self.stack.get(index)?.screen.region(&env)
    }

    /// The area a screen occupies.
    fn region_of(&self, index: usize) -> Rect {
        self.overlay(index)
            .unwrap_or_else(|| self.framebuffer.bounds())
    }

    /// Actions bound by any key map preset, for validation.
    fn preset_actions(&self) -> Vec<Id> {
        let mut all = Keymap::default();
        for (_, source) in &self.keymap_presets {
            all.load(source, Origin::Preset, true);
        }
        all.actions().cloned().collect()
    }

    /// Builds the key map from all sources and the selected preset.
    /// Problems were reported when the sources were first loaded.
    fn rebuild_keymap(&mut self) {
        let mut keymap = Keymap::default();
        let preset = self
            .shared
            .settings
            .text(core_settings::KEYMAP_PRESET.as_str());
        if let Some((_, source)) = self
            .keymap_presets
            .iter()
            .find(|(id, _)| id.as_str() == preset)
        {
            keymap.load(source, Origin::Preset, true);
        }
        for (origin, source) in &self.keymap_sources {
            keymap.load(source, *origin, *origin != Origin::User);
        }
        keymap.shrink_to_fit();
        self.shared.keymap = keymap;
    }

    fn invalidate(&mut self, region: Rect) {
        self.pending = Some(self.pending.map_or(region, |p| p.union(&region)));
    }

    /// Redraws `region`: the topmost full screen and every overlay above
    /// it, each clipped to the region.
    fn redraw(&mut self, region: Rect) {
        let env = self.shared.env(self.now);
        let regions: Vec<Option<Rect>> = self.stack.iter().map(|s| s.screen.region(&env)).collect();
        let base = regions.iter().rposition(Option::is_none).unwrap_or(0);
        let bounds = self.framebuffer.bounds();
        let mut canvas = self.framebuffer.canvas();
        let mut canvas = canvas.clipped(region);
        canvas.clear(Color::WHITE);
        for (active, own) in self.stack[base..].iter_mut().zip(&regions[base..]) {
            let Some(area) = own.unwrap_or(bounds).intersection(&region) else {
                continue;
            };
            active.screen.draw(&mut canvas.clipped(area), &env);
        }
        // A message over everything.
        if let Some((notice, _)) = &mut self.notice
            && let Some(area) = notice.region(&env).unwrap_or(bounds).intersection(&region)
        {
            notice.draw(&mut canvas.clipped(area), &env);
        }
    }

    /// Runs `f` with the screen at `index` and a context for it.
    fn with_screen(
        &mut self,
        index: usize,
        f: impl FnOnce(&mut dyn Screen, &mut Cx<'_, '_>) -> Handled,
    ) -> Handled {
        let earlier = mem::take(&mut self.output.redraw);
        let Some(active) = self.stack.get_mut(index) else {
            self.output.redraw = earlier;
            return Handled::No;
        };
        let env = self.shared.env(self.now);
        let mut cx = Cx::new(&env, &mut self.output);
        let handled = f(active.screen.as_mut(), &mut cx);
        // `request_redraw` means the caller's own region. A screen below an
        // overlay is still partly visible (e.g. the page behind a menu that
        // changes a reading setting); one below a full screen is redrawn
        // when it is shown again.
        let requested = mem::replace(&mut self.output.redraw, earlier);
        if requested {
            let top = self.stack.len() - 1;
            if index == top {
                self.output.redraw = true;
            } else if (index + 1..=top).all(|i| self.overlay(i).is_some()) {
                let region = self.region_of(index);
                self.invalidate(region);
            }
        }
        handled
    }

    /// Writes a changed setting to the configuration file on the device,
    /// keeping the rest of the file as it is.
    fn persist_setting(&mut self, id: &Id) {
        let Some(value) = self.shared.settings.global(id.as_str()) else {
            return;
        };
        let Some((section, key)) = id.as_str().split_once('.') else {
            return;
        };
        let value = value.to_config_text();
        self.save_config(section, key, &value);
    }

    /// The effective values of the scoped settings, to see what a scope
    /// change changes.
    fn scoped_values(&self) -> Vec<(Id, Option<quireink_sdk::SettingValue>)> {
        let settings = &self.shared.settings;
        settings
            .specs()
            .filter(|spec| spec.scoped)
            .map(|spec| (spec.id.clone(), settings.get(spec.id.as_str()).cloned()))
            .collect()
    }

    /// Announces the scoped settings whose value differs from `before`.
    fn announce_scope_changes(&mut self, before: Vec<(Id, Option<quireink_sdk::SettingValue>)>) {
        for (id, old) in before {
            if self.shared.settings.get(id.as_str()) != old.as_ref() {
                self.setting_changed(&id);
                self.output.commands.push(Command::Emit(
                    events::SETTING_CHANGED,
                    Params::new().with(builtin_params::ID, id.as_str().to_string()),
                ));
            }
        }
    }

    /// Queues an HTTP request for the shell, or fails it without network.
    fn request(&mut self, request: quireink_sdk::HttpRequest) {
        if !self.shared.network {
            let params = request
                .params
                .with(quireink_sdk::http_params::STATUS, "0")
                .with(quireink_sdk::http_params::BODY, "no network");
            self.output
                .commands
                .push(Command::Emit(request.reply, params));
            return;
        }
        let body = match &request.body_from {
            Some(path) => {
                match quireink_sdk::read_file(self.shared.storage.as_ref(), path, MAX_FILE) {
                    Ok(data) => Some(data),
                    Err(e) => {
                        let params = request
                            .params
                            .with(quireink_sdk::http_params::STATUS, "0")
                            .with(quireink_sdk::http_params::BODY, format!("{e}"));
                        self.output
                            .commands
                            .push(Command::Emit(request.reply, params));
                        return;
                    }
                }
            }
            None => request.body.map(String::into_bytes),
        };
        self.next_request = self.next_request.wrapping_add(1);
        let id = self.next_request;
        let max_response = if request.save_to.is_some() {
            MAX_FILE
        } else {
            MAX_RESPONSE
        };
        self.requests.push(HttpCall {
            id,
            method: request.method,
            url: request.url,
            headers: request.headers,
            body,
            max_response,
        });
        self.awaiting
            .push((id, request.reply, request.params, request.save_to));
    }

    /// Activates the scope stored at `path` for the top screen, or ends
    /// the scope (see [`Command::SetScope`]).
    fn set_scope(&mut self, path: Option<String>) {
        if path == self.scope.as_ref().map(|(p, _)| p.clone()) {
            // Same scope (e.g. the reader shown again): keep it.
            if let Some((_, owner)) = &mut self.scope {
                *owner = self.stack.last().map(screen_key);
            }
            return;
        }
        let before = self.scoped_values();
        let values = path.as_deref().map(|path| {
            let storage = self.shared.storage.as_ref();
            let text = quireink_sdk::read_file(storage, path, 16 * 1024)
                .ok()
                .and_then(|data| String::from_utf8(data).ok())
                .unwrap_or_default();
            let mut values = quireink_sdk::collections::VecMap::new();
            for entry in quireink_config::parse(&text).filter_map(Result::ok) {
                let id = format!("{}.{}", entry.section, entry.key);
                let parsed = self
                    .shared
                    .settings
                    .specs()
                    .find(|spec| spec.id.as_str() == id)
                    .and_then(|spec| {
                        Some((spec.id.clone(), spec.kind.parse(&entry.value.as_str())?))
                    });
                if let Some((id, value)) = parsed {
                    values.insert(id, value);
                }
            }
            values
        });
        self.shared.settings.set_scope(values);
        self.scope = path.map(|path| (path, self.stack.last().map(screen_key)));
        self.announce_scope_changes(before);
    }

    /// Changes a setting in the active scope and saves the scope.
    fn set_in_scope(&mut self, id: &Id, value: quireink_sdk::SettingValue) {
        let before = self.scoped_values();
        if let Err(error) = self.shared.settings.set_in_scope(id.as_str(), value) {
            return self.warn(WarningSource::Runtime, None, format!("`{id}`: {error}"));
        }
        self.save_scope();
        self.announce_scope_changes(before);
        // The switch itself is not a scoped value others see change.
        if *id == core_settings::SCOPE_ONLY {
            self.output.commands.push(Command::Emit(
                events::SETTING_CHANGED,
                Params::new().with(builtin_params::ID, id.as_str().to_string()),
            ));
        }
    }

    /// Writes the active scope's values to its file (removed when empty).
    fn save_scope(&mut self) {
        let (Some((path, _)), Some(values)) = (&self.scope, self.shared.settings.scope()) else {
            return;
        };
        let storage = self.shared.storage.as_ref();
        if values.is_empty() {
            let _ = storage.remove(path);
            return;
        }
        let mut text = String::new();
        for (id, value) in values.iter() {
            if let Some((section, key)) = id.as_str().split_once('.') {
                text = quireink_config::set(&text, section, key, &value.to_config_text());
            }
        }
        if storage.write(path, text.as_bytes()).is_err() {
            self.warn(
                WarningSource::Runtime,
                None,
                format!("could not save settings to {path}"),
            );
        }
    }

    /// Ends the scope when the screen that set it is gone.
    fn check_scope_owner(&mut self) {
        let Some((_, owner)) = &self.scope else {
            return;
        };
        let alive = self.stack.iter().any(|s| Some(screen_key(s)) == *owner);
        if !alive {
            self.set_scope(None);
        }
    }

    /// Redefines or resets a menu (see [`Command::SetMenu`]) and saves it.
    fn set_menu(&mut self, menu: Id, entries: Option<Vec<Id>>) {
        let value = entries
            .as_ref()
            .map(|ids| ids.iter().map(Id::as_str).collect::<Vec<_>>().join(", "));
        match entries {
            Some(ids) => self.shared.menus.customize(menu.clone(), ids),
            None => self.shared.menus.reset(menu.as_str()),
        }
        // New entries need their labels.
        self.set_menu_titles();
        let storage = self.shared.storage.as_ref();
        let source = read_config(storage).unwrap_or_default();
        let updated = match &value {
            Some(value) => quireink_config::set(&source, "menus", menu.as_str(), value),
            None => quireink_config::remove(&source, "menus", menu.as_str()),
        };
        if storage.write(CONFIG_FILE, updated.as_bytes()).is_err() {
            self.warn(
                WarningSource::Runtime,
                None,
                format!("could not save menu `{menu}` to {CONFIG_FILE}"),
            );
        }
        self.output.commands.push(Command::Emit(
            events::MENU_CHANGED,
            Params::new().with(builtin_params::ID, menu.as_str().to_string()),
        ));
    }

    /// Adds a font file of `plugin`, described by its header.
    fn add_font_file(&mut self, plugin: &Id, id: Id, path: String) {
        let source = WarningSource::Plugin(plugin.clone());
        if !quireink_text_outlines() {
            return self.warn(
                source,
                None,
                format!("{path}: this device only reads fonts made for it (.qifp)"),
            );
        }
        let storage = self.shared.storage.as_ref();
        let info = quireink_sdk::text::sfnt::read_info(&mut |offset, buf| {
            storage.read_at(&path, offset, buf).unwrap_or(0)
        });
        match info {
            Some(info) => {
                if let Err(error) = self.shared.fonts.add_file(id, path, info) {
                    self.warn(source, None, format!("{error}"));
                }
            }
            None => self.warn(source, None, format!("{path}: not a font")),
        }
    }

    /// Chooses the reading font family from `core.reading-font` and loads
    /// its files.
    fn apply_reading_font(&mut self) {
        let chosen = self
            .shared
            .settings
            .text(core_settings::READING_FONT.as_str())
            .to_string();
        let family = (chosen != core_settings::DEFAULT_FONT
            && self.shared.fonts.families().contains(&chosen.as_str()))
        .then_some(chosen.as_str());
        self.shared.fonts.set_family(family);
        for path in self.shared.fonts.load(self.shared.storage.as_ref()) {
            self.warn(
                WarningSource::Runtime,
                None,
                format!("{path}: font could not be loaded"),
            );
        }
    }

    /// Sets `key` in `section` of the configuration file on the device.
    fn save_config(&mut self, section: &str, key: &str, value: &str) {
        let storage = self.shared.storage.as_ref();
        let source = read_config(storage).unwrap_or_default();
        let updated = quireink_config::set(&source, section, key, value);
        if storage.write(CONFIG_FILE, updated.as_bytes()).is_err() {
            self.warn(
                WarningSource::Runtime,
                None,
                format!("could not save `{section}.{key}` to {CONFIG_FILE}"),
            );
        }
    }

    /// Switches a plugin for the next start (see [`Command::SetPluginEnabled`]).
    fn set_plugin_enabled(&mut self, id: &Id, enabled: bool) {
        let Some(info) = self.shared.plugins.get(id.as_str()) else {
            return self.warn(
                WarningSource::Runtime,
                None,
                format!("unknown plugin `{id}`"),
            );
        };
        if !enabled && !info.can_switch_off() {
            return self.warn(
                WarningSource::Runtime,
                None,
                format!("plugin `{id}` cannot be switched off"),
            );
        }
        if info.enabled == enabled {
            return;
        }
        self.save_config("plugins", id.as_str(), if enabled { "on" } else { "off" });
        if let Some(info) = self.shared.plugins.get_mut(id.as_str()) {
            info.enabled = enabled;
        }
    }

    /// After the screen stack changed: redraw `region`, flashing for full
    /// screens and refreshing quickly when only overlays changed.
    fn screen_changed(&mut self, region: Rect, overlays_only: bool) {
        self.captured.clear();
        self.screen_timer = None;
        self.output.wake_at = None;
        self.invalidate(region);
        self.output.request_refresh(if overlays_only {
            RefreshMode::Partial
        } else {
            RefreshMode::Full
        });
        self.with_top(|screen, cx| {
            screen.on_show(cx);
            Handled::Yes
        });
    }

    fn setting_changed(&mut self, id: &Id) {
        if *id == core_settings::LANGUAGE {
            self.shared
                .translator
                .set_language(self.shared.settings.text(id.as_str()));
        } else if *id == core_settings::NIGHT_MODE {
            self.framebuffer
                .set_inverted(self.shared.settings.bool(id.as_str()));
        }
        if *id == core_settings::KEYMAP_PRESET {
            self.rebuild_keymap();
        }
        if *id == core_settings::READING_FONT {
            self.apply_reading_font();
        }
        if *id == core_settings::LANGUAGE || *id == core_settings::NIGHT_MODE {
            let bounds = self.framebuffer.bounds();
            self.invalidate(bounds);
            self.output.request_refresh(RefreshMode::Full);
        }
    }

    fn warn(&mut self, source: WarningSource, line: Option<u32>, message: String) {
        self.warnings.push(Warning {
            source,
            line,
            message,
        });
    }
}

/// The configuration file on the device, if any.
fn read_config(storage: &dyn Storage) -> Option<String> {
    let data = quireink_sdk::read_file(storage, CONFIG_FILE, 64 * 1024).ok()?;
    String::from_utf8(data).ok()
}

/// Plugin ids switched on (`on`) or off in the `[plugins]` section of a
/// user config.
fn plugin_switches(source: &str, on: bool) -> Vec<String> {
    quireink_config::parse(source)
        .filter_map(Result::ok)
        .filter(|entry| entry.section == "plugins" && entry.value.as_bool() == Some(on))
        .map(|entry| String::from(entry.key))
        .collect()
}

/// Everything plugins can read through [`Env`].
struct Shared {
    device: DeviceInfo,
    settings: Settings,
    translator: Translator,
    fonts: Fonts,
    formats: Formats,
    hyphenation: Hyphenation,
    menus: Menus,
    actions: Actions,
    plugins: Plugins,
    status: SystemStatus,
    status_items: StatusItems,
    widgets: quireink_sdk::widgets::Widgets,
    text_filters: quireink_sdk::filters::TextFilters,
    network: bool,
    /// Text copied here.
    clipboard: String,
    /// The system's clipboard has text (see
    /// [`Kernel::set_clipboard_available`]).
    system_clipboard: bool,
    /// The shell speaks (see [`KernelBuilder::speech`]).
    speech: bool,
    /// The shell sleeps after the sleep screen (see [`KernelBuilder::sleep`]).
    sleep: bool,
    /// Slots some screen is bound to.
    slots: Vec<Id>,
    /// Folders fonts are read from.
    font_dirs: Vec<String>,
    /// Passwords and keys (see [`quireink_sdk::secrets`]).
    secrets: quireink_sdk::secrets::Secrets,
    storage: Box<dyn Storage>,
    keymap: Keymap,
}

impl Shared {
    fn env(&self, now: Instant) -> Env<'_> {
        Env {
            device: &self.device,
            now,
            metrics: Metrics::new(self.device.display.dpi),
            settings: &self.settings,
            i18n: &self.translator,
            fonts: &self.fonts,
            formats: &self.formats,
            hyphenation: &self.hyphenation,
            menus: &self.menus,
            actions: &self.actions,
            plugins: &self.plugins,
            status: &self.status,
            status_items: &self.status_items,
            widgets: &self.widgets,
            text_filters: &self.text_filters,
            network: self.network,
            clipboard: self.system_clipboard || !self.clipboard.is_empty(),
            speech: self.speech,
            sleep: self.sleep,
            slots: &self.slots,
            font_dirs: &self.font_dirs,
            secrets: &self.secrets,
            storage: self.storage.as_ref(),
            keys: &self.keymap,
        }
    }
}

/// `text` cut to what the clipboard holds, at a character boundary.
fn clipped(text: &str) -> String {
    let mut end = text.len().min(quireink_sdk::MAX_CLIPBOARD);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    String::from(&text[..end])
}

/// Whether this build renders OpenType/TrueType fonts.
fn quireink_text_outlines() -> bool {
    quireink_sdk::text::OUTLINES
}

/// Identifies an open screen instance (by its address), e.g. the owner of
/// a settings scope.
fn screen_key(screen: &ActiveScreen) -> usize {
    core::ptr::from_ref::<dyn Screen>(screen.screen.as_ref()).cast::<()>() as usize
}
