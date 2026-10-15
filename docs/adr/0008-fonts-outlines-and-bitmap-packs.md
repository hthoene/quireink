# 0008. Fonts: runtime outlines and bitmap packs

- Status: Accepted
- Date: 2026-10-14

## Context

QuireInk needs good-looking, anti-aliased text on every device. Larger
devices should render any OpenType/TrueType font at any size. The ESP32-C3
has no FPU and no atomic compare-and-swap; `skrifa`/`read-fonts` (the
maintained font reader from Google Fonts) depend on `alloc::sync::Arc`,
which does not exist on that target. The popular `ttf-parser` is no longer
maintained (RUSTSEC-2026-0192).

## Decision

- `quireink-text` provides one `Font` type with two backends:
  - **Outline** (feature `outline`): `skrifa` reads outlines at runtime, a
    built-in accumulation rasterizer renders 4-bit anti-aliased glyphs, and
    a size-bounded LRU glyph cache keeps page turns cheap.
  - **Bitmap packs** (`.qifp`, always available): glyphs pre-rendered per
    size, read in place from flash or storage without RAM or floating-point
    work.
- Packs are produced by `quireink-fontgen` (and by the `fonts-default`
  build script) with the *same* rasterizer, so text renders pixel-identical
  on both paths; a test enforces this.
- Fonts are contributions (`FontSpec`) bound to roles (`core.font-ui`,
  `core.font-reading`) like screens to slots; users rebind roles in
  `[fonts]`.
- Default fonts: Literata (reading) and Atkinson Hyperlegible Next (UI),
  both OFL-1.1, shipped by the `fonts-default` plugin.

## Consequences

- Any device can show any font; small devices are limited to the sizes and
  character sets in their packs (default: Latin, 20 to 48 px).
- No kerning or complex shaping yet; a shaping step (e.g. HarfRust) can be
  added for Tier L later without changing the font API.
- Variable-font features such as optical size are not yet applied per size.
