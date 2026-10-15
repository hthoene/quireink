#!/usr/bin/env bash
# Builds an installable QuireInk APK for arm64 Android devices without
# Gradle: cargo-ndk compiles the Rust library, the Android SDK's build tools
# package and sign it. Sample books from samples/library/books are bundled.
#
# Requirements: Android SDK with build tools, a platform and the NDK;
# `cargo install cargo-ndk`; `rustup target add aarch64-linux-android`;
# a JDK (for `jar` and `apksigner`).
#
# Usage: shells/android/build-apk.sh [--debug]
# Output: target/android/quireink.apk
#   adb install -r target/android/quireink.apk
#
# Signing: with the release key when QUIREINK_KEYSTORE (a .jks or .p12
# file), QUIREINK_KEYSTORE_PASSWORD and QUIREINK_KEY_ALIAS are set
# (QUIREINK_KEY_PASSWORD if the key has its own); otherwise with a debug
# key made on this computer. See docs/releasing.md.

set -euo pipefail

cd "$(dirname "$0")/../.."
profile=release
cargo_profile=(--release)
if [[ "${1:-}" == "--debug" ]]; then
    profile=debug
    cargo_profile=()
fi

# The newest version in a directory of versioned subdirectories.
latest() { ls -1 "$1" 2>/dev/null | sort -V | tail -n 1; }
fail() { echo "error: $*" >&2; exit 1; }

sdk="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}"
if [[ -z "$sdk" ]]; then
    for candidate in "${LOCALAPPDATA:-}/Android/Sdk" "$HOME/Android/Sdk" "$HOME/Library/Android/sdk"; do
        [[ -d "$candidate" ]] && sdk="$candidate" && break
    done
fi
[[ -d "$sdk" ]] || fail "Android SDK not found; set ANDROID_HOME"
export ANDROID_HOME="$sdk"
if [[ -z "${ANDROID_NDK_HOME:-}" ]]; then
    ndk="$(latest "$sdk/ndk")"
    [[ -n "$ndk" ]] || fail "no NDK in $sdk/ndk"
    export ANDROID_NDK_HOME="$sdk/ndk/$ndk"
fi
tools="$sdk/build-tools/$(latest "$sdk/build-tools")"
platform="$sdk/platforms/$(latest "$sdk/platforms")/android.jar"
[[ -f "$platform" ]] || fail "no Android platform in $sdk/platforms"

jdk_bin=""
for candidate in "${JAVA_HOME:-}/bin" "/c/Program Files/Android/Android Studio/jbr/bin" \
    /c/Program\ Files/Java/*/bin "/Applications/Android Studio.app/Contents/jbr/Contents/Home/bin"; do
    if [[ -x "$candidate/jar" || -x "$candidate/jar.exe" ]]; then
        jdk_bin="$candidate"
        break
    fi
done
[[ -n "$jdk_bin" ]] || fail "no JDK with 'jar' found; set JAVA_HOME"
exe() { if [[ -x "$1.exe" ]]; then echo "$1.exe"; else echo "$1"; fi; }
jar="$(exe "$jdk_bin/jar")"
javac="$(exe "$jdk_bin/javac")"
java="$(exe "$jdk_bin/java")"
keytool="$(exe "$jdk_bin/keytool")"
aapt2="$(exe "$tools/aapt2")"
zipalign="$(exe "$tools/zipalign")"

out=target/android
rm -rf "$out/apk" "$out/assets"
mkdir -p "$out/apk/lib" "$out/assets/books"

echo "Building the native library ($profile)"
cargo ndk -t arm64-v8a --platform 26 -o "$out/apk/lib" build "${cargo_profile[@]}" -p quireink-android

echo "Compiling the speech listener (the only Java class)"
rm -rf "$out/classes" "$out/dex"
mkdir -p "$out/classes" "$out/dex"
"$javac" -source 8 -target 8 -nowarn -Xlint:-options -classpath "$platform" -d "$out/classes"     shells/android/java/io/github/hthoene/quireink/*.java
"$java" -cp "$tools/lib/d8.jar" com.android.tools.r8.D8 --release --min-api 26     --lib "$platform" --output "$out/dex" "$out"/classes/io/github/hthoene/quireink/*.class

echo "Packaging"
# The workspace's version, which the app shares.
version="$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' Cargo.toml)"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+ ]] || fail "no version in Cargo.toml"
# Android installs a version over another only if its code is higher:
# 1.2.3 is 10203 (a pre-release counts as its release).
IFS=. read -r major minor patch <<< "${version%%-*}"
version_code=$((major * 10000 + minor * 100 + patch))
cp samples/library/books/*.epub samples/library/books/*.txt "$out/assets/books/"
# Resources: the launcher icon (drawn by tools/android-icon.py).
rm -f "$out/res.zip"
"$aapt2" compile --dir shells/android/res -o "$out/res.zip"
"$aapt2" link -o "$out/unsigned.apk" \
    --manifest shells/android/AndroidManifest.xml \
    "$out/res.zip" \
    -I "$platform" \
    --min-sdk-version 26 --target-sdk-version 35 \
    --version-code "$version_code" --version-name "$version"
# Native library and assets via `jar`: aapt2 writes asset paths with
# backslashes on Windows.
"$jar" --update --no-manifest --file "$out/unsigned.apk" -C "$out/apk" lib
"$jar" --update --no-manifest --file "$out/unsigned.apk" -C "$out" assets
"$jar" --update --no-manifest --file "$out/unsigned.apk" -C "$out/dex" classes.dex
"$zipalign" -f -p 4 "$out/unsigned.apk" "$out/aligned.apk"

if [[ -n "${QUIREINK_KEYSTORE:-}" ]]; then
    [[ -f "$QUIREINK_KEYSTORE" ]] || fail "no keystore at $QUIREINK_KEYSTORE"
    [[ -n "${QUIREINK_KEYSTORE_PASSWORD:-}" && -n "${QUIREINK_KEY_ALIAS:-}" ]] \
        || fail "QUIREINK_KEYSTORE needs QUIREINK_KEYSTORE_PASSWORD and QUIREINK_KEY_ALIAS"
    export QUIREINK_KEY_PASSWORD="${QUIREINK_KEY_PASSWORD:-$QUIREINK_KEYSTORE_PASSWORD}"
    echo "Signing with the release key"
    # The passwords come from the environment, never on a command line.
    "$java" -jar "$tools/lib/apksigner.jar" sign --ks "$QUIREINK_KEYSTORE" \
        --ks-key-alias "$QUIREINK_KEY_ALIAS" \
        --ks-pass env:QUIREINK_KEYSTORE_PASSWORD --key-pass env:QUIREINK_KEY_PASSWORD \
        --out "$out/quireink.apk" "$out/aligned.apk"
else
    keystore="$HOME/.android/debug.keystore"
    if [[ ! -f "$keystore" ]]; then
        mkdir -p "$(dirname "$keystore")"
        "$keytool" -genkeypair -keystore "$keystore" -alias androiddebugkey \
            -storepass android -keypass android -keyalg RSA -keysize 2048 \
            -validity 10000 -dname "CN=Android Debug,O=Android,C=US"
    fi
    echo "Signing with the debug key (not for releases)"
    "$java" -jar "$tools/lib/apksigner.jar" sign --ks "$keystore" \
        --ks-pass pass:android --key-pass pass:android \
        --out "$out/quireink.apk" "$out/aligned.apk"
fi
rm -f "$out/unsigned.apk" "$out/aligned.apk" "$out/quireink.apk.idsig"
echo "Done: $out/quireink.apk"
