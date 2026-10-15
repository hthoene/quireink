//! Access to the device's shared storage, where books live (by default in
//! `Books`, where Boox and most e-readers keep them; any folder can be
//! chosen as the library).
//!
//! Android 11 and later need "All files access" for that
//! (`MANAGE_EXTERNAL_STORAGE`, granted on a system settings page); older
//! versions need the storage permission. QuireInk never asks on its own: the
//! home screen offers it, and the app restarts with the shared storage when
//! the user comes back with access granted.

use std::path::PathBuf;

use android_activity::AndroidApp;
use jni::objects::{JObject, JObjectArray, JString, JValue};
use jni::{JNIEnv, JavaVM};

/// Android 11, from which "All files access" replaces the storage
/// permission.
const ANDROID_11: i32 = 30;
const READ_STORAGE: &str = "android.permission.READ_EXTERNAL_STORAGE";

/// Runs `f` with the JNI environment and the activity, clearing any Java
/// exception it leaves; `None` on failure. Local references `f` creates
/// are freed afterwards: the thread never returns to Java, so they would
/// pile up otherwise (e.g. reading the battery every minute).
pub(crate) fn with_activity<T>(
    app: &AndroidApp,
    f: impl FnOnce(&mut JNIEnv<'_>, &JObject<'_>) -> jni::errors::Result<T>,
) -> Option<T> {
    // SAFETY: android-activity provides the process's Java VM and a global
    // reference to the activity, both valid while the app runs; the
    // activity reference is borrowed, never deleted here.
    let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr().cast()) }.ok()?;
    let mut env = vm.attach_current_thread_permanently().ok()?;
    let activity = unsafe { JObject::from_raw(app.activity_as_ptr().cast()) };
    let result = env.with_local_frame(32, |env| f(env, &activity));
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_describe();
        let _ = env.exception_clear();
    }
    result.ok()
}

/// Prepares a Java `Looper` on this (native) thread, never run: some
/// system services create a `Handler` on first use, which needs one.
pub(crate) fn prepare_looper(env: &mut JNIEnv<'_>) -> jni::errors::Result<()> {
    let looper = env
        .call_static_method(
            "android/os/Looper",
            "myLooper",
            "()Landroid/os/Looper;",
            &[],
        )?
        .l()?;
    if looper.is_null() {
        env.call_static_method("android/os/Looper", "prepare", "()V", &[])?;
    }
    Ok(())
}

fn sdk_int(env: &mut JNIEnv<'_>) -> jni::errors::Result<i32> {
    env.get_static_field("android/os/Build$VERSION", "SDK_INT", "I")?
        .i()
}

/// Whether QuireInk may read the shared storage.
pub fn granted(app: &AndroidApp) -> bool {
    with_activity(app, |env, activity| {
        if sdk_int(env)? >= ANDROID_11 {
            env.call_static_method(
                "android/os/Environment",
                "isExternalStorageManager",
                "()Z",
                &[],
            )?
            .z()
        } else {
            let permission = env.new_string(READ_STORAGE)?;
            let result = env
                .call_method(
                    activity,
                    "checkSelfPermission",
                    "(Ljava/lang/String;)I",
                    &[JValue::Object(&permission)],
                )?
                .i()?;
            Ok(result == 0)
        }
    })
    .unwrap_or(false)
}

/// The shared storage, e.g. `/storage/emulated/0`.
pub fn storage_dir(app: &AndroidApp) -> Option<PathBuf> {
    with_activity(app, |env, _| {
        let dir = env
            .call_static_method(
                "android/os/Environment",
                "getExternalStorageDirectory",
                "()Ljava/io/File;",
                &[],
            )?
            .l()?;
        let path = env
            .call_method(&dir, "getAbsolutePath", "()Ljava/lang/String;", &[])?
            .l()?;
        let path: String = env.get_string(&path.into())?.into();
        Ok(PathBuf::from(path))
    })
}

/// The roots of removable volumes such as SD cards, e.g.
/// `/storage/1A2B-3C4D`. Android names a folder of the app's on every
/// mounted volume, the shared storage first; the root is above its
/// `Android` folder. Readable with the same access as the shared storage.
pub fn removable_dirs(app: &AndroidApp) -> Vec<PathBuf> {
    with_activity(app, |env, activity| {
        let dirs = env
            .call_method(
                activity,
                "getExternalFilesDirs",
                "(Ljava/lang/String;)[Ljava/io/File;",
                &[JValue::Object(&JObject::null())],
            )?
            .l()?;
        let dirs = JObjectArray::from(dirs);
        let mut roots = Vec::new();
        for i in 1..env.get_array_length(&dirs)? {
            let dir = env.get_object_array_element(&dirs, i)?;
            // A volume that is not mounted right now.
            if dir.is_null() {
                continue;
            }
            let path = env
                .call_method(&dir, "getAbsolutePath", "()Ljava/lang/String;", &[])?
                .l()?;
            let path: String = env.get_string(&JString::from(path))?.into();
            if let Some(end) = path.find("/Android/") {
                roots.push(PathBuf::from(&path[..end]));
            }
        }
        Ok(roots)
    })
    .unwrap_or_default()
}

/// Asks the user for access: opens the "All files access" page for
/// QuireInk (Android 11+) or shows the permission dialog.
pub fn request(app: &AndroidApp) {
    let opened = with_activity(app, |env, activity| {
        if sdk_int(env)? < ANDROID_11 {
            let class = env.find_class("java/lang/String")?;
            let permissions = env.new_object_array(1, class, JObject::null())?;
            let permission = env.new_string(READ_STORAGE)?;
            env.set_object_array_element(&permissions, 0, permission)?;
            env.call_method(
                activity,
                "requestPermissions",
                "([Ljava/lang/String;I)V",
                &[JValue::Object(&permissions), JValue::Int(1)],
            )?;
            return Ok(true);
        }
        let package = env
            .call_method(activity, "getPackageName", "()Ljava/lang/String;", &[])?
            .l()?;
        let package: String = env.get_string(&package.into())?.into();
        let uri_text = env.new_string(format!("package:{package}"))?;
        let uri = env
            .call_static_method(
                "android/net/Uri",
                "parse",
                "(Ljava/lang/String;)Landroid/net/Uri;",
                &[JValue::Object(&uri_text)],
            )?
            .l()?;
        let action = env.new_string("android.settings.MANAGE_APP_ALL_FILES_ACCESS_PERMISSION")?;
        let intent = env.new_object(
            "android/content/Intent",
            "(Ljava/lang/String;Landroid/net/Uri;)V",
            &[JValue::Object(&action), JValue::Object(&uri)],
        )?;
        env.call_method(
            activity,
            "startActivity",
            "(Landroid/content/Intent;)V",
            &[JValue::Object(&intent)],
        )?;
        Ok(true)
    });
    if opened.is_none() {
        // Some devices lack the per-app page: open the list of all apps.
        let fallback = with_activity(app, |env, activity| {
            let action = env.new_string("android.settings.MANAGE_ALL_FILES_ACCESS_PERMISSION")?;
            let intent = env.new_object(
                "android/content/Intent",
                "(Ljava/lang/String;)V",
                &[JValue::Object(&action)],
            )?;
            env.call_method(
                activity,
                "startActivity",
                "(Landroid/content/Intent;)V",
                &[JValue::Object(&intent)],
            )?;
            Ok(())
        });
        if fallback.is_none() {
            log::warn!("cannot open the storage access settings");
        }
    }
}
