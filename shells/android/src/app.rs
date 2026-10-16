//! The native activity: event loop, window, input and storage.

use std::ffi::CString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant as StdInstant};

use android_activity::input::{InputEvent as AndroidInput, KeyAction, MotionAction, ToolType};
use android_activity::{AndroidApp, InputStatus, MainEvent, PollEvent, WindowManagerFlags};
use ndk::hardware_buffer_format::HardwareBufferFormat;
use ndk::native_window::NativeWindow;
use quireink_core::{Effects, Kernel};
use quireink_fs::DirStorage;
use quireink_graphics::Framebuffer;
use quireink_platform::{
    ButtonState, DeviceInfo, DirEntry, DisplayInfo, InputCaps, InputEvent, Instant, Mounted,
    PixelFormat, Point, PointerEvent, PointerPhase, PointerTool, RefreshMode, Rotation, Storage,
    StorageError,
};

use crate::epd::Epd;
use crate::lights::Lights;
use crate::plugin::{AndroidPlugin, Frontlight};
use crate::{access, keys, pixels, status};

/// Where the device's shared storage appears in QuireInk's storage.
const SHARED: &str = "storage";
/// Where SD cards appear: the first as `SD card`, more numbered.
const REMOVABLE: &str = "SD card";
/// Data plugins on the shared storage, where users can copy them.
const PLUGINS: &str = "QuireInk/plugins";
/// Dictionaries on the shared storage.
const DICTIONARIES: &str = "QuireInk/dictionaries";
/// Where KOReader keeps dictionaries; used if it has any.
const KOREADER_DICTIONARIES: &str = "koreader/data/dict";
/// Fonts to read with: QuireInk's folder, KOReader's and the one Boox's
/// reader uses.
const FONTS: &str = "QuireInk/fonts";
const KOREADER_FONTS: &str = "koreader/fonts";
const SYSTEM_FONTS: &str = "fonts";

/// The running app once the window size is known.
struct Running {
    kernel: Kernel,
    /// The whole screen, as the window shows it.
    frame: Framebuffer,
    wake_at: Option<Instant>,
    /// The kernel asked to be built again (e.g. plugins were switched).
    restart: bool,
    /// The shared storage is mounted as `storage`.
    shared_storage: bool,
    /// The strongest refresh requested since the last frame was shown.
    refresh: Option<RefreshMode>,
    /// When the battery and time zone were read last.
    status_read: Instant,
    /// HTTP requests in flight.
    net: crate::net::Net,
    /// For the system's clipboard.
    app: AndroidApp,
    /// Plugins' servers.
    servers: quireink_serve::Servers,
    /// Plugins' sockets.
    sockets: quireink_serve::Sockets,
    /// Reading aloud.
    speech: crate::speech::Speech,
    /// The screen is kept on (while reading aloud).
    awake: bool,
    /// How the system was asked to turn the screen (see
    /// `Kernel::rotation`).
    rotation: Rotation,
}

/// How often the battery and time zone are read again.
const STATUS_EVERY_MS: u64 = 60_000;

