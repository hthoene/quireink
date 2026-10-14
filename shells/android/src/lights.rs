//! The frontlight of Onyx Boox devices: brightness and, where the panel has
//! warm and cold LEDs, colour temperature.
//!
//! Onyx's system class `android.onyx.hardware.DeviceController` has static
//! methods for this. Devices differ in how the light is driven (detection
//! as in KOReader):
//!
//! | Kind | Brightness | Warmth | Switch |
//! |------|------------|--------|--------|
//! | CTM (e.g. Go 7) | type 7 | type 6 | type 4 |
//! | Warm and cold LEDs | type 3 (cold) | type 2 (warm) | per type |
//! | Single light | type 1 | none | type 1 |

use android_activity::AndroidApp;
use jni::JNIEnv;
use jni::objects::JValue;

use crate::access::with_activity;

const CONTROLLER: &str = "android/onyx/hardware/DeviceController";

/// How the device's light is driven.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lights {
    brightness: i32,
    warmth: Option<i32>,
    /// The channel opened before and closed after: `None` when each type
    /// is its own channel.
    switch: Option<i32>,
    pub max_brightness: i32,
    pub max_warmth: i32,
}

impl Lights {
    /// The device's light, if it has one QuireInk can drive.
    pub fn detect(app: &AndroidApp) -> Option<Self> {
        with_activity(app, |env, activity| {
            let manufacturer = env
                .get_static_field("android/os/Build", "MANUFACTURER", "Ljava/lang/String;")?
                .l()?;
            let manufacturer: String = env.get_string(&manufacturer.into())?.into();
            if !manufacturer.eq_ignore_ascii_case("onyx") {
                return Ok(None);
            }
            let ctm = env
                .call_static_method(CONTROLLER, "checkCTM", "()Z", &[])?
                .z()?;
            let has = |env: &mut JNIEnv<'_>, name: &str| -> jni::errors::Result<bool> {
                env.call_static_method(
                    CONTROLLER,
                    name,
                    "(Landroid/content/Context;)Z",
                    &[JValue::Object(activity)],
                )?
                .z()
            };
            let (brightness, warmth, switch) = if ctm || max(env, 6)? > 0 {
                (7, Some(6), Some(4))
            } else if has(env, "hasFLBrightness")? {
                (1, None, None)
            } else if has(env, "hasCTMBrightness")? || max(env, 2)? > 0 {
                (3, Some(2), None)
            } else {
                return Ok(None);
            };
            let max_brightness = max(env, brightness)?;
            let max_warmth = match warmth {
                Some(t) => max(env, t)?,
                None => 0,
            };
            Ok((max_brightness > 0).then_some(Lights {
                brightness,
                warmth: warmth.filter(|_| max_warmth > 0),
                switch,
                max_brightness,
                max_warmth,
            }))
        })
        .flatten()
    }

    pub fn has_warmth(&self) -> bool {
        self.warmth.is_some()
    }

    /// The current brightness, from 0 to `max_brightness`.
    pub fn brightness(&self, app: &AndroidApp) -> i32 {
        with_activity(app, |env, _| value(env, self.brightness)).unwrap_or(0)
    }

    /// The current warmth, from 0 to `max_warmth`.
    pub fn warmth(&self, app: &AndroidApp) -> i32 {
        self.warmth
            .and_then(|t| with_activity(app, |env, _| value(env, t)))
            .unwrap_or(0)
    }

    /// Sets the brightness; 0 switches the light off.
    pub fn set_brightness(&self, app: &AndroidApp, level: i32) {
        self.set(
            app,
            self.brightness,
            level.clamp(0, self.max_brightness),
            true,
        );
    }

    /// Sets the colour temperature (higher is warmer).
    pub fn set_warmth(&self, app: &AndroidApp, level: i32) {
        if let Some(t) = self.warmth {
            self.set(app, t, level.clamp(0, self.max_warmth), false);
        }
    }

    /// Writes `level` to light `kind`, switching its channel on first, or
    /// off for brightness 0.
    fn set(&self, app: &AndroidApp, kind: i32, level: i32, closes: bool) {
        let channel = self.switch.unwrap_or(kind);
        let done = with_activity(app, |env, _| {
            if level == 0 {
                if closes {
                    call_bool(env, "closeFrontLight", channel)?;
                }
            } else if !call_bool(env, "isLightOn", channel)? {
                call_bool(env, "openFrontLight", channel)?;
            }
            env.call_static_method(
                CONTROLLER,
                "setLightValue",
                "(II)V",
                &[JValue::Int(kind), JValue::Int(level)],
            )?;
            Ok(())
        });
        if done.is_none() {
            log::warn!("frontlight: cannot set type {kind} to {level}");
        }
    }
}

fn max(env: &mut JNIEnv<'_>, kind: i32) -> jni::errors::Result<i32> {
    env.call_static_method(CONTROLLER, "getMaxLightValue", "(I)I", &[JValue::Int(kind)])?
        .i()
}

fn value(env: &mut JNIEnv<'_>, kind: i32) -> jni::errors::Result<i32> {
    env.call_static_method(CONTROLLER, "getLightValue", "(I)I", &[JValue::Int(kind)])?
        .i()
}

fn call_bool(env: &mut JNIEnv<'_>, name: &str, channel: i32) -> jni::errors::Result<bool> {
    env.call_static_method(CONTROLLER, name, "(I)Z", &[JValue::Int(channel)])?
        .z()
}
