//! Reading aloud through the system's speech output (`TextToSpeech`).
//!
//! The engine is bound at the first text, not at start-up, and answers
//! asynchronously: texts that come before it is ready wait for it a
//! while. Texts handed over while one is spoken are queued, so they follow
//! without a pause. The engine reports the end of each text to
//! `SpeechListener` (QuireInk's only Java class), which calls [`ended`]
//! on one of its threads; the event loop is woken to pass it on.

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use android_activity::{AndroidApp, AndroidAppWaker};
use jni::JNIEnv;
use jni::objects::{GlobalRef, JClass, JObject, JString, JValue};
use jni::sys::jboolean;
use quireink_core::SpeechCall;

use crate::access::with_activity;

/// How often a text waiting for the engine to be ready looks again.
pub const POLL: Duration = Duration::from_millis(100);
/// How long a text waits for the engine to be ready.
const READY_WAIT: Duration = Duration::from_secs(8);
/// The most characters spoken at once (engines take at least 4000).
const MAX_CHARS: usize = 3_900;
/// `TextToSpeech.QUEUE_FLUSH` and `QUEUE_ADD`.
const QUEUE_FLUSH: i32 = 0;
const QUEUE_ADD: i32 = 1;
/// The listener class, in the APK's `classes.dex`.
const LISTENER: &str = "io.github.hthoene.quireink.SpeechListener";

/// Texts the engine reported as ended (spoken or not), from its threads.
static ENDED: Mutex<Vec<(u32, bool)>> = Mutex::new(Vec::new());
/// Wakes the event loop when a text ended.
static WAKER: OnceLock<AndroidAppWaker> = OnceLock::new();

/// Called by `SpeechListener` when a text ended.
extern "system" fn ended(mut env: JNIEnv<'_>, _: JClass<'_>, id: JString<'_>, spoken: jboolean) {
    let id: Option<u32> = env
        .get_string(&id)
        .ok()
        .and_then(|id| String::from(id).parse().ok());
    if let (Some(id), Ok(mut ended)) = (id, ENDED.lock()) {
        ended.push((id, spoken != 0));
    }
    if let Some(waker) = WAKER.get() {
        waker.wake();
    }
}

/// A text for the engine.
struct Text {
    id: u32,
    text: String,
    language: String,
    rate: u16,
    queue: bool,
    since: Instant,
}

/// The system's speech output.
pub struct Speech {
    app: AndroidApp,
    /// The engine, once bound.
    tts: Option<GlobalRef>,
    /// Texts waiting for the engine to be ready.
    pending: VecDeque<Text>,
    /// Texts that ended here (failed, or nothing to say).
    ended: Vec<(u32, bool)>,
    /// The language and speed the engine was last set to.
    voice: Option<(String, u16)>,
}

impl Speech {
    pub fn new(app: &AndroidApp) -> Self {
        let _ = WAKER.set(app.create_waker());
        Self {
            app: app.clone(),
            tts: None,
            pending: VecDeque::new(),
            ended: Vec::new(),
            voice: None,
        }
    }

    /// Whether texts wait for the engine: it is to be looked at again
    /// (see [`POLL`]).
    pub fn waiting(&self) -> bool {
        !self.pending.is_empty() || !self.ended.is_empty()
    }

    /// Does what the kernel asks.
    pub fn apply(&mut self, call: SpeechCall) {
        match call {
            SpeechCall::Say {
                id,
                text,
                language,
                rate,
                queue,
            } => {
                if !queue {
                    self.stop();
                }
                if self.tts.is_none() {
                    self.tts = self.create();
                }
                self.pending.push_back(Text {
                    id,
                    text,
                    language,
                    rate,
                    queue,
                    since: Instant::now(),
                });
                self.start_pending();
            }
            SpeechCall::Stop => self.stop(),
        }
    }

    /// Texts that ended since the last call, with whether they were
    /// spoken.
    pub fn poll(&mut self) -> Vec<(u32, bool)> {
        self.start_pending();
        let mut ended = std::mem::take(&mut self.ended);
        if let Ok(mut reported) = ENDED.lock() {
            ended.append(&mut reported);
        }
        ended
    }

    /// Hands the waiting texts to the engine once it is ready.
    fn start_pending(&mut self) {
        let Some(tts) = self.tts.clone() else {
            // Not even bound: what waits fails.
            self.ended
                .extend(self.pending.drain(..).map(|text| (text.id, false)));
            return;
        };
        while let Some(text) = self.pending.pop_front() {
            let voice = self.voice.clone();
            let started =
                with_activity(&self.app, |env, _| start(env, tts.as_obj(), &text, &voice));
            match started {
                Some(Started::Yes(voice)) => self.voice = Some(voice),
                Some(Started::NotReady) if text.since.elapsed() < READY_WAIT => {
                    self.pending.push_front(text);
                    return;
                }
                _ => self.ended.push((text.id, false)),
            }
        }
    }

