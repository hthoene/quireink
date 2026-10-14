//! The native activity: event loop, window, input and storage.

use std::ffi::CString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant as StdInstant};

use android_activity::input::{InputEvent as AndroidInput, KeyAction, MotionAction, ToolType};
use android_activity::{AndroidApp, InputStatus, MainEvent, PollEvent};
use ndk::hardware_buffer_format::HardwareBufferFormat;
use ndk::native_window::NativeWindow;
use quireink_core::{Effects, Kernel};
use quireink_fs::DirStorage;
use quireink_graphics::Framebuffer;
use quireink_platform::{
    ButtonState, DeviceInfo, DisplayInfo, InputCaps, InputEvent, Instant, Mounted, PixelFormat,
    Point, PointerEvent, PointerPhase, PointerTool, RefreshMode, Rotation,
};

use crate::epd::Epd;
use crate::lights::Lights;
use crate::plugin::{AndroidPlugin, Frontlight};
use crate::{access, keys, pixels, status};

/// Where the device's shared storage appears in QuireInk's storage.
const SHARED: &str = "storage";
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
        let timeout = running
            .as_ref()
            .and_then(|r| r.wake_at)
            .map(|at| Duration::from_millis(at.since(now())));
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
                            present(window, &running.frame);
                        }
                    }
                }
                MainEvent::RedrawNeeded { .. } | MainEvent::WindowResized { .. } => {
                    if let (Some(window), Some(running)) = (&window, &running) {
                        present(window, &running.frame);
                    }
                }
                MainEvent::Resume { .. } => {
                    // The time zone or the battery may have changed.
                    if let Some(running) = running.as_mut() {
                        running.status_read = Instant(0);
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
        if state.wake_at.is_some_and(|at| at <= now()) {
            let effects = state.kernel.tick(now());
            changed |= apply(state, effects);
        }
        if changed && let Some(window) = &window {
            if let (Some(epd), Some(mode)) = (epd.as_mut(), state.refresh.take()) {
                epd.set(mode);
            }
            present(window, &state.frame);
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
                present(window, &running.frame);
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
        },
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
    let mut builder = Kernel::builder(device).plugin_dir("plugins");
    builder = match shared {
        Some(shared) => builder
            .storage(Mounted::new(storage, SHARED, shared))
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
    state.restart |= effects.restart();
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

/// Copies the whole frame into the window.
fn present(window: &NativeWindow, frame: &Framebuffer) {
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
    pixels::to_rgba(&frame.view(), 0, stride, |at, rgba| {
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
