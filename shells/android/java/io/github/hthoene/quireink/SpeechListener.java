package io.github.hthoene.quireink;

import android.speech.tts.UtteranceProgressListener;

/**
 * Tells QuireInk when the system's speech output has spoken a text, so the
 * next one follows without a pause. QuireInk's only Java code: the speech
 * output reports progress only to a subclass of this listener.
 */
public final class SpeechListener extends UtteranceProgressListener {
    /** Implemented in Rust (shells/android/src/speech.rs). */
    private static native void ended(String id, boolean spoken);

    @Override
    public void onStart(String id) {}

    @Override
    public void onDone(String id) {
        ended(id, true);
    }

    @Override
    @SuppressWarnings("deprecation")
    public void onError(String id) {
        ended(id, false);
    }
}
