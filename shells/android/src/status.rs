//! The time zone and the battery, read from Android for the kernel's status
//! (clock and battery in status lines).

use std::time::{SystemTime, UNIX_EPOCH};

use android_activity::AndroidApp;
use jni::JNIEnv;
use jni::objects::{JObject, JValue};
use quireink_platform::{Battery, Instant, SystemStatus, WallClock};

use crate::access::with_activity;

/// `BatteryManager.BATTERY_STATUS_CHARGING`.
const CHARGING: i32 = 2;

/// The current wall clock, time zone offset and battery.
pub fn read(app: &AndroidApp, now: Instant) -> SystemStatus {
    let unix_millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64);
    let offset = with_activity(app, |env, _| {
        let zone = env
            .call_static_method(
                "java/util/TimeZone",
                "getDefault",
                "()Ljava/util/TimeZone;",
                &[],
            )?
            .l()?;
        let millis = env
            .call_method(&zone, "getOffset", "(J)I", &[JValue::Long(unix_millis)])?
            .i()?;
        Ok(millis / 60_000)
    });
    SystemStatus {
        clock: Some(WallClock {
            at: now,
            unix_millis,
            utc_offset_minutes: offset,
        }),
        battery: with_activity(app, battery).flatten(),
    }
}

/// The battery from the sticky `ACTION_BATTERY_CHANGED` broadcast.
fn battery(env: &mut JNIEnv<'_>, activity: &JObject<'_>) -> jni::errors::Result<Option<Battery>> {
    let action = env.new_string("android.intent.action.BATTERY_CHANGED")?;
    let filter = env.new_object(
        "android/content/IntentFilter",
        "(Ljava/lang/String;)V",
        &[JValue::Object(&action)],
    )?;
    let intent = env
        .call_method(
            activity,
            "registerReceiver",
            "(Landroid/content/BroadcastReceiver;Landroid/content/IntentFilter;)Landroid/content/Intent;",
            &[JValue::Object(&JObject::null()), JValue::Object(&filter)],
        )?
        .l()?;
    if intent.is_null() {
        return Ok(None);
    }
    let mut extra = |name: &str| -> jni::errors::Result<i32> {
        let key = env.new_string(name)?;
        env.call_method(
            &intent,
            "getIntExtra",
            "(Ljava/lang/String;I)I",
            &[JValue::Object(&key), JValue::Int(-1)],
        )?
        .i()
    };
    let (level, scale, status) = (extra("level")?, extra("scale")?, extra("status")?);
    if level < 0 || scale <= 0 {
        return Ok(None);
    }
    Ok(Some(Battery {
        percent: (level * 100 / scale).clamp(0, 100) as u8,
        charging: status == CHARGING,
    }))
}
