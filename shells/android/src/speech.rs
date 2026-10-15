//! Reading aloud through the system's speech output (`TextToSpeech`).
//!
//! The engine is bound at the first text, not at start-up, and answers
//! asynchronously: a text that comes before it is ready waits for it a
//! while. Without Java code there is no progress listener, so the end of
//! a text is seen by asking `isSpeaking` regularly (see [`POLL`]).

use std::time::{Duration, Instant};

use android_activity::AndroidApp;
use jni::JNIEnv;
use jni::objects::{GlobalRef, JObject, JValue};
use quireink_core::SpeechCall;

use crate::access::with_activity;

/// How often the end of a text is looked for.
pub const POLL: Duration = Duration::from_millis(100);
/// How long a text waits for the engine to be ready.
const READY_WAIT: Duration = Duration::from_secs(8);
/// How long a text may take to start being heard; one never heard is
/// taken as spoken after this (e.g. only punctuation).
const START_WAIT: Duration = Duration::from_secs(3);
/// The most characters spoken at once (engines take at least 4000).
const MAX_CHARS: usize = 3_900;
/// `TextToSpeech.QUEUE_FLUSH`: instead of what is spoken now.
const QUEUE_FLUSH: i32 = 0;

/// A text for the engine.
struct Text {
    id: u32,
    text: String,
    language: String,
    rate: u16,
    since: Instant,
}

/// The system's speech output.
pub struct Speech {
    app: AndroidApp,
    /// The engine, once bound.
    tts: Option<GlobalRef>,
    /// A text waiting for the engine.
    pending: Option<Text>,
    /// The text being spoken: its number, since when, and whether it was
    /// heard yet.
    current: Option<(u32, Instant, bool)>,
    /// A text that ended at once (it failed, or there was nothing to say),
    /// with whether it was spoken.
    ended: Option<(u32, bool)>,
}

impl Speech {
    pub fn new(app: &AndroidApp) -> Self {
        Self {
            app: app.clone(),
            tts: None,
            pending: None,
            current: None,
            ended: None,
        }
    }

    /// Whether a text waits or is spoken: its end is to be looked for.
    pub fn active(&self) -> bool {
        self.pending.is_some() || self.current.is_some() || self.ended.is_some()
    }

    /// Does what the kernel asks.
    pub fn apply(&mut self, call: SpeechCall) {
        match call {
            SpeechCall::Say {
                id,
                text,
                language,
                rate,
            } => {
                self.stop();
                if text.trim().is_empty() {
                    self.ended = Some((id, true));
                    return;
                }
                if self.tts.is_none() {
                    self.tts = self.create();
                }
                self.pending = Some(Text {
                    id,
                    text,
                    language,
                    rate,
                    since: Instant::now(),
                });
            }
            SpeechCall::Stop => self.stop(),
        }
    }

    /// Looks whether the text waiting can start or the one spoken has
    /// ended: returns it with whether it was spoken.
    pub fn poll(&mut self) -> Option<(u32, bool)> {
        if let Some(ended) = self.ended.take() {
            return Some(ended);
        }
        let Some(tts) = self.tts.clone() else {
            // Not even bound: what waits fails.
            return self.pending.take().map(|text| (text.id, false));
        };
        if let Some(text) = self.pending.take() {
            return match with_activity(&self.app, |env, _| start(env, tts.as_obj(), &text)) {
                Some(Started::Yes) => {
                    self.current = Some((text.id, Instant::now(), false));
                    None
                }
                Some(Started::NotReady) if text.since.elapsed() < READY_WAIT => {
                    self.pending = Some(text);
                    None
                }
                _ => Some((text.id, false)),
            };
        }
        let (id, since, heard) = self.current?;
        let speaking = with_activity(&self.app, |env, _| {
            env.call_method(tts.as_obj(), "isSpeaking", "()Z", &[])?.z()
        })
        .unwrap_or(false);
        if speaking {
            self.current = Some((id, since, true));
            None
        } else if heard || since.elapsed() >= START_WAIT {
            self.current = None;
            Some((id, true))
        } else {
            None
        }
    }

    /// Stops speaking, forgetting the text.
    fn stop(&mut self) {
        self.pending = None;
        self.ended = None;
        if self.current.take().is_some()
            && let Some(tts) = &self.tts
        {
            with_activity(&self.app, |env, _| {
                env.call_method(tts.as_obj(), "stop", "()I", &[])?.i()
            });
        }
    }

    /// Binds the engine (it becomes ready a little later).
    fn create(&self) -> Option<GlobalRef> {
        let tts = with_activity(&self.app, |env, activity| {
            crate::access::prepare_looper(env)?;
            let tts = env.new_object(
                "android/speech/tts/TextToSpeech",
                "(Landroid/content/Context;Landroid/speech/tts/TextToSpeech$OnInitListener;)V",
                &[JValue::Object(activity), JValue::Object(&JObject::null())],
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

enum Started {
    Yes,
    /// The engine is not bound yet.
    NotReady,
    /// No voice for the language, or the engine refused the text.
    Failed,
}

/// Starts speaking `text`, in its language and at its speed.
fn start(env: &mut JNIEnv<'_>, tts: &JObject<'_>, text: &Text) -> jni::errors::Result<Started> {
    // The engine lists its voices once it is bound.
    let voices = env
        .call_method(tts, "getVoices", "()Ljava/util/Set;", &[])?
        .l()?;
    if voices.is_null() {
        return Ok(Started::NotReady);
    }
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
    let words: String = text.text.chars().take(MAX_CHARS).collect();
    let words = env.new_string(words)?;
    let id = env.new_string(text.id.to_string())?;
    let result = env
        .call_method(
            tts,
            "speak",
            "(Ljava/lang/CharSequence;ILandroid/os/Bundle;Ljava/lang/String;)I",
            &[
                JValue::Object(&words),
                JValue::Int(QUEUE_FLUSH),
                JValue::Object(&JObject::null()),
                JValue::Object(&id),
            ],
        )?
        .i()?;
    Ok(if result == 0 {
        Started::Yes
    } else {
        Started::Failed
    })
}
