# Releasing

How a version of QuireInk is released: the Android app on GitHub (signed
with the release key) and, later, on F-Droid.

## Versions

All crates and the app share the workspace's version (`version` under
`[workspace.package]` in `Cargo.toml`), following Semantic Versioning.
Android installs a version over another only if its version code is
higher: 1.2.3 is 10203. The app's manifest
(`shells/android/AndroidManifest.xml`) states both, where F-Droid looks
for them; `build-apk.sh` stops if they differ from `Cargo.toml`.

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

1. Set the version in `Cargo.toml` (also the crates' versions under
   `[workspace.dependencies]`), in the SDK (`SDK_VERSION` in
   `crates/quireink-sdk/src/version.rs`; a test checks it) and in the
   manifest (`android:versionName`, `android:versionCode`), and give
   `CHANGELOG.md` its section,
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

## Reproducible builds

The app comes out byte for byte the same wherever it is built with the
tools of `shells/android/versions.env` (Rust, NDK, build tools, platform,
cargo-ndk, JDK): `build-apk.sh` with `QUIREINK_REPRODUCIBLE=1` checks
them, writes no paths of the computer into the library and packs the APK
in a fixed order, uncompressed and at a fixed time. The workflow
`Reproducible build` builds it twice in two folders and compares (on
every tag, or by hand). One dependency, `jni-macros`, wrote its code in a
random order; `patches/jni-macros` is a copy that does not ("Modified for
QuireInk"), until it is fixed upstream.

To move to newer tools, change `versions.env`: the release workflow, the
check and F-Droid's recipe all read it.

## F-Droid

F-Droid builds apps itself from their source, at a tag, and reads their
description, screenshots and changelogs from `fastlane/metadata/android`
(English and German). Since its build is the same as the release's, it
publishes the release's APK, signed with QuireInk's key, and users can
move between GitHub and F-Droid without reinstalling.

The recipe is `shells/android/fdroid.yml`. To submit it, once:

1. Make an account on [GitLab](https://gitlab.com) and fork
   [fdroiddata](https://gitlab.com/fdroid/fdroiddata).
2. Copy the recipe to `metadata/io.github.hthoene.quireink.yml` in the
   fork, on a new branch, commit and push.
3. Open a merge request to fdroiddata. Its pipeline builds the app;
   F-Droid's reviewers answer there, often with small changes to the
   recipe.

After that, F-Droid finds new versions by their tags (`v1.2.3`) and the
manifest, and builds them by itself. Each version needs its changelog in
`fastlane/metadata/android/*/changelogs/<version code>.txt`.

Translation uses DeepL and highlights can go to Readwise, services that
are not free software; F-Droid marks the app `NonFreeNet` for that (the
recipe says so). Both need the user's own key and do nothing without it.
