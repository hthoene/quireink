# QuireInk for Android

A native activity in Rust, without Java code. The app runs on Android
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

- **Books:** `Android/data/io.github.hthoene.quireink/files/books` on the
  device's storage, reachable over USB from a computer. The sample books
  are copied there on first start, and `quireink.ini` lives in the folder
  above.
- **Touch:** tap the left or right side to turn pages, the middle for the
  menu, or swipe. Long-press for more options.
- **Buttons:** page keys and volume keys turn pages; Back goes back.
- **Language:** the system language, unless one is chosen in the settings.

## Limitations

These are planned:

- E-ink refresh modes are left to the device. Boox's optimization settings
  for the app apply; Onyx's SDK will map QuireInk's refresh requests later.
- Rendering is grayscale, also on colour panels.
- Only the app's own folder is read, not the whole shared storage.
- There is no frontlight control and no app icon yet.