#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("QuireInk"),
    );
    let start = StdInstant::now();
    let now = move || Instant(start.elapsed().as_millis() as u64);
    let mut epd = Epd::detect(&app);
    let lights = Lights::detect(&app);
    let mut running: Option<Running> = None;
    let mut window: Option<NativeWindow> = None;
    let mut quit = false;

    while !quit {
        let mut timeout = running
            .as_ref()
            .and_then(|r| r.wake_at)
            .map(|at| Duration::from_millis(at.since(now())));
        // Speech waiting for the engine looks again soon.
        if running.as_ref().is_some_and(|r| r.speech.waiting()) {
            let poll = crate::speech::POLL;
            timeout = Some(timeout.map_or(poll, |t| t.min(poll)));
        }
        app.poll_events(timeout, |event| {
            let PollEvent::Main(event) = event else {
                return;
            };
            match event {
                MainEvent::InitWindow { .. } => {
                    window = app.native_window();
                    if let Some(window) = &window {
                        if running.is_none() {
                            running = start_kernel(&app, window, lights, now());
                        }
                        if let Some(running) = &running {
                            // A new or returning window: start clean.
                            if let Some(epd) = epd.as_mut() {
                                epd.set(RefreshMode::Full);
                            }
                            present(window, &running.frame, running.kernel.colors());
                        }
                    }
                }
                MainEvent::RedrawNeeded { .. }
                | MainEvent::WindowResized { .. }
                | MainEvent::ConfigChanged { .. } => {
                    if let (Some(window), Some(running)) = (&window, running.as_mut()) {
                        // Turned: the same screens, made anew for the new size.
                        let size = (window.width(), window.height());
                        let frame = (
                            i32::from(running.frame.width()),
                            i32::from(running.frame.height()),
                        );
                        if size != frame
                            && let (Ok(width), Ok(height)) =
                                (u16::try_from(size.0), u16::try_from(size.1))
                            && let Ok(new) = Framebuffer::new(width, height, PixelFormat::Gray8)
                        {
                            running.frame = new;
                            let effects = running.kernel.resize(now(), width, height);
                            apply(running, effects);
                            if let Some(epd) = epd.as_mut() {
                                epd.set(RefreshMode::Full);
                            }
                        }
                        present(window, &running.frame, running.kernel.colors());
                    }
                }
                MainEvent::Resume { .. } => {
                    // The time zone or the battery may have changed.
                    if let Some(running) = running.as_mut() {
                        running.status_read = Instant(0);
                        let effects = running.kernel.resume(now());
                        apply(running, effects);
                    }
                    // Back from the settings page that grants storage
                    // access: restart with the shared storage.
                    if let Some(running) = running.as_mut()
                        && !running.shared_storage
                        && access::granted(&app)
                    {
                        running.restart = true;
                    }
                }
                MainEvent::Pause => {
                    // Other apps get the device's own refresh setting.
                    if let Some(epd) = epd.as_mut() {
                        epd.reset();
                    }
                    // Plugins save or send what is pending (e.g. progress).
                    if let Some(running) = running.as_mut() {
                        let effects = running.kernel.suspend(now());
                        apply(running, effects);
                    }
                }
                MainEvent::GainedFocus => {
                    // Maybe copied something in another app (not read
                    // until pasted).
                    if let Some(running) = running.as_mut() {
                        let available = crate::clipboard::has_text(&app);
                        running.kernel.set_clipboard_available(available);
                    }
                }
                MainEvent::TerminateWindow { .. } => window = None,
                MainEvent::Destroy => quit = true,
                _ => {}
            }
        });

        let Some(state) = running.as_mut() else {
            continue;
        };
        let mut changed = false;
        if let Ok(mut events) = app.input_events_iter() {
            while events.next(|event| {
                let handled = input(event)
                    .into_iter()
                    .flatten()
                    .map(|event| {
                        let effects = state.kernel.handle(now(), event);
                        changed |= apply(state, effects);
                    })
                    .count()
                    > 0;
                if handled {
                    InputStatus::Handled
                } else {
                    InputStatus::Unhandled
                }
            }) {}
        }
        if now().since(state.status_read) >= STATUS_EVERY_MS || state.status_read == Instant(0) {
            let at = now();
            state.status_read = at;
            let effects = state.kernel.set_status(at, status::read(&app, at));
            changed |= apply(state, effects);
        }
        while let Some(response) = state.net.next() {
            let effects = state.kernel.http_response(now(), response);
            changed |= apply(state, effects);
        }
        while let Some(event) = state.servers.next() {
            let effects = state.kernel.server_event(now(), event);
            changed |= apply(state, effects);
        }
        while let Some(event) = state.sockets.take_event() {
            let effects = state.kernel.socket_event(now(), event);
            changed |= apply(state, effects);
        }
        let kernel = &state.kernel;
        state
            .servers
            .pump(|path, at, buffer| kernel.read_served(path, at, buffer).ok());
        if state.kernel.wants_clipboard() {
            let text = crate::clipboard::read(&app);
            let effects = state.kernel.clipboard_text(now(), text);
            changed |= apply(state, effects);
        }
        for (id, spoken) in state.speech.poll() {
            let effects = state.kernel.speech_done(now(), id, spoken);
            changed |= apply(state, effects);
        }
        if state.wake_at.is_some_and(|at| at <= now()) {
            // What changed shows before the timer runs: it may take a
            // while (e.g. reading a folder on an SD card) and say so first.
            if changed && let Some(window) = &window {
                show(window, epd.as_mut(), state);
                changed = false;
            }
            let effects = state.kernel.tick(now());
            changed |= apply(state, effects);
        }
        // The screen stays on while reading aloud: a sleeping device would
        // stop after the sentence.
        let speaking = state.kernel.speaking();
        if speaking != state.awake {
            state.awake = speaking;
            let (on, none) = (
                WindowManagerFlags::KEEP_SCREEN_ON,
                WindowManagerFlags::empty(),
            );
            if speaking {
                app.set_window_flags(on, none);
            } else {
                app.set_window_flags(none, on);
            }
        }
        if changed && let Some(window) = &window {
            show(window, epd.as_mut(), state);
        }
        for warning in state.kernel.take_warnings() {
            log::warn!("{warning}");
        }
        if state.restart
            && let Some(window) = &window
        {
            log::info!("restarting");
            // The old kernel goes first: its files are closed and its
            // memory is free before the new one starts.
            drop(running.take());
            running = start_kernel(&app, window, lights, now());
            if let Some(running) = &running {
                if let Some(epd) = epd.as_mut() {
                    epd.set(RefreshMode::Full);
                }
                present(window, &running.frame, running.kernel.colors());
            }
        }
    }
}

