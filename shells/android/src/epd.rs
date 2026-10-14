//! E-ink refresh modes on Onyx Boox devices.
//!
//! Onyx adds hidden methods to `android.view.View`; on current firmware
//! (e.g. the Go 7 with Android 13) `setDefaultUpdateMode(int)` chooses the
//! waveform the system uses for the window's next updates and
//! `resetUpdateMode()` restores the user's setting. QuireInk sets the mode
//! before each frame it shows, so the system refreshes once, with the right
//! waveform:
//!
//! | QuireInk | Onyx waveform |
//! |----------|---------------|
//! | `Full` (every few pages, new screens) | GC16, full, flashing: clears ghosting |
//! | `Partial` (page turns) | GC16 regional: clean grays, no flash |
//! | `Fast` (focus moves, switches) | DU: fastest, black and white |
//!
//! On other devices, or if the methods are missing, nothing changes and
//! the system refreshes as before. Values as documented by KOReader.

use android_activity::AndroidApp;
use jni::objects::{GlobalRef, JMethodID, JValue};
use jni::signature::{Primitive, ReturnType};
use jni::{JNIEnv, JavaVM};
use quireink_platform::RefreshMode;

/// Full update (32) + wait (64) + GC16 (2).
const FULL: i32 = 98;
/// Regional update with GC16.
const PARTIAL: i32 = 2;
/// Regional update with DU.
const FAST: i32 = 1;

/// The Onyx e-ink controller of the activity's window.
pub struct Epd {
    vm: JavaVM,
    view: GlobalRef,
    set_mode: JMethodID,
    reset: JMethodID,
    /// The mode set last; `None` after a reset.
    current: Option<RefreshMode>,
}

impl Epd {
    /// The controller, on Onyx devices whose firmware has the methods.
    pub fn detect(app: &AndroidApp) -> Option<Self> {
        // SAFETY: android-activity provides the process's Java VM and a
        // global reference to the activity, valid while the app runs; the
        // activity reference is borrowed, never deleted here.
        let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr().cast()) }.ok()?;
        let activity = unsafe { jni::objects::JObject::from_raw(app.activity_as_ptr().cast()) };
        let found = {
            let mut env = vm.attach_current_thread_permanently().ok()?;
            let result = find(&mut env, &activity);
            clear_exception(&mut env);
            result
        };
        let (view, set_mode, reset) = found?;
        log::info!("Onyx e-ink modes available");
        Some(Self {
            vm,
            view,
            set_mode,
            reset,
            current: None,
        })
    }

    /// Chooses the waveform for the next frame.
    pub fn set(&mut self, mode: RefreshMode) {
        if self.current == Some(mode) {
            return;
        }
        let code = match mode {
            RefreshMode::Full => FULL,
            RefreshMode::Partial => PARTIAL,
            RefreshMode::Fast => FAST,
        };
        if self.call(self.set_mode, &[JValue::Int(code).as_jni()]) {
            self.current = Some(mode);
            log::debug!("e-ink mode {code} ({:?})", self.device_mode());
        } else {
            log::warn!("e-ink mode {code} not accepted");
        }
    }

    /// The mode the device reports for the window.
    fn device_mode(&self) -> Option<i32> {
        let mut env = self.vm.attach_current_thread_permanently().ok()?;
        let mode = env
            .call_method(&self.view, "getDefaultUpdateMode", "()I", &[])
            .and_then(|v| v.i());
        clear_exception(&mut env);
        mode.ok()
    }

    /// Gives the device back its own refresh setting, e.g. when the app is
    /// paused.
    pub fn reset(&mut self) {
        self.call(self.reset, &[]);
        self.current = None;
    }

    fn call(&self, method: JMethodID, args: &[jni::sys::jvalue]) -> bool {
        let Ok(mut env) = self.vm.attach_current_thread_permanently() else {
            return false;
        };
        // SAFETY: `method` was looked up on `android.view.View` with
        // exactly these argument types and a void return type.
        let result = unsafe {
            env.call_method_unchecked(
                &self.view,
                method,
                ReturnType::Primitive(Primitive::Void),
                args,
            )
        };
        clear_exception(&mut env);
        result.is_ok()
    }
}

/// The window's view and the Onyx methods, if all exist.
fn find(
    env: &mut JNIEnv<'_>,
    activity: &jni::objects::JObject<'_>,
) -> Option<(GlobalRef, JMethodID, JMethodID)> {
    let manufacturer = env
        .get_static_field("android/os/Build", "MANUFACTURER", "Ljava/lang/String;")
        .ok()?
        .l()
        .ok()?;
    let manufacturer: String = env.get_string(&manufacturer.into()).ok()?.into();
    if !manufacturer.eq_ignore_ascii_case("onyx") {
        return None;
    }
    let class = env.find_class("android/view/View").ok()?;
    let set_mode = env
        .get_method_id(&class, "setDefaultUpdateMode", "(I)V")
        .ok()?;
    let reset = env.get_method_id(&class, "resetUpdateMode", "()V").ok()?;
    let window = env
        .call_method(activity, "getWindow", "()Landroid/view/Window;", &[])
        .ok()?
        .l()
        .ok()?;
    let view = env
        .call_method(&window, "getDecorView", "()Landroid/view/View;", &[])
        .ok()?
        .l()
        .ok()?;
    let view = env.new_global_ref(view).ok()?;
    Some((view, set_mode, reset))
}

fn clear_exception(env: &mut JNIEnv<'_>) {
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_clear();
    }
}
