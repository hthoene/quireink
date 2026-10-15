# QuireInk for Android

A native activity in Rust; its only Java code is a small listener for the
system's speech output (reading aloud). The app runs on Android
e-readers (Boox and others) and on any Android device with an arm64 CPU
and Android 8 or later. It is tested on a Boox Go 7 (Android 13).

## Building

Requirements:

- the Android SDK with build tools, a platform and the NDK (e.g. installed
  by Android Studio);
- a JDK (Android Studio includes one);
- `rustup target add aarch64-linux-android` and `cargo install cargo-ndk`.

Then build and install:

```bash
shells/android/build-apk.sh
adb install -r target/android/quireink.apk
```

The script finds the SDK through `ANDROID_HOME` and the usual install
locations, builds `libquireink.so` with `cargo-ndk`, and packages and signs
the APK with the SDK's build tools and the debug keystore. It does not use
Gradle. `--debug` builds without optimizations. CI builds the same APK on
every push; it can be downloaded from the workflow run's artifacts.

To install on an e-reader, enable USB debugging once: open *Settings →
About*, tap *Build number* seven times, then enable *Developer options →
USB debugging*.

## Using it

- **Books:** choose *Allow access to your books* on the home screen and
  allow *All files access* (Android 11+) or the storage permission (older
  versions); QuireInk restarts and shows the shared `Books` folder, where
  Boox and most e-readers keep books. Any other folder can be chosen under
  *Settings → Library → Library folder*. Until access is allowed, the
  library is `Android/data/io.github.hthoene.quireink/files/books` with the
  sample books. `quireink.ini` and all caches stay in
  `Android/data/io.github.hthoene.quireink/files`.
- **Plugins without code:** copy plugin folders into `QuireInk/plugins` on
  the device's storage (created once access is allowed) and restart.
- **Touch:** tap the left or right side to turn pages, the middle for the
  menu, or swipe. Long-press for more options.
- **Buttons:** page keys and volume keys turn pages; Back goes back.
- **Language:** the system language, unless one is chosen in the settings.
- **Frontlight (Onyx Boox):** brightness and, where the device has warm
  and cold light, its warmth, in the reader's menu and under
  *Settings → Display*. They follow the device, so changes made with the
  system's own controls show up too.
- **E-ink (Onyx Boox):** QuireInk chooses the waveform for every frame:
  a flashing full refresh every few pages (*Settings → Display*) and on
  new screens, clean partial refreshes for page turns and fast ones for
  menus. Boox's own per-app refresh setting applies again when QuireInk is
  in the background.

## Limitations

These are planned:

- Rendering is grayscale, also on colour panels.