/// Builds the kernel for the window's size.
fn start_kernel(
    app: &AndroidApp,
    window: &NativeWindow,
    lights: Option<Lights>,
    now: Instant,
) -> Option<Running> {
    let (width, height) = (window.width(), window.height());
    let dpi = app.config().density().unwrap_or(160);
    let device = DeviceInfo {
        id: "android",
        name: "Android e-reader",
        display: DisplayInfo {
            width: u16::try_from(width).ok()?,
            height: u16::try_from(height).ok()?,
            dpi: u16::try_from(dpi).unwrap_or(160),
            format: PixelFormat::Gray8,
            rotation: Rotation::Deg0,
            color: None,
        },
        input: InputCaps {
            buttons: true,
            touch: true,
            pen: false,
            // The page buttons (or volume keys) and back.
            keys: &[
                quireink_platform::buttons::PAGE_FORWARD,
                quireink_platform::buttons::PAGE_BACK,
                quireink_platform::buttons::BACK,
            ],
        },
        memory: quireink_platform::MemoryClass::Large,
    };
    let root = app
        .external_data_path()
        .or_else(|| app.internal_data_path())
        .unwrap_or_else(|| PathBuf::from("."));
    install_samples(app, &root);
    let storage = match DirStorage::new(&root) {
        Ok(storage) => storage,
        Err(e) => {
            log::error!("storage {}: {e}", root.display());
            return None;
        }
    };
    // The shared storage as `storage`, if the user allowed access; the
    // library is then its `Books` folder unless the user chose another.
    let shared = access::granted(app)
        .then(|| access::storage_dir(app))
        .flatten()
        .and_then(|dir| {
            // Data plugins and dictionaries are copied there.
            for folder in [PLUGINS, DICTIONARIES, FONTS] {
                if let Err(e) = std::fs::create_dir_all(dir.join(folder)) {
                    log::warn!("{}: {e}", dir.join(folder).display());
                }
            }
            match DirStorage::new(&dir) {
                Ok(shared) => Some(shared),
                Err(e) => {
                    log::warn!("{}: {e}", dir.display());
                    None
                }
            }
        });
    // KOReader's dictionaries if there are any, else QuireInk's folder.
    let dictionaries = access::storage_dir(app)
        .filter(|dir| shared.is_some() && dir.join(KOREADER_DICTIONARIES).is_dir())
        .map_or(DICTIONARIES, |_| KOREADER_DICTIONARIES);
    let shared_storage = shared.is_some();
    // SD cards next to the shared storage, with the same access.
    let removable: Vec<(&'static str, DirStorage)> = if shared_storage {
        access::removable_dirs(app)
            .into_iter()
            .filter_map(|dir| match DirStorage::new(&dir) {
                Ok(volume) => Some(volume),
                Err(e) => {
                    log::warn!("{}: {e}", dir.display());
                    None
                }
            })
            .enumerate()
            .map(|(n, volume)| {
                let name = match n {
                    0 => REMOVABLE.to_string(),
                    n => format!("{REMOVABLE} {}", n + 1),
                };
                // Once per start: the mount points live as long as the app.
                (&*Box::leak(name.into_boxed_str()), volume)
            })
            .collect()
    } else {
        Vec::new()
    };
    let mut builder = Kernel::builder(device)
        .plugin_dir("plugins")
        .network(true)
        .system_clipboard(true)
        .speech(true)
        .rotation(true)
        .colors(true);
    builder = match shared {
        Some(shared) => builder
            .storage(removable.into_iter().fold(
                Volumes(Box::new(Mounted::new(storage, SHARED, shared))),
                |all, (name, volume)| Volumes(Box::new(Mounted::new(all, name, volume))),
            ))
            .plugin_dir(format!("{SHARED}/{PLUGINS}"))
            .font_dir(format!("{SHARED}/{FONTS}"))
            .font_dir(format!("{SHARED}/{KOREADER_FONTS}"))
            .font_dir(format!("{SHARED}/{SYSTEM_FONTS}"))
            .setting_default("files.root", &format!("{SHARED}/Books"))
            .setting_default("dictionary.folder", &format!("{SHARED}/{dictionaries}")),
        None => builder.storage(storage),
    };
    // The system language, unless the user chose another one.
    if let Some(language) = app.config().language() {
        builder = builder.setting_default("core.language", &language);
    }
    let builder =
        quireink_app::install(builder, quireink_app::Fonts::Outline).plugin(AndroidPlugin {
            app: app.clone(),
            shared_storage,
            screen: (
                u32::from(device.display.width),
                u32::from(device.display.height),
            ),
            frontlight: lights.map(|lights| Frontlight {
                lights,
                brightness: lights.brightness(app),
                warmth: lights.warmth(app),
            }),
        });
    let mut kernel = match builder.build() {
        Ok(kernel) => kernel,
        Err(e) => {
            log::error!("cannot start: {e}");
            return None;
        }
    };
    let frame = Framebuffer::new(
        device.display.width,
        device.display.height,
        PixelFormat::Gray8,
    )
    .ok()?;
    let _ = kernel.set_status(now, status::read(app, now));
    let effects = kernel.start(now);
    let mut running = Running {
        kernel,
        frame,
        wake_at: None,
        restart: false,
        shared_storage,
        refresh: None,
        status_read: now,
        net: crate::net::Net::new(app),
        app: app.clone(),
        servers: {
            let waker = app.create_waker();
            quireink_serve::Servers::new(move || waker.wake())
        },
        sockets: {
            let waker = app.create_waker();
            quireink_serve::Sockets::new(move || waker.wake())
        },
        speech: crate::speech::Speech::new(app),
        awake: false,
        rotation: Rotation::Deg0,
    };
    apply(&mut running, effects);
    Some(running)
}

