#!/usr/bin/env bash
# Builds an installable QuireInk APK for arm64 Android devices without
# Gradle: cargo-ndk compiles the Rust library, the Android SDK's build tools
# package and sign it. Sample books from samples/library/books are bundled.
#
# Requirements: Android SDK with build tools, a platform and the NDK;
# `cargo install cargo-ndk`; `rustup target add aarch64-linux-android`;
# a JDK (for `javac` and `apksigner`); Python 3.
#
# Usage: shells/android/build-apk.sh [--debug]
# Output: target/android/quireink.apk
#   adb install -r target/android/quireink.apk
#
# Signing: with the release key when QUIREINK_KEYSTORE (a .jks or .p12
# file), QUIREINK_KEYSTORE_PASSWORD and QUIREINK_KEY_ALIAS are set
# (QUIREINK_KEY_PASSWORD if the key has its own); not at all with
# QUIREINK_UNSIGNED=1 (target/android/quireink-unsigned.apk, as F-Droid
# builds it); otherwise with a debug key made on this computer.
#
# Releases: with QUIREINK_REPRODUCIBLE=1 the tools must be the versions in
# shells/android/versions.env, so the app comes out byte for byte the same
# on every computer. See docs/releasing.md.

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

# shellcheck source=versions.env
source shells/android/versions.env
reproducible="${QUIREINK_REPRODUCIBLE:-}"
# The pinned version in `dir` if it is there; otherwise the newest, but
# not for a reproducible build.
pick() {
    local dir="$1" pinned="$2"
    if [[ -d "$dir/$pinned" ]]; then
        echo "$pinned"
    elif [[ -n "$reproducible" ]]; then
        fail "$dir/$pinned is needed for a reproducible build"
    else
        latest "$dir"
    fi
}

sdk="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}"
if [[ -z "$sdk" ]]; then
    for candidate in "${LOCALAPPDATA:-}/Android/Sdk" "$HOME/Android/Sdk" "$HOME/Library/Android/sdk"; do
        [[ -d "$candidate" ]] && sdk="$candidate" && break
    done
fi
[[ -d "$sdk" ]] || fail "Android SDK not found; set ANDROID_HOME"
export ANDROID_HOME="$sdk"
if [[ -n "$reproducible" && -n "${ANDROID_NDK_HOME:-}" ]]; then
    [[ "$(basename "$ANDROID_NDK_HOME")" == "$NDK_VERSION" ]] \
        || fail "NDK $NDK_VERSION is needed for a reproducible build, not $ANDROID_NDK_HOME"
elif [[ -z "${ANDROID_NDK_HOME:-}" ]]; then
    ndk="$(pick "$sdk/ndk" "$NDK_VERSION")"
    [[ -n "$ndk" ]] || fail "no NDK in $sdk/ndk"
    export ANDROID_NDK_HOME="$sdk/ndk/$ndk"
fi
tools="$sdk/build-tools/$(pick "$sdk/build-tools" "$BUILD_TOOLS_VERSION")"
platform="$sdk/platforms/$(pick "$sdk/platforms" "android-$PLATFORM_VERSION")/android.jar"
[[ -f "$platform" ]] || fail "no Android platform in $sdk/platforms"

jdk_bin=""
for candidate in "${JAVA_HOME:-}/bin" "/c/Program Files/Android/Android Studio/jbr/bin" \
    /c/Program\ Files/Java/*/bin "/Applications/Android Studio.app/Contents/jbr/Contents/Home/bin"; do
    if [[ -x "$candidate/javac" || -x "$candidate/javac.exe" ]]; then
        jdk_bin="$candidate"
        break
    fi
done
[[ -n "$jdk_bin" ]] || fail "no JDK with 'javac' found; set JAVA_HOME"
exe() { if [[ -x "$1.exe" ]]; then echo "$1.exe"; else echo "$1"; fi; }
javac="$(exe "$jdk_bin/javac")"
java="$(exe "$jdk_bin/java")"
keytool="$(exe "$jdk_bin/keytool")"
aapt2="$(exe "$tools/aapt2")"
zipalign="$(exe "$tools/zipalign")"
python="$(command -v python3 || command -v python)" || fail "no Python 3 found"

if [[ -n "$reproducible" ]]; then
    rustc --version | grep -q "^rustc $RUST_VERSION " \
        || fail "Rust $RUST_VERSION is needed for a reproducible build, not $(rustc --version)"
    cargo ndk --version | grep -q " $CARGO_NDK_VERSION" \
        || fail "cargo-ndk $CARGO_NDK_VERSION is needed for a reproducible build"
    "$javac" -version 2>&1 | grep -q "^javac $JDK_VERSION[.]" \
        || fail "JDK $JDK_VERSION is needed for a reproducible build"
fi

out=target/android
rm -rf "$out/apk" "$out/assets"
mkdir -p "$out/apk/lib" "$out/apk/assets/books"

echo "Building the native library ($profile)"
# No paths of this computer in the library, and no symbols: the same
# bytes wherever it is built, and smaller.
src="$(pwd -W 2>/dev/null || pwd)"
cargo_home="${CARGO_HOME:-$HOME/.cargo}"
export RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$src=/quireink --remap-path-prefix=$cargo_home=/cargo"
export CARGO_PROFILE_RELEASE_STRIP=symbols
cargo ndk -t arm64-v8a --platform 26 -o "$out/apk/lib" build "${cargo_profile[@]}" -p quireink-android

echo "Compiling the speech listener (the only Java class)"
rm -rf "$out/classes" "$out/dex"
mkdir -p "$out/classes" "$out/dex"
"$javac" -source 8 -target 8 -nowarn -Xlint:-options -classpath "$platform" -d "$out/classes" \
    shells/android/java/io/github/hthoene/quireink/*.java
"$java" -cp "$tools/lib/d8.jar" com.android.tools.r8.D8 --release --min-api 26 \
    --lib "$platform" --output "$out/dex" "$out"/classes/io/github/hthoene/quireink/*.class
cp "$out/dex/classes.dex" "$out/apk/"

echo "Packaging"
# The workspace's version, which the app shares.
version="$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' Cargo.toml)"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+ ]] || fail "no version in Cargo.toml"
# Android installs a version over another only if its code is higher:
# 1.2.3 is 10203 (a pre-release counts as its release).
IFS=. read -r major minor patch <<< "${version%%-*}"
version_code=$((major * 10000 + minor * 100 + patch))
cp samples/library/books/*.epub samples/library/books/*.txt "$out/apk/assets/books/"
# Resources: the launcher icon (drawn by tools/android-icon.py).
rm -f "$out/res.zip" "$out/unsigned.apk"
"$aapt2" compile --dir shells/android/res -o "$out/res.zip"
"$aapt2" link -o "$out/unsigned.apk" \
    --manifest shells/android/AndroidManifest.xml \
    "$out/res.zip" \
    -I "$platform" \
    --min-sdk-version 26 --target-sdk-version "$PLATFORM_VERSION" \
    --version-code "$version_code" --version-name "$version"
# The library, the books and the code: sorted, stored, at a fixed time.
"$python" shells/android/pack.py "$out/unsigned.apk" "$out/apk"
"$zipalign" -f -p 4 "$out/unsigned.apk" "$out/aligned.apk"

if [[ -n "${QUIREINK_UNSIGNED:-}" ]]; then
    mv "$out/aligned.apk" "$out/quireink-unsigned.apk"
    rm -f "$out/unsigned.apk"
    echo "Done: $out/quireink-unsigned.apk"
    exit 0
elif [[ -n "${QUIREINK_KEYSTORE:-}" ]]; then
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
