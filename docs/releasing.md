# Releasing

How a version of QuireInk is released: the Android app on GitHub (signed
with the release key) and, later, on F-Droid.

## Versions

All crates and the app share the workspace's version (`version` under
`[workspace.package]` in `Cargo.toml`), following Semantic Versioning.
Android installs a version over another only if its version code is
higher; `shells/android/build-apk.sh` derives it from the version:
1.2.3 is 10203.

## The release key, once

Every release of the app must be signed with the same key, or Android
refuses to install it over the one before. Make it once, keep it and
its password safe (a password manager, and a backup of the file away
from the computer), and never put it into the repository:

```sh
keytool -genkeypair -keystore quireink-release.p12 -storetype PKCS12 \
  -alias quireink -keyalg RSA -keysize 4096 -validity 36500 \
  -dname "CN=QuireInk"
```

`keytool` asks for the password. Then give it to GitHub as secrets of the
repository (the GitHub CLI asks for each value):

```sh
base64 -w0 quireink-release.p12 | gh secret set ANDROID_KEYSTORE_BASE64
gh secret set ANDROID_KEYSTORE_PASSWORD
gh secret set ANDROID_KEY_ALIAS
```

(`ANDROID_KEY_PASSWORD` only if the key has a password of its own.)

To build a release locally, `build-apk.sh` takes the same as environment
variables: `QUIREINK_KEYSTORE` (the file), `QUIREINK_KEYSTORE_PASSWORD`,
`QUIREINK_KEY_ALIAS`. Without them it signs with a debug key, fine for
testing but not for releases.

An app signed with the debug key cannot be updated to one signed with
the release key: uninstall it first. QuireInk's own folder on the device
(`Android/data/…`) goes with it; books and plugins in the shared `QuireInk`
folder stay.

## A release

1. Set the version in `Cargo.toml` and give `CHANGELOG.md` its section,
   `## [x.y.z] - YYYY-MM-DD`, with a link at the end.
2. Add `fastlane/metadata/android/*/changelogs/<version code>.txt` (at most
   500 characters each), and new screenshots if the app looks different
   (`tools/store-screenshots.sh`).
3. Commit, then tag and push:

   ```sh
   git tag -a v0.1.0 -m "QuireInk 0.1.0"
   git push origin main v0.1.0
   ```

The workflow `Release` checks that the tag is the version, builds the app
signed with the release key and makes the GitHub release with the app,
its SHA-256 and the changelog's section as notes. Versions before 1.0 are
marked as pre-releases.

## F-Droid

F-Droid builds apps itself from their source, at a tag, and reads their
description, screenshots and changelogs from `fastlane/metadata/android`
(English and German are there). To be listed, QuireInk needs a build
recipe in [fdroiddata](https://gitlab.com/fdroid/fdroiddata), submitted
as a merge request. What it has to say and what is still to do:

- **The build:** no Gradle; F-Droid runs `shells/android/build-apk.sh`
  with the Android SDK and NDK, Rust with the target
  `aarch64-linux-android` and `cargo-ndk`. Everything QuireInk uses is
  free software and comes from source.
- **The signature:** F-Droid signs with its own key unless the app it
  builds is byte for byte the one released here; then it publishes ours,
  and users can switch between GitHub and F-Droid without reinstalling.
  For that the build must be reproducible: a fixed Rust version for
  releases, paths not written into the library, and fixed times in the
  APK (today `jar` writes the current time). Until then the two are
  separate apps to Android.
- **Anti-features:** translation uses DeepL and highlights can go to
  Readwise, services that are not free software; F-Droid marks such apps
  `NonFreeNet`. Both need the user's own key and do nothing without it.
