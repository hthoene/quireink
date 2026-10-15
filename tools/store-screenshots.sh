#!/usr/bin/env bash
# Takes the screenshots of the app stores' metadata (F-Droid reads
# fastlane/metadata/android) in the emulator, as a Boox Go 7, in English
# and German, with the public-domain sample books.
#
# Usage: tools/store-screenshots.sh   (from the repository root)

set -euo pipefail

cd "$(dirname "$0")/.."
cargo build -q -p quireink-emulator
emu=target/debug/quireink-emu
[[ -x "$emu" ]] || emu=target/debug/quireink-emu.exe
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

for lang in en-US de-DE; do
    lib="$work/$lang"
    out="fastlane/metadata/android/$lang/images/phoneScreenshots"
    mkdir -p "$lib/books" "$out"
    cp samples/library/books/* "$lib/books/"
    run() {
        "$emu" --profile boox-go-7 --headless --library "$lib" \
            --set "core.language=${lang%%-*}" --set reader.open-last=off "$@" >/dev/null
    }
    # Read a little of one book, then of the other, the one in the
    # language last: a book to continue and one read before. The library
    # first gathers the books' details, then lists the last read first:
    # rows at y 190 (first), 420 (second) and 600 (third).
    if [[ "$lang" == en-US ]]; then rows=(600 420); else rows=(190 600); fi
    run --set slots.core.home=files.browser \
        --script "wait:3000,tap:600:${rows[0]},wait:500,page-forward,page-forward"
    run --set slots.core.home=files.browser \
        --script "wait:500,tap:600:${rows[1]},wait:500,page-forward,page-forward,page-forward,page-forward,page-forward,page-forward" \
        --screenshot "$out/2.png"
    run --script "wait:1000" --screenshot "$out/1.png"
    run --set slots.core.home=files.browser --script "wait:500" --screenshot "$out/3.png"
    # The reading menu over a page.
    run --set slots.core.home=files.browser \
        --script "wait:500,tap:600:190,wait:500,tap:632:840" --screenshot "$out/4.png"
    run --set slots.core.home=settings.list --screenshot "$out/5.png"
    echo "$out"
done

# Gray, as on the panel, and small: 16 levels (needs Pillow).
python="$(command -v python3 || command -v python)"
"$python" - <<'EOF'
import glob
from PIL import Image
for path in glob.glob("fastlane/metadata/android/*/images/phoneScreenshots/*.png"):
    Image.open(path).convert("L").quantize(16).convert("L").save(path, optimize=True)
EOF
