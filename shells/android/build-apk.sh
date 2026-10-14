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
java="$(exe "$jdk_bin/java")"
keytool="$(exe "$jdk_bin/keytool")"
aapt2="$(exe "$tools/aapt2")"
zipalign="$(exe "$tools/zipalign")"

out=target/android
rm -rf "$out/apk" "$out/assets"
mkdir -p "$out/apk/lib" "$out/assets/books"

echo "Building the native library ($profile)"
cargo ndk -t arm64-v8a --platform 26 -o "$out/apk/lib" build "${cargo_profile[@]}" -p quireink-android

echo "Packaging"
cp samples/library/books/*.epub samples/library/books/*.txt "$out/assets/books/"
"$aapt2" link -o "$out/unsigned.apk" \
    --manifest shells/android/AndroidManifest.xml \
    -I "$platform" \
    --min-sdk-version 26 --target-sdk-version 35 \
    --version-code 1 --version-name "$(cargo metadata --no-deps --format-version 1 \
        | grep -o '"name":"quireink-android","version":"[^"]*"' | grep -o '[0-9][^"]*$')"
# Native library and assets via `jar`: aapt2 writes asset paths with
# backslashes on Windows.
"$jar" --update --no-manifest --file "$out/unsigned.apk" -C "$out/apk" lib
"$jar" --update --no-manifest --file "$out/unsigned.apk" -C "$out" assets
"$zipalign" -f -p 4 "$out/unsigned.apk" "$out/aligned.apk"

keystore="$HOME/.android/debug.keystore"
if [[ ! -f "$keystore" ]]; then
    mkdir -p "$(dirname "$keystore")"
    "$keytool" -genkeypair -keystore "$keystore" -alias androiddebugkey \
        -storepass android -keypass android -keyalg RSA -keysize 2048 \
        -validity 10000 -dname "CN=Android Debug,O=Android,C=US"
fi
"$java" -jar "$tools/lib/apksigner.jar" sign --ks "$keystore" \
    --ks-pass pass:android --key-pass pass:android \
    --out "$out/quireink.apk" "$out/aligned.apk"
rm -f "$out/unsigned.apk" "$out/aligned.apk" "$out/quireink.apk.idsig"
echo "Done: $out/quireink.apk"
