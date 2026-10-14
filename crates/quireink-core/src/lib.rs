//! The QuireInk kernel.
//!
//! The kernel hosts plugins and connects them: it keeps the registries of
//! contributions, resolves slots to screens, turns raw input into gestures
//! and actions through layered key maps, applies plugin commands and decides
//! how the e-ink panel is refreshed. It contains no product UI — even its own
//! contributions are registered as the `core` plugin.
//!
//! The kernel is *sans-I/O*: shells feed it [`InputEvent`]s and time, and
//! execute the returned [`Effects`]. It never blocks, spawns threads or
//! talks to an operating system, so it runs unchanged on microcontrollers,
//! Linux, Android and in the browser.
//!
//! ```ignore
//! let mut kernel = Kernel::builder(device).plugin(MyPlugin).build()?;
//! let effects = kernel.start(clock.now());
//! loop {
//!     execute(effects, &kernel);              // refresh the panel, arm timers
//!     effects = match wait_for_input_or_timer() {
//!         Input(event) => kernel.handle(clock.now(), event),
//!         Timer => kernel.tick(clock.now()),
//!     };
//! }
//! ```

#![no_std]

extern crate alloc;

mod core_plugin;
mod input;
mod keymap;
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
    Color, DeviceInfo, FrameView, InputEvent, Instant, PointerEvent, PointerPhase, Rect,
    RefreshMode,
};
use quireink_sdk::builtin::{GLOBAL_KEYMAP_LAYER, settings as core_settings, slots};
use quireink_sdk::{
    Command, Cx, Env, Fonts, GestureKind, Handled, Id, Manifest, Metrics, Output, Plugin,
    RegisterError, SDK_VERSION, Screen, Settings,
};

use crate::core_plugin::{CORE, CorePlugin};
use crate::input::{Recognized, Recognizer, Thresholds};
use crate::keymap::{InputKey, Keymap, Origin, Zone};
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
}

/// The effects of one kernel call. Refreshes are already coalesced: there
/// is at most one per call.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Effects {
    refresh: Option<(Rect, RefreshMode)>,
    wake_at: Option<Instant>,
}

impl Effects {
    pub fn refresh(&self) -> Option<(Rect, RefreshMode)> {
        self.refresh
    }

    pub fn wake_at(&self) -> Option<Instant> {
        self.wake_at
    }

    pub fn is_empty(&self) -> bool {
        self.refresh.is_none() && self.wake_at.is_none()
    }