/// Copies the sample books from the APK on first start, so there is
/// something to read.
fn install_samples(app: &AndroidApp, root: &Path) {
    let books = root.join("books");
    let has_books = std::fs::read_dir(&books).is_ok_and(|mut entries| entries.next().is_some());
    if has_books {
        return;
    }
    if let Err(e) = std::fs::create_dir_all(&books) {
        log::warn!("{}: {e}", books.display());
        return;
    }
    let assets = app.asset_manager();
    let Some(dir) = assets.open_dir(c"books") else {
        return;
    };
    for name in dir {
        let Ok(text) = name.to_str() else {
            continue;
        };
        let Ok(path) = CString::new(format!("books/{text}")) else {
            continue;
        };
        let Some(mut asset) = assets.open(&path) else {
            continue;
        };
        let mut data = Vec::new();
        if asset.read_to_end(&mut data).is_ok()
            && let Err(e) = std::fs::write(books.join(text), data)
        {
            log::warn!("{text}: {e}");
        }
    }
}

/// Executes the effects of a kernel call; `true` if the screen changed.
fn apply(state: &mut Running, effects: Effects) -> bool {
    state.wake_at = effects.wake_at();
    for call in state.kernel.take_requests() {
        state.net.start(call);
    }
    if let Some(text) = state.kernel.take_clipboard() {
        crate::clipboard::write(&state.app, &text);
    }
    for call in state.kernel.take_server_calls() {
        state.servers.apply(call);
    }
    for call in state.kernel.take_socket_calls() {
        state.sockets.apply(call);
    }
    for call in state.kernel.take_speech() {
        state.speech.apply(call);
    }
    state.restart |= effects.restart();
    // The system turns the window; its new size comes as an event.
    let rotation = state.kernel.rotation();
    if rotation != state.rotation {
        state.rotation = rotation;
        turn(&state.app, rotation);
    }
    if let Some((_, mode)) = effects.refresh() {
        state.refresh = Some(state.refresh.map_or(mode, |m| m.max(mode)));
    }
    if effects.refresh().is_none() {
        return false;
    }
    let frame = &mut state.frame;
    state
        .kernel
        .render(&mut |top, rows| frame.write_rows(top, rows));
    true
}

