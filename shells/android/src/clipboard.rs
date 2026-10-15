//! The system's clipboard, through `ClipboardManager`.
//!
//! Android 12 and later tell the user when an app reads text another app
//! copied, so QuireInk only checks whether there is text when it comes to
//! the front, and reads it when the user asks to paste.

use android_activity::AndroidApp;
use jni::JNIEnv;
use jni::objects::{JObject, JString, JValue};

use crate::access::with_activity;

/// The clipboard manager. Its first use creates a `Handler`, which needs
/// a Java `Looper` on this (native) thread.
fn manager<'a>(env: &mut JNIEnv<'a>, activity: &JObject<'_>) -> jni::errors::Result<JObject<'a>> {
    crate::access::prepare_looper(env)?;
    let name = env.new_string("clipboard")?;
    env.call_method(
        activity,
        "getSystemService",
        "(Ljava/lang/String;)Ljava/lang/Object;",
        &[JValue::Object(&name)],
    )?
    .l()
}

/// Puts `text` on the clipboard.
pub fn write(app: &AndroidApp, text: &str) {
    let done = with_activity(app, |env, activity| {
        let manager = manager(env, activity)?;
        let label = env.new_string("QuireInk")?;
        let text = env.new_string(text)?;
        let clip = env
            .call_static_method(
                "android/content/ClipData",
                "newPlainText",
                "(Ljava/lang/CharSequence;Ljava/lang/CharSequence;)Landroid/content/ClipData;",
                &[JValue::Object(&label), JValue::Object(&text)],
            )?
            .l()?;
        env.call_method(
            &manager,
            "setPrimaryClip",
            "(Landroid/content/ClipData;)V",
            &[JValue::Object(&clip)],
        )?;
        Ok(())
    });
    if done.is_none() {
        log::warn!("could not copy to the clipboard");
    }
}

/// Whether the clipboard has something, without reading it.
pub fn has_text(app: &AndroidApp) -> bool {
    with_activity(app, |env, activity| {
        let manager = manager(env, activity)?;
        env.call_method(&manager, "hasPrimaryClip", "()Z", &[])?.z()
    })
    .unwrap_or(false)
}

/// The clipboard's text, if any.
pub fn read(app: &AndroidApp) -> Option<String> {
    with_activity(app, |env, activity| {
        let manager = manager(env, activity)?;
        let clip = env
            .call_method(
                &manager,
                "getPrimaryClip",
                "()Landroid/content/ClipData;",
                &[],
            )?
            .l()?;
        if clip.is_null() || env.call_method(&clip, "getItemCount", "()I", &[])?.i()? < 1 {
            return Ok(None);
        }
        let item = env
            .call_method(
                &clip,
                "getItemAt",
                "(I)Landroid/content/ClipData$Item;",
                &[JValue::Int(0)],
            )?
            .l()?;
        let text = env
            .call_method(
                &item,
                "coerceToText",
                "(Landroid/content/Context;)Ljava/lang/CharSequence;",
                &[JValue::Object(activity)],
            )?
            .l()?;
        if text.is_null() {
            return Ok(None);
        }
        let text = env
            .call_method(&text, "toString", "()Ljava/lang/String;", &[])?
            .l()?;
        let text: String = env.get_string(&JString::from(text))?.into();
        Ok(Some(text))
    })
    .flatten()
}