    pub fn iter(&self) -> impl Iterator<Item = Effect> {
        let refresh = self
            .refresh
            .map(|(region, mode)| Effect::Refresh { region, mode });
        refresh.into_iter().chain(self.wake_at.map(Effect::WakeAt))
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
    device_keymap: Option<Cow<'static, str>>,
    user_config: Option<String>,
    glyph_cache: usize,
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

    /// Memory for glyphs rasterized at runtime (default 32 KiB). Bitmap font
    /// packs do not use it.
    pub fn glyph_cache(mut self, bytes: usize) -> Self {
        self.glyph_cache = bytes;
        self
    }

    pub fn build(self) -> Result<Kernel, KernelError> {
        let display = self.device.display;
        let framebuffer = Framebuffer::new(display.width, display.height, display.format)
            .map_err(KernelError::Graphics)?;
        let mut kernel = Kernel {
            device: self.device,
            framebuffer,
            registry: Registry::default(),
            keymap: Keymap::default(),
            settings: Settings::new(),
            translator: Translator::new(),
            fonts: Fonts::new(self.glyph_cache),
            recognizer: Recognizer::default(),
            captured: Vec::new(),
            stack: Vec::new(),
            refresh: RefreshPolicy::default(),
            output: Output::default(),
            warnings: Vec::new(),
            plugins: Vec::new(),
            now: Instant::default(),
        };

        let disabled = self
            .user_config
            .as_deref()
            .map(disabled_plugins)
            .unwrap_or_default();
        let core: Box<dyn Plugin> = Box::new(CorePlugin);
        for plugin in core::iter::once(core).chain(self.plugins) {
            let manifest = plugin.manifest();
            if manifest.id != CORE && disabled.iter().any(|id| id == manifest.id.as_str()) {
                continue;
            }
            kernel.register_plugin(plugin.as_ref(), manifest);
        }

        if let Some(source) = &self.device_keymap {
            for problem in kernel.keymap.load(source, Origin::Device, true) {
                kernel.warn(
                    WarningSource::DeviceKeymap,
                    Some(problem.line),
                    problem.message.into_owned(),
                );
            }
        }
        if let Some(source) = &self.user_config {
            kernel.apply_user_config(source);
        }
        kernel.validate();
        kernel
            .translator
            .set_language(kernel.settings.text(core_settings::LANGUAGE.as_str()));
        kernel
            .framebuffer
            .set_inverted(kernel.settings.bool(core_settings::NIGHT_MODE.as_str()));
        Ok(kernel)
    }
}

/// A screen on the navigation stack.
struct ActiveScreen {
    id: Id,
    /// The slot it was opened for, if any.
    slot: Option<Id>,
    screen: Box<dyn Screen>,
}

/// The QuireInk kernel. See the [crate documentation](crate).
pub struct Kernel {
    device: DeviceInfo,
    framebuffer: Framebuffer,
    registry: Registry,
    keymap: Keymap,
    settings: Settings,
    translator: Translator,
    fonts: Fonts,
    recognizer: Recognizer,
    /// Pointer ids captured by the active screen.
    captured: Vec<u8>,
    stack: Vec<ActiveScreen>,
    refresh: RefreshPolicy,
    output: Output,
    warnings: Vec<Warning>,
    plugins: Vec<Manifest>,
    now: Instant,
}

impl fmt::Debug for Kernel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Kernel")
            .field("device", &self.device.id)
            .field("plugins", &self.plugins)
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
            device_keymap: None,
            user_config: None,
            glyph_cache: DEFAULT_GLYPH_CACHE,
        }
    }

    /// Opens the home slot. Call once after building.
    pub fn start(&mut self, now: Instant) -> Effects {
        self.now = now;
        self.output.commands.push(Command::OpenSlot(slots::HOME));
        self.finish()
    }

    /// Handles an input event.
    pub fn handle(&mut self, now: Instant, event: InputEvent) -> Effects {
        self.now = now;
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

    /// Advances time, e.g. when a requested [`Effect::WakeAt`] is due.
    pub fn tick(&mut self, now: Instant) -> Effects {
        self.now = now;
        let thresholds = self.thresholds();
        for recognized in self.recognizer.tick(now, &thresholds) {
            self.recognized(recognized);
        }
        self.finish()
    }

    /// The framebuffer the shell transfers to the panel.
    pub fn frame(&self) -> FrameView<'_> {
        self.framebuffer.view()
    }

    pub fn device(&self) -> &DeviceInfo {
        &self.device
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn translator(&self) -> &Translator {
        &self.translator
    }

    pub fn fonts(&self) -> &Fonts {
        &self.fonts
    }

    /// Manifests of all successfully registered plugins, `core` first.
    pub fn plugins(&self) -> &[Manifest] {
        &self.plugins
    }

    /// The id of the screen currently shown.
    pub fn active_screen(&self) -> Option<&Id> {
        self.stack.last().map(|s| &s.id)
    }

    /// Returns and clears the collected warnings.
    pub fn take_warnings(&mut self) -> Vec<Warning> {
        mem::take(&mut self.warnings)
    }

    // ---- Registration -------------------------------------------------

    fn register_plugin(&mut self, plugin: &dyn Plugin, manifest: Manifest) {
        let id = manifest.id.clone();
        let source = || WarningSource::Plugin(id.clone());
        if id.as_str() != id.namespace() {
            return self.warn(source(), None, "plugin ids must not contain '.'".into());
        }
        if self.plugins.iter().any(|m| m.id == id) {
            return self.warn(
                source(),
                None,
                "a plugin with this id is already loaded".into(),
            );
        }
        if !SDK_VERSION.satisfies(&manifest.sdk) {
            let message = format!("requires SDK {}, this is {}", manifest.sdk, SDK_VERSION);
            return self.warn(source(), None, message);
        }

        let mut staging = Staging::default();
        let result = plugin
            .register(&mut PluginRegistrar::new(id.clone(), &mut staging))
            .and_then(|()| self.merge(staging));
        match result {
            Ok(()) => self.plugins.push(manifest),
            Err(error) => {
                let line = match error {
                    RegisterError::Parse { line, .. } => Some(line),
                    _ => None,
                };
                self.warn(source(), line, format!("not loaded: {error}"));
            }
        }
    }

    /// Merges a plugin's contributions — all of them or none.
    fn merge(&mut self, mut staging: Staging) -> Result<(), RegisterError> {
        for setting in &staging.settings {
            if self.settings.get(setting.id.as_str()).is_some() {
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
            .find(|f| self.fonts.contains(f.id.as_str()))
        {
            return Err(RegisterError::Duplicate {
                id: font.id.clone(),
            });
        }
        staging.merge_into(&mut self.registry)?;
        for setting in staging.settings.drain(..) {
            // Cannot fail: checked above.
            let _ = self.settings.define(setting);
        }
        for source in &staging.keymaps {
            // Validated by the registrar.
            self.keymap.load(source, Origin::Plugin, true);
        }
        for catalog in staging.catalogs.drain(..) {
            self.translator.add(catalog);
        }
        for font in staging.fonts.drain(..) {
            // Cannot fail: checked above.
            let _ = self.fonts.add(font);
        }
        for (role, font, priority) in staging.font_roles.drain(..) {
            self.fonts.propose(role, font, priority);
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
                    (Ok(role), Ok(font)) if self.fonts.contains(font.as_str()) => {
                        self.fonts.bind_user(role, font);
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
                    if let Err(error) = self.settings.set_from_text(&id, &value) {
                        self.warn(WarningSource::UserConfig, line, format!("`{id}`: {error}"));
                    }
                }
            }
        }
        for problem in self.keymap.load(source, Origin::User, false) {
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
        let mut unknown: Vec<&Id> = self
            .keymap
            .actions()
            .filter(|action| !self.registry.actions.contains_key(*action))
            .collect();
        unknown.sort();
        unknown.dedup();
        for action in unknown {
            messages.push(format!("key map refers to unknown action `{action}`"));
        }
        for message in messages {
            self.warn(WarningSource::Runtime, None, message);
        }
    }

    // ---- Input --------------------------------------------------------

    fn thresholds(&self) -> Thresholds {
        let metrics = Metrics::new(self.device.display.dpi);
        Thresholds {
            long_press_ms: self
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
                if self.with_top(|screen, cx| screen.on_gesture(gesture, cx)) == Handled::Yes {
                    return;
                }
                let display = self.device.display;
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
            self.keymap.resolve(&layers, &key).cloned()
        };
        if let Some(action) = action {
            self.dispatch(action);
        }
    }

    fn dispatch(&mut self, action: Id) {
        if self.with_top(|screen, cx| screen.on_action(&action, cx)) == Handled::Yes {
            return;
        }
        match self.registry.actions.get(&action) {
            Some(spec) => {
                if let Some(handler) = &spec.handler {
                    let env = env(
                        &self.device,
                        self.now,
                        &self.settings,
                        &self.translator,
                        &self.fonts,
                    );
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
        let Some(top) = self.stack.last_mut() else {
            return Handled::No;
        };
        let env = env(
            &self.device,
            self.now,
            &self.settings,
            &self.translator,
            &self.fonts,
        );
        let mut cx = Cx::new(&env, self.framebuffer.canvas(), &mut self.output);
        f(top.screen.as_mut(), &mut cx)
    }

    // ---- Commands and drawing ----------------------------------------

    fn finish(&mut self) -> Effects {
        self.run_commands();
        if mem::take(&mut self.output.redraw) {
            self.draw_top();
        }
        let requested = self.output.refresh.take();
        let mut effects = Effects::default();
        if let Some(dirty) = self.framebuffer.take_dirty() {
            let interval = self
                .settings
                .int(core_settings::FULL_REFRESH_INTERVAL.as_str())
                .clamp(0, i64::from(u32::MAX)) as u32;
            let screen = self.framebuffer.bounds();
            let requested = requested.unwrap_or(RefreshMode::Partial);
            let (mode, region) = self.refresh.decide(requested, dirty, screen, interval);
            effects.refresh = Some((region, mode));
        }
        effects.wake_at = self.recognizer.next_deadline(&self.thresholds());
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
            Command::OpenSlot(slot) => {
                let open = self
                    .stack
                    .iter()
                    .position(|s| s.slot.as_ref() == Some(&slot));
                if let Some(index) = open {
                    if index + 1 < self.stack.len() {
                        self.stack.truncate(index + 1);
                        self.screen_changed();
                    }
                    return;
                }
                match self.registry.resolve_slot(slot.as_str()).cloned() {
                    Some(screen) => self.open(screen, Some(slot)),
                    None => self.warn(
                        WarningSource::Runtime,
                        None,
                        format!("no screen for slot `{slot}`"),
                    ),
                }
            }
            Command::OpenScreen(screen) => self.open(screen, None),
            Command::Back => {
                if self.stack.len() > 1 {
                    self.stack.pop();
                    self.screen_changed();
                }
            }
            Command::SetSetting(id, value) => match self.settings.set(id.as_str(), value) {
                Ok(()) => self.setting_changed(&id),
                Err(error) => {
                    self.warn(WarningSource::Runtime, None, format!("`{id}`: {error}"));
                }
            },
            Command::Redraw(mode) => {
                self.output.redraw = true;
                self.output.request_refresh(mode);
            }
            _ => {}
        }
    }

    fn open(&mut self, id: Id, slot: Option<Id>) {
        let Some(spec) = self.registry.screens.get(&id) else {
            return self.warn(
                WarningSource::Runtime,
                None,
                format!("unknown screen `{id}`"),
            );
        };
        let env = env(
            &self.device,
            self.now,
            &self.settings,
            &self.translator,
            &self.fonts,
        );
        let screen = (spec.factory)(&env);
        self.stack.push(ActiveScreen { id, slot, screen });
        self.screen_changed();
    }

    fn screen_changed(&mut self) {
        self.captured.clear();
        self.output.redraw = true;
        self.output.request_refresh(RefreshMode::Full);
    }

    fn setting_changed(&mut self, id: &Id) {
        if *id == core_settings::LANGUAGE {
            self.translator
                .set_language(self.settings.text(id.as_str()));
        } else if *id == core_settings::NIGHT_MODE {
            self.framebuffer
                .set_inverted(self.settings.bool(id.as_str()));
        }
        self.output.redraw = true;
        self.output.request_refresh(RefreshMode::Full);
    }

    fn draw_top(&mut self) {
        let Some(top) = self.stack.last_mut() else {
            return;
        };
        let env = env(
            &self.device,
            self.now,
            &self.settings,
            &self.translator,
            &self.fonts,
        );
        let mut canvas = self.framebuffer.canvas();
        canvas.clear(Color::WHITE);
        top.screen.draw(&mut canvas, &env);
    }

    fn warn(&mut self, source: WarningSource, line: Option<u32>, message: String) {
        self.warnings.push(Warning {
            source,
            line,
            message,
        });
    }
}

/// Plugin ids switched off in the `[plugins]` section of a user config.
fn disabled_plugins(source: &str) -> Vec<String> {
    quireink_config::parse(source)
        .filter_map(Result::ok)
        .filter(|entry| entry.section == "plugins" && entry.value.as_bool() == Some(false))
        .map(|entry| String::from(entry.key))
        .collect()
}

/// The read-only environment handed to plugins.
fn env<'a>(
    device: &'a DeviceInfo,
    now: Instant,
    settings: &'a Settings,
    translator: &'a Translator,
    fonts: &'a Fonts,
) -> Env<'a> {
    Env {
        device,
        now,
        metrics: Metrics::new(device.display.dpi),
        settings,
        i18n: translator,
        fonts,
    }
}