/// Asks the system to turn the screen.
fn turn(app: &AndroidApp, rotation: Rotation) {
    // `ActivityInfo.SCREEN_ORIENTATION_*`.
    let orientation = match rotation {
        Rotation::Deg0 => 1,
        Rotation::Deg90 => 0,
        Rotation::Deg180 => 9,
        Rotation::Deg270 => 8,
    };
    let turned = access::with_activity(app, |env, activity| {
        env.call_method(
            activity,
            "setRequestedOrientation",
            "(I)V",
            &[jni::objects::JValue::Int(orientation)],
        )?;
        Ok(())
    });
    if turned.is_none() {
        log::warn!("cannot turn the screen");
    }
}

/// Shows the frame, with the e-ink refresh the kernel asked for.
fn show(window: &NativeWindow, epd: Option<&mut Epd>, state: &mut Running) {
    if let (Some(epd), Some(mode)) = (epd, state.refresh.take()) {
        epd.set(mode);
    }
    present(window, &state.frame, state.kernel.colors());
}

/// Copies the whole frame into the window, in the colours of the
/// settings.
fn present(window: &NativeWindow, frame: &Framebuffer, colors: [[u8; 3]; 2]) {
    let (width, height) = (i32::from(frame.width()), i32::from(frame.height()));
    if let Err(e) =
        window.set_buffers_geometry(width, height, Some(HardwareBufferFormat::R8G8B8A8_UNORM))
    {
        log::warn!("window geometry: {e}");
        return;
    }
    let Ok(mut buffer) = window.lock(None) else {
        return;
    };
    let stride = buffer.stride();
    let Some(bytes) = buffer.bytes() else {
        return;
    };
    let tint = pixels::tint(colors);
    pixels::to_rgba(&frame.view(), 0, stride, &tint, |at, rgba| {
        if let Some(pixel) = bytes.get_mut(at..at + 4) {
            for (slot, value) in pixel.iter_mut().zip(rgba) {
                slot.write(value);
            }
        }
    });
}