    /// Stops speaking, forgetting the texts.
    fn stop(&mut self) {
        self.pending.clear();
        if let Some(tts) = &self.tts {
            with_activity(&self.app, |env, _| {
                env.call_method(tts.as_obj(), "stop", "()I", &[])?.i()
            });
        }
        // Reports of texts cut off are of no use any more.
        self.ended.clear();
        if let Ok(mut reported) = ENDED.lock() {
            reported.clear();
        }
    }

    /// Binds the engine (it becomes ready a little later) and gives it
    /// the listener.
    fn create(&self) -> Option<GlobalRef> {
        let tts = with_activity(&self.app, |env, activity| {
            crate::access::prepare_looper(env)?;
            let tts = env.new_object(
                "android/speech/tts/TextToSpeech",
                "(Landroid/content/Context;Landroid/speech/tts/TextToSpeech$OnInitListener;)V",
                &[JValue::Object(activity), JValue::Object(&JObject::null())],
            )?;
            let listener = listener(env, activity)?;
            env.call_method(
                &tts,
                "setOnUtteranceProgressListener",
                "(Landroid/speech/tts/UtteranceProgressListener;)I",
                &[JValue::Object(&listener)],
            )?;
            env.new_global_ref(tts)
        });
        if tts.is_none() {
            log::warn!("no speech output");
        }
        tts
    }
}

impl Drop for Speech {
    fn drop(&mut self) {
        if let Some(tts) = self.tts.take() {
            with_activity(&self.app, |env, _| {
                env.call_method(tts.as_obj(), "shutdown", "()V", &[])?;
                Ok(())
            });
        }
    }
}

/// A new `SpeechListener`, its native method bound to [`ended`]. The
/// class comes from the app's class loader (a native thread's `FindClass`
/// sees only the system's classes).
fn listener<'a>(env: &mut JNIEnv<'a>, activity: &JObject<'_>) -> jni::errors::Result<JObject<'a>> {
    let loader = env
        .call_method(activity, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])?
        .l()?;
    let name = env.new_string(LISTENER)?;
    let class = env
        .call_method(
            &loader,
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&name)],
        )?
        .l()?;
    let class = JClass::from(class);
    env.register_native_methods(
        &class,
        &[jni::NativeMethod {
            name: "ended".into(),
            sig: "(Ljava/lang/String;Z)V".into(),
            fn_ptr: ended as *mut std::ffi::c_void,
        }],
    )?;
    env.new_object(&class, "()V", &[])
}

enum Started {
    /// With the language and speed the engine is set to now.
    Yes((String, u16)),
    /// The engine is not bound yet.
    NotReady,
    /// No voice for the language, or the engine refused the text.
    Failed,
}

/// Starts speaking `text` (or queues it), in its language and at its
/// speed; `voice` is what the engine is set to already.
fn start(
    env: &mut JNIEnv<'_>,
    tts: &JObject<'_>,
    text: &Text,
    voice: &Option<(String, u16)>,
) -> jni::errors::Result<Started> {
    // The engine lists its voices once it is bound.
    let voices = env
        .call_method(tts, "getVoices", "()Ljava/util/Set;", &[])?
        .l()?;
    if voices.is_null() {
        return Ok(Started::NotReady);
    }
    let wanted = (text.language.clone(), text.rate);
    if voice.as_ref() != Some(&wanted) {
        let locale = if text.language.is_empty() {
            env.call_static_method(
                "java/util/Locale",
                "getDefault",
                "()Ljava/util/Locale;",
                &[],
            )?
            .l()?
        } else {
            let tag = env.new_string(&text.language)?;
            env.call_static_method(
                "java/util/Locale",
                "forLanguageTag",
                "(Ljava/lang/String;)Ljava/util/Locale;",
                &[JValue::Object(&tag)],
            )?
            .l()?
        };
        let language = env
            .call_method(
                tts,
                "setLanguage",
                "(Ljava/util/Locale;)I",
                &[JValue::Object(&locale)],
            )?
            .i()?;
        // LANG_MISSING_DATA (-1), LANG_NOT_SUPPORTED (-2).
        if language < 0 {
            return Ok(Started::Failed);
        }
        let rate = f32::from(text.rate) / 100.0;
        env.call_method(tts, "setSpeechRate", "(F)I", &[JValue::Float(rate)])?;
    }
    let words: String = text.text.chars().take(MAX_CHARS).collect();
    let words = env.new_string(words)?;
    let id = env.new_string(text.id.to_string())?;
    let mode = if text.queue { QUEUE_ADD } else { QUEUE_FLUSH };
    let result = env
        .call_method(
            tts,
            "speak",
            "(Ljava/lang/CharSequence;ILandroid/os/Bundle;Ljava/lang/String;)I",
            &[
                JValue::Object(&words),
                JValue::Int(mode),
                JValue::Object(&JObject::null()),
                JValue::Object(&id),
            ],
        )?
        .i()?;
    Ok(if result == 0 {
        Started::Yes(wanted)
    } else {
        Started::Failed
    })
}

/// Opens the system's speech settings, where the voice, its language and
/// the engine are chosen; `false` if the device has none.
pub fn open_settings(app: &AndroidApp) -> bool {
    with_activity(app, |env, activity| {
        let action = env.new_string("com.android.settings.TTS_SETTINGS")?;
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
    })
    .is_some()
}