/// Translates an Android input event; `None` leaves it to Android.
fn input(event: &AndroidInput<'_>) -> Option<Vec<InputEvent>> {
    match event {
        AndroidInput::KeyEvent(key) => {
            let name = keys::button(u32::from(key.key_code()))?;
            let state = match key.action() {
                KeyAction::Down if key.repeat_count() == 0 => ButtonState::Pressed,
                // Held keys repeat; the kernel measures long presses itself.
                KeyAction::Down => return Some(Vec::new()),
                KeyAction::Up => ButtonState::Released,
                _ => return Some(Vec::new()),
            };
            Some(vec![InputEvent::Button { name, state }])
        }
        AndroidInput::MotionEvent(motion) => {
            let phase = match motion.action() {
                MotionAction::Down | MotionAction::PointerDown => PointerPhase::Down,
                MotionAction::Up | MotionAction::PointerUp => PointerPhase::Up,
                MotionAction::Move => PointerPhase::Move,
                MotionAction::Cancel => PointerPhase::Cancel,
                _ => return Some(Vec::new()),
            };
            // Down and up concern one pointer; moves report all of them.
            let pointers: Vec<_> = match phase {
                PointerPhase::Move => motion.pointers().collect(),
                _ => vec![motion.pointer_at_index(motion.pointer_index())],
            };
            Some(
                pointers
                    .into_iter()
                    .map(|pointer| {
                        InputEvent::Pointer(PointerEvent {
                            id: pointer.pointer_id().clamp(0, 255) as u8,
                            phase,
                            point: Point::new(pointer.x() as i32, pointer.y() as i32),
                            tool: match pointer.tool_type() {
                                ToolType::Stylus => PointerTool::Pen,
                                ToolType::Eraser => PointerTool::Eraser,
                                ToolType::Mouse => PointerTool::Mouse,
                                _ => PointerTool::Finger,
                            },
                            pressure: (pointer.pressure().clamp(0.0, 1.0) * 1000.0) as u16,
                        })
                    })
                    .collect(),
            )
        }
        _ => None,
    }
}

/// Storage with any number of mount points (one per SD card).
struct Volumes(Box<dyn Storage>);

impl Storage for Volumes {
    fn read_at(&self, path: &str, offset: u64, buf: &mut [u8]) -> Result<usize, StorageError> {
        self.0.read_at(path, offset, buf)
    }

    fn size(&self, path: &str) -> Result<u64, StorageError> {
        self.0.size(path)
    }

    fn write(&self, path: &str, data: &[u8]) -> Result<(), StorageError> {
        self.0.write(path, data)
    }

    fn append(&self, path: &str, data: &[u8]) -> Result<(), StorageError> {
        self.0.append(path, data)
    }

    fn list(&self, dir: &str, visit: &mut dyn FnMut(DirEntry<'_>)) -> Result<(), StorageError> {
        self.0.list(dir, visit)
    }

    fn remove(&self, path: &str) -> Result<(), StorageError> {
        self.0.remove(path)
    }

    fn exists(&self, path: &str) -> bool {
        self.0.exists(path)
    }

    fn rename(&self, from: &str, to: &str) -> Result<(), StorageError> {
        self.0.rename(from, to)
    }

    fn create_dir(&self, path: &str) -> Result<(), StorageError> {
        self.0.create_dir(path)
    }

    fn remove_dir_all(&self, path: &str) -> Result<(), StorageError> {
        self.0.remove_dir_all(path)
    }

    fn local_path(&self, path: &str, with: &mut dyn FnMut(&str)) {
        self.0.local_path(path, with);
    }
}
