# QuireInk: Project Plan

> Status: **Draft v0.9**, a living document. Decisions that are not yet final are
> marked **[OPEN]**. Facts about hardware that still need verification are
> marked **[VERIFY]**. Once a decision is final it moves into an Architecture
> Decision Record under `docs/adr/`.

---

## 1. Vision

QuireInk is a lean, fast, power-efficient e-reader application that runs from
**one codebase** on as many e-ink devices as possible, from a microcontroller
with 380 KB of RAM up to Linux and Android e-readers.

The core is deliberately small and simple. Everything beyond the essentials is
added through **plugins**, so the community can extend QuireInk without
touching the core.

### 1.1 Guiding principles

| # | Principle | What it means in practice |
|---|-----------|---------------------------|
| P1 | **Simple to use** | Opening the device continues the last book. Every function is reachable in at most two steps from the reader. Sensible defaults; no setting is required to start reading. |
| P2 | **Lean** | The smallest target (ESP32-C3) defines the budget. No garbage collector, no runtime, no heavyweight dependencies in the core. |
| P3 | **Fast** | Page turns feel instant. Work is done ahead of time (pagination caches) and in the background, never while the reader waits. |
| P4 | **Power-efficient** | Event-driven, never polling. Only redraw what changed. Radios off unless needed. Sleep whenever idle. |
| P5 | **Small kernel, everything else is a plugin** | The core is a kernel: plugin host, registries, input routing, rendering, settings. Every screen (including home and library), format, theme, key map, translation and service is a plugin; first-party ones use the same SDK as community plugins. Anything can be added, replaced, reconfigured or removed. |
| P6 | **Portable by construction** | The core never talks to an OS. All I/O goes through platform interfaces implemented by thin shells. |
| P7 | **Testable without hardware** | Every feature can be developed and tested in the desktop emulator, including memory limits of the smallest device. |
| P8 | **Open-source quality** | Clean history, documentation, CI, reproducible builds, license hygiene from day one. |

### 1.2 Non-goals (for now)

- Smartphone/tablet UI with animations or app-store polish. (Colour e-ink is
  supported, see §3.7, but the UI stays e-ink-first.)
- DRM-protected content (Kindle KFX, Adobe ADEPT, etc.).
- Being a full KOReader replacement on day one; we grow towards it.

---

## 2. Target devices

Devices are grouped into **capability tiers**. Features are designed per tier, so
small devices are never blocked by features of large devices, and large
devices are not limited by small ones.

| Tier | Devices | CPU / RAM | Shell | Notes |
|------|---------|-----------|-------|-------|
| **S** (small) | Xteink X4, X3 | ESP32-C3, ~380 KB SRAM, no PSRAM, 16 MB flash | `esp32` (bare metal) | Buttons only. microSD **[VERIFY]**. Display 800×480 **[VERIFY: controller, grayscale capability]**. |
| **M** (medium) | Xteink X4 Pro | ESP32-S3 **[VERIFY]**, 512 KB SRAM, PSRAM **[VERIFY]** | `esp32` | 4.3″, 219 ppi (800×480 **[VERIFY]**), touch, previous/next buttons, touch home key, power, reset; warm/cool frontlight, Wi-Fi 2.4 GHz, Bluetooth, microSD (16 GB included), 1,100 mAh, pogo-pin charging (no USB port). Third-party firmware allowed for devices bought from Xteink. Xtensa toolchain via `espup`. Emulator profile `xteink-x4-pro` assumes no PSRAM (256 KiB heap). |
| **L** (large) | Kobo, Kindle (jailbroken), PocketBook, reMarkable | ARM Linux, ≥ 256 MB RAM | `linux-fb` | Touch, frontlight, Wi-Fi, vendor-specific e-ink refresh. Colour models exist (Kobo Libra/Clara Colour, Kindle Colorsoft, PocketBook Era Color, reMarkable Paper Pro). |
| **L** | Boox & other Android e-readers | Android, ≥ 1 GB RAM | `android` | Thin Java/Kotlin shim only where vendor SDKs require it. Monochrome and colour models. |
| **Dev** | Desktop Linux, Windows, macOS | n/a | `desktop` (= emulator) | Primary development environment. |
| **Dev** | Web browser | WASM | `web` | Public online demo (GitHub Pages) + emulator. |

### 2.1 First devices

Hardware on hand:

- **Boox Go 7**: monochrome, Android **[VERIFY version]**
- **Boox Air 6C**: colour, Android 16 **[VERIFY]**
- **Xteink X4**: on order (used)
- **Xteink X4 Pro**: on order

1. **Desktop emulator**: everything starts here (see §9).
2. **Xteink X4**: the hardest constraint; proves the architecture.
3. **Boox Go 7 / Air 6C** (Android): available now, covers monochrome *and*
   colour; pulled forward if the X4 arrives late.
4. **Kobo**: easiest real Linux device (runs custom software without jailbreak),
   then Kindle, PocketBook, reMarkable.

---

## 3. Key technical decisions

### 3.1 Language: Rust (**decided**)

| Criterion | Rust | C++ (on top of CrossPoint) |
|-----------|------|----------------------------|
| Runs on ESP32-C3 without runtime | ✅ `no_std` + `alloc`, `riscv32imc-unknown-none-elf` is a stable upstream target | ✅ |
| One codebase for all targets | ✅ Cargo workspace + features, trivial cross-compiling (incl. WASM, Android, static musl) | ⚠️ CMake/PlatformIO/NDK/Emscripten build zoo |
| Memory safety with untrusted input (books, plugins) | ✅ | ⚠️ parsers are a classic attack surface |
| Plugin system | ✅ traits for built-in plugins, `wasmi` (pure Rust) for runtime plugins | ⚠️ |
| Head start | ⚠️ write from scratch | ✅ reuse CrossPoint's EPUB/render code |
| Community familiarity | ✅ growing in embedded | ✅ |

**Decision: Rust.** CrossPoint Reader (MIT) remains a valuable reference:
its algorithms and hardware knowledge (display init, waveforms, button
mapping) can be ported with attribution.

### 3.2 License: Apache-2.0 (**decided**)

The license limits which libraries we may use. Options considered:

| Option | Pros | Cons |
|--------|------|------|
| **Apache-2.0** | Permissive, maximum adoption; explicit patent grant protects contributors and users. Compatible with GPL-3.0 (one-way). | Cannot link GPL-2.0-only or AGPL libraries into distributed binaries under Apache terms. |
| MIT OR Apache-2.0 (Rust ecosystem default) | Same, plus MIT simplicity. | Dual licensing adds little for an application. |
| **GPL-3.0-or-later** | Can use FBInk, DjVuLibre; forks must stay open. | Cannot use MuPDF (AGPL) unless AGPL; less attractive for embedding. |
| **AGPL-3.0** (like KOReader) | Can use MuPDF, FBInk, DjVuLibre; could reuse KOReader code. | Strongest copyleft; least permissive. |

**Decision: Apache-2.0** for core, shells and first-party plugins.
Contributions are accepted under the same license (inbound = outbound, DCO
sign-off). **Copyleft libraries are only allowed inside optional plugins** that
are distributed separately under their own license (e.g. a `pdf-mupdf` plugin
under AGPL). PDF can alternatively use `pdfium` (BSD/Apache). FBInk (GPL) is
not linked; the Linux shell implements the vendor ioctls itself.

License compliance is enforced in CI with `cargo-deny`.

### 3.3 Sans-I/O core

The core is a pure state machine:

```text
           ┌──────────── Shell (platform) ────────────┐
 events ──▶│  core.handle(Event) -> Vec<Effect>       │──▶ effects
           └──────────────────────────────────────────┘
 Event  = Input(Action) | Timer | StorageReady | NetResponse | Power(…) | …
 Effect = Refresh(region, mode) | ScheduleTimer | Read/Write | HttpRequest | Sleep | …
```

- No threads, no async runtime and no OS calls in the core.
- Shells decide *how* to execute effects (Embassy on ESP32, epoll on Linux,
  Android looper, browser event loop).
- Deterministic → reproducible tests, input replay, snapshot testing.

### 3.4 Rendering via display lists

Layout produces a **display list** per page (positioned glyph runs, rules,
images) instead of pixels.

- Small and serializable → can be cached on storage.
- Rasterized **in horizontal bands**, so the smallest devices never need a full
  high-depth framebuffer in RAM (bands are streamed to the display controller).
- Dirty-region diffing between display lists → minimal partial refreshes.

### 3.5 Reading positions are content-based, not page-based

Note: pixel-identical pagination only holds for identical screen size, font and
settings. Devices differ, so positions are stored as **locators** that point
into the content:

```text
Locator { spine_index, node_path, char_offset }   // EPUB/HTML/FB2
Locator { byte_offset }                           // TXT
Locator { page_index }                            // fixed-layout (PDF, CBZ)
```

Locators convert to/from **KOReader xpointers** (e.g.
`/body/DocFragment[3]/body/div/p[5]/text().42`) and percentage, so progress
syncs with KOReader devices via the kosync protocol. Pagination caches map
locators ↔ page numbers per device profile.

### 3.6 Crate naming

The crate name `quire` is already taken on crates.io, so all crates use the
prefix `quireink-`.

### 3.7 Colour e-ink

Colour e-readers (Kaleido 3 and successors, Gallery 3, …) are supported on
Tier L. Design rules:

- **Pixel format is a parameter**, like resolution and DPI. The rasterizer works
  on an abstract pixel format; colour formats are a Cargo feature that is
  compiled out on Tier S to save flash and RAM.
- **Text stays sharp:** colour-filter panels have a lower effective colour
  resolution (typically half the monochrome resolution). Text is rendered in
  full-resolution grayscale; colour is used where it adds value.
- **Where colour is used:** book covers, images in EPUBs, comics/manga (CBZ),
  PDFs, highlight colours, optional UI accent colour. The UI must remain fully
  usable in monochrome; colour is never the only carrier of information.
- **Colour pipeline:** per-panel colour profile (saturation boost, gamma) and
  dithering adapted to the colour filter layout; tunable in settings.
- **Refresh modes:** colour panels need dedicated waveforms (e.g. a slower
  colour-accurate mode vs. fast monochrome mode for page turns); the
  `RefreshMode` set is extended per device.
- The emulator gets colour panel profiles that simulate the reduced colour
  resolution and muted saturation.

---

## 4. Architecture

> **Design rule #1: the core is a kernel, the product is plugins.**
> The core contains *no* product UI and *no* product logic. Home screen,
> library, reader, menus, settings screens, file formats, themes, key maps and
> translations are all **first-party plugins** that use exactly the same public
> SDK as community plugins. If a built-in feature can be written as a plugin,
> it must be. This guarantees that the SDK is always powerful enough to
> replace anything QuireInk ships with.

### 4.1 Layers

```text
┌─────────────────────────────────────────────────────────────────────┐
│ Plugins (first-party and community: same SDK, same rights)          │
│  ui-home · ui-files · ui-reader · ui-settings · ui-menu · theme-*   │
│  format-txt/epub/fb2/… · sync-kosync · source-opds · lang-de · …    │
├─────────────────────────────────────────────────────────────────────┤
│ SDK (quireink-sdk): the only API plugins may use; SemVer-versioned  │
│  ids · manifest · contributions · services · events · widgets ·     │
│  canvas · settings · i18n · storage namespaces                      │
├─────────────────────────────────────────────────────────────────────┤
│ Kernel (quireink-core, no_std + alloc)                              │
│  plugin host · registries · slot bindings · input → action routing  │
│  event bus · service locator · settings store · refresh manager     │
│  widget runtime · render engine (display lists, rasterizer)         │
├─────────────────────────────────────────────────────────────────────┤
│ Platform vocabulary (quireink-platform)                             │
│  display info · raw input events · pixel formats · Display trait …  │
├──────────────┬──────────────┬──────────────┬───────────┬────────────┤
│ esp32 shell  │ linux-fb     │ android      │ desktop   │ web        │
└──────────────┴──────────────┴──────────────┴───────────┴────────────┘
```

Dependency rules (enforced in CI):

- Plugins depend on `quireink-sdk` only, never on `quireink-core` internals.
- The kernel depends on the SDK (it implements the host side) and never on a
  plugin.
- Shells depend on the kernel plus the plugins they bundle.

### 4.2 Workspace layout

```text
quireink/
├─ crates/
│  ├─ quireink-platform/      # shell-facing vocabulary (no_std, no alloc)
│  ├─ quireink-config/        # human-editable config format (no_std)
│  ├─ quireink-i18n/          # message catalogs, plural rules (no_std)
│  ├─ quireink-graphics/      # framebuffer, canvas, later display lists (no_std)
│  ├─ quireink-sdk/           # THE plugin API (no_std)
│  ├─ quireink-core/          # kernel (no_std)
│  ├─ quireink-text/          # line breaking, hyphenation, fonts      (later)
│  ├─ quireink-zip/           # streaming ZIP reader                    (later)
│  └─ quireink-xml/           # streaming XML/HTML tokenizer            (later)
├─ plugins/                   # first-party plugins, one crate each
│  ├─ diagnostics/            # M0 test screen
│  ├─ ui-home/  ui-files/  ui-reader/  ui-settings/  ui-menu/
│  ├─ theme-default/  keymap-default/  lang-de/
│  ├─ format-txt/  format-epub/  …
│  └─ sync-kosync/  source-opds/  …
├─ shells/
│  ├─ desktop/                # emulator with device profiles
│  └─ esp32/  linux-fb/  android/  web/
├─ tools/                     # fontgen, hyphgen, plugin packager
├─ assets/                    # default fonts, icons, hyphenation data
└─ docs/                      # PLANNING.md, adr/, sdk/
```

### 4.3 Platform interfaces (sketch)

```rust
pub struct DisplayInfo {
    pub width: u16, pub height: u16, pub dpi: u16,
    pub format: PixelFormat,        // Mono1, Gray2, Gray4, Gray8, Rgb565, Rgb888
    pub color: Option<ColorPanel>,  // colour filter layout + effective colour resolution
    pub rotation: Rotation,
}

pub enum RefreshMode { Fast, Partial, Full }

pub trait Display {
    fn info(&self) -> DisplayInfo;
    fn refresh(&mut self, frame: FrameView<'_>, region: Rect, mode: RefreshMode);
}
```

Shells deliver **raw, device-neutral input events** (named buttons and touch
gestures), *not* actions:

```text
Button("page-forward", Pressed) · Button("page-forward", Released)
Pointer(Down, x, y, finger) · Pointer(Move, …) · Pointer(Up, …)
```

The kernel resolves them to actions through **layered key maps** (see §7.4),
so mapping works the same on every device and is fully user-configurable.

---

## 5. Memory and performance strategy (Tier S)

Budget for the ESP32-C3 (≈ 380 KB SRAM total; Wi-Fi alone needs a large share
while active):

| Item | Measured (X4 profile) | Technique |
|------|-----------------------|-----------|
| Framebuffer | 11.7 KiB | Bands of 100 rows; the panel controller keeps the frame (ADR 0012) |
| Glyph cache | 0 | Bitmap font packs read in place from flash |
| Kernel, registries, settings, key maps | ≈ 9 KiB | Sorted-vector maps (`VecMap`), capacity released after registration |
| Translations | ≈ 4 KiB | Catalogs index into built-in text; inactive languages unloaded |
| Open book (EPUB) | ≈ 1 KiB + 12 B per chapter | Package cached on storage; ZIP index of 8 B per entry |
| Folder listing | one page | Keyset pagination over the directory, never the whole folder |
| Table of contents | one page | Read entry by entry from the cache |
| ZIP inflate (while converting) | 10.5 KiB + ≤ 32 KiB | Decoder state + window no larger than the entry |
| XML tokenizer | ≤ 4 KiB | Text emitted early, overlong markup skipped |
| Wi-Fi + TLS | on demand only | Freed when sync is done |

Totals (X4 profile, heap budget 192 KiB): home screen 34 KiB, reading TXT
or EPUB 38 KiB, peak 91 KiB while converting an EPUB chapter with images
for the first time (was 132 / 155 / over budget before the optimizations of
2026-10-14; images, setting groups and the plugin manager added since).

Rules:

- **Streaming everywhere**: never load a whole file, chapter, folder or
  table of contents into RAM; memory must not grow with the size of books
  or libraries.
- **Precompute, then cache**: conversions and indexes go to storage
  (`.quireink/plugins/<id>/…`). Page turn = read one locator + lay out one
  page.
- **Draw only in `draw`**: screens may be drawn in bands, several times
  per frame.
- **No hidden allocation in hot paths**; big buffers are allocated
  fallibly, so running out of memory is an error message, not a crash.
- The emulator enforces the same heap cap (see §9.2); `--heap-report`
  shows where the memory goes.

Still open, to verify on the device (M4):

- **Fragmentation.** The emulator counts bytes, not free blocks. The 32 KiB
  inflate window needs one contiguous block; if the device allocator
  fragments over long sessions, reserve a scratch region for large,
  short-lived buffers.
- **Stack.** Not measured by the emulator. Inflate state (10.5 KiB) is
  briefly built on the stack before it moves to the heap.
- **Real budget.** 192 KiB is a conservative guess for the ESP32-C3 heap.

---

## 6. User interface

The kernel draws the complete UI, so it looks and behaves the same on every
device and is designed for e-ink from the start. But the kernel does not
*define* the UI: every screen is a plugin contribution.

### 6.1 Slots: every screen is replaceable

The UI is organised in **slots**: named roles that are bound to a screen
contributed by some plugin:

| Slot | Default binding (first-party plugin) | Examples of alternatives |
|------|--------------------------------------|--------------------------|
| `core.boot` | open last book, else `core.home` | always home, a specific collection |
| `core.home` | `ui-home.dashboard` | plain file browser, "bookshelf", minimalist "one book" screen, stats dashboard |
| `core.library` | `ui-files.browser` | tag/collection browser, Calibre view, series view |
| `core.reader` | `ui-reader.paged` | scroll mode, RSVP speed reader, comic panel view |
| `core.reader-menu` | `ui-menu.overlay` | radial menu, bottom bar, button-only menu |
| `core.settings` | `ui-settings.tree` | simplified settings for beginners |
| `core.sleep` | `ui-home.cover-sleep` | clock, quote, custom image, calendar |
| `core.keyboard` | `ui-input.keyboard` | other layouts, T9 for button devices |
| `core.dialog` | `ui-dialog.default` | none yet |

Bindings are configuration (`[slots] core.home = ui-files.browser`). Plugins
may add **new slots** (e.g. a dictionary plugin adds `dictionary.popup`, which
another plugin may then replace).

### 6.2 Declarative layouts and widgets

Screens such as the home dashboard are **layouts**: trees of widgets described
in data files, so users can rearrange them without programming:

```text
home.layout:
  column
    widget ui-home.continue-reading   size=large
    widget ui-home.recent-books       rows=1
    row
      widget ui-home.collections
      widget stats.reading-time        # contributed by a community plugin
```

- Every widget type is a contribution (`ui-home.recent-books`, `stats.reading-time`, …).
- Multiple home pages (swipe or button to switch) are just multiple layouts.
- The layout file syntax is decided in an ADR before M2 **[OPEN: tree syntax]**.

### 6.3 Data and presentation are separated

Replaceable screens are only possible if the data behind them is *not* owned
by a screen. Therefore the library is a **service** (index, metadata,
collections, reading state, covers, recent books) with a query API. The file
browser, the dashboard and any community "bookshelf" are just different views
of the same service.

### 6.4 Interaction rules (defaults, all configurable)

- Works with **buttons only** (X4) *and* touch: every widget is focusable and
  navigable with Up/Down/Confirm/Back.
- Default tap zones: left third = previous, right third = next, centre = menu.
- Max two levels deep from the reader; every screen has an obvious "back".
- Large targets, high contrast, no animations, no spinners that force refreshes.
- Partial refresh by default, configurable full refresh every *n* pages.
- UI scale and a "night mode" (inverted) as first-class settings.

### 6.5 Themes

No widget hard-codes visual values. Fonts, sizes, spacing, borders, corner
radii, icon sets, accent colour and refresh preferences are **design tokens**
from the active theme (data). Themes can inherit from other themes and be
overridden per screen.

### 6.6 Languages

- UI languages at launch: **English** (built-in fallback) and **German**.
- Translations are catalog files in the config format (`i18n/<lang>.ini`), one
  per language and plugin; adding a language means adding files, not code.
- Lookup goes through a fallback chain (e.g. `de-AT` → `de` → `en` → key), so a
  missing string never breaks the UI.
- Plural rules are named in the catalog (`plural = one-other`); new rule
  families are a few lines in `quireink-i18n`.
- CI checks every catalog against English (missing/unknown keys, placeholders).

### 6.7 Settings structure

The generated settings screen is a flat list in registration order: fine
for a dozen settings, confusing for many. Planned, keeping it generated
from contributions so every plugin benefits automatically:

- **Groups and sub-pages:** settings declare a group (`core.group.reading`,
  `display`, `controls`, `device`, `language`, or a plugin's own group).
  The main page lists groups; groups open sub-pages. Groups are
  contributions with a title, an icon and an order, so plugins can add
  their own and users can reorder them.
- **Levels:** each setting is *basic* or *advanced*; the basic view shows
  only what most readers need (font size, night mode, language, button
  layout), the advanced view everything (refresh interval, long-press
  time, diagnostics).
- **Descriptions and units:** a short help text per setting and units
  rendered by the UI ("11 pt", "6 pages") instead of in the title.
- **Better editors per kind:** sliders for ranges, a picker overlay for
  choices with many options (languages, fonts, key map presets), and the
  on-screen keyboard for text (library folder).
- **Live preview:** reading settings preview on a sample paragraph.
- **Search** on devices with a keyboard or touch.
- **Contributed settings pages:** a plugin may replace the generated page
  for its group with its own screen (e.g. Wi-Fi with scanning).

Status: ✅ groups as contributions (`add_setting_group`; reading,
display, controls, library, system, other), basic/advanced levels with a
switch on the main page, descriptions shown below the list, units with
plural forms, a chooser list for choices and languages, `−`/`+` buttons
for numbers (a first tap only selects), text settings on the on-screen
keyboard (slot `core.keyboard`). ⬜ sliders, live preview, search,
contributed pages.

---

## 7. Extensibility

> **Design rule #2: everything that varies between users is a contribution,
> identified by a namespaced id, and can be added, replaced, reconfigured or
> removed.**

### 7.1 What was wrong in the first draft

| First draft | Problem | Now |
|-------------|---------|-----|
| Fixed set of screens in the core | Home/library could not be replaced | Screens are contributions bound to slots (§6.1) |
| `Action` as a closed enum | Plugins could not add commands | Actions are namespaced ids in a registry |
| Shells translate keys to actions | Key maps differ per shell, not user-editable everywhere | Shells send raw input; the kernel owns layered key maps |
| Three plugin "levels" with different APIs | Three APIs to keep stable, uneven power | One plugin model; code is optional and has two backends |
| Plugins only at a few "extension points" | Everything else was fixed | Uniform registries + services + events + filters |
| Library index inside the core/file browser | Alternative home screens had no data | Library is a service (§6.3) |
| Built-ins could use internal APIs | SDK would lag behind built-in features | Built-ins depend on the SDK only (§4.1) |

### 7.2 One plugin model

A plugin is a **manifest** plus optional **code**:

```ini
# plugin.ini
[plugin]
id = stats
name = Reading statistics
version = 1.2.0
sdk = 0.1                 # required SDK version (SemVer compatible)
requires = ui-home >=0.1  # optional dependencies on other plugins
permissions = storage.plugin, events.reading

[contributes]             # declarative contributions, no code needed
translations = i18n/
keymaps = keymap.ini
settings = settings.ini
layouts = layouts/
```

| Part | Tier S (ESP32-C3) | Tier M/L |
|------|-------------------|----------|
| Manifest + declarative contributions (layouts, themes, key maps, settings schemas, translations, fonts, CSS) | ✅ loaded from SD card at runtime (data plugins: translations, key maps, setting defaults ✅; fonts, CSS, layouts ⬜) | ✅ |
| Native code (Rust crate using the SDK) | ✅ compiled into the firmware (selected per build) | ✅ compiled in |
| WebAssembly code (any language, sandboxed via `wasmi`, fuel-metered) | ❌ | ✅ installed at runtime |

Native Rust plugins are never loaded dynamically (`.so`/`.dll`): Rust has no
stable ABI, and WASM is the safer runtime format. For Tier S, a **web firmware
builder** produces firmware with the user's choice of native plugins
(ESP Web Tools + CI-built variants).

### 7.3 Building blocks

| Block | What it is | Examples |
|-------|-----------|----------|
| **Contributions** | Named things registered in kernel registries | actions, screens, slots, widgets, layouts, themes, key maps, gesture maps, settings, menu entries, status-bar items, formats, fonts, hyphenation, translations, sleep screens |
| **Services** | Versioned interfaces provided by one plugin and consumed by others | `library` (index, metadata, collections), `reading-state`, `bookmarks`, `dictionary`, `sync`, `network`, `book-source` |
| **Events** | Notifications a plugin can subscribe to | `book.opened`, `page.turned`, `book.finished`, `library.changed`, `device.sleep`, `settings.changed` |
| **Filters** | Ordered chains that may inspect and modify data | input before key-map lookup, document content before layout, display list before rasterizing (overlays, highlights), refresh decisions |

Every contribution can be **overridden** (higher priority or an explicit user
binding wins) or **disabled** (`[disable] ui-menu.entry.toc = true`), so
removing things is as easy as adding them.

### 7.4 Input routing

```text
raw input ─▶ input filters ─▶ focused widget ─▶ key map layers ─▶ action ─▶ handler
                                                (screen → slot → global)
```

- Key maps are layered: *user* overrides *device* overrides *plugin defaults*.
- Maps bind buttons, long presses, button chords, tap zones (3×3 grid or
  custom rectangles) and gestures to **action ids**.
- Any plugin may contribute actions; users can bind any action to any input.

### 7.5 Reading pipeline: every stage is replaceable

```text
book source ─▶ format decoder ─▶ content events ─▶ content filters ─▶ layout
 (VFS, OPDS)    (format-*)                         (CSS overrides,     (line breaker,
                                                    typography,         fonts)
                                                    hyphenation)
   ─▶ display list ─▶ overlays ─▶ rasterizer ─▶ refresh policy
      (per page)      (highlights, annotations,   (per device / screen)
                       header/footer)
```

### 7.6 SDK rules that keep this maintainable

1. **Namespaced ids everywhere** (`plugin-id.name`); `core.*` is reserved.
2. **SemVer for the SDK**; plugins declare the SDK version they need. Until
   1.0, breaking changes are allowed but batched and documented.
3. **Message-friendly APIs:** every SDK call must be expressible over a
   message boundary (serializable arguments, no borrowed host data kept across
   calls), so the WASM backend offers the same power as native plugins.
4. **Zero cost when unused:** hooks run at coarse granularity (per event,
   per page, per screen), never per glyph or per pixel.
5. **Fault isolation:** WASM plugins are sandboxed and fuel-limited; a
   misbehaving plugin is disabled, not the device. **Safe mode** (hold a
   button at boot) starts with first-party plugins only.
6. **Permissions** are declared in the manifest and granted by the user
   (network, storage outside the plugin's namespace, reading data).
7. **Per-plugin storage namespace** for data and settings
   (`.quireink/plugins/<id>/`).
8. **Shareable setups:** a "setup" bundles configuration, layouts, themes and
   a plugin list, so users can share complete customisations.

### 7.7 Feature check: can it be built as a plugin?

The architecture is validated against features e-reader users commonly ask
for. Each must be buildable as a plugin **without changing the kernel**.
Where the first draft could not support a feature, the gap was added to the
architecture (G1 to G12 below).

| Feature users want | Built from | Gap |
|--------------------|-----------|-----|
| Dictionary lookup on long-press, Wikipedia, translation | gesture on page → hit test → word → slot `core.lookup` (overlay); ✅ StarDict dictionary plugin (`quireink-stardict`: block index, dictzip, synonyms), word choice with buttons; Wikipedia/translation can bind the same slot | G1, G5 |
| Highlights, notes, annotation export (Markdown, Readwise, Obsidian) | selection + locators; `annotations` service; display-list overlay filter; network/storage. ✅ bookmarks and highlights at source positions (`PageLine::words`), selection by touch or buttons, the selection bar as menu `reader.selection` (plugins add actions and get `reader.text-chosen`; what does not fit goes into a *More* list) · ⬜ notes (needs the on-screen keyboard), export | G5 |
| Footnote popups, internal links | link hit test; overlay | G1, G5 |
| Reading statistics, streaks, goals, time-left estimates | `page.turned` events with timestamps; plugin storage; home widgets | G4 |
| Quick settings while reading | ✅ any menu as a panel of controls (`core.panel`: switches as buttons, numbers with − and +, small choices as segments, pages when long); the reader's font and layout open this way (tap at the bottom) and are edited like any menu | G1 |
| Status lines with time, battery, chapter, page | ✅ status items as contributions (`Registrar::add_status_item`), lines above and below the text with left/centre/right parts chosen in the settings, redrawn only when what they show changes | G2 |
| Font choice, own fonts, publisher's formatting | ✅ reading font by family (`core.reading-font`; bold and italic faces follow), font files from a folder (`KernelBuilder::font_dir`; Android: `QuireInk/fonts` and KOReader's `koreader/fonts`) or a data plugin's `fonts/`, read from their header at start and loaded only when chosen; `reader.publisher-styles` shows books without their CSS, embedded fonts (`@font-face` with TrueType/OpenType, IDPF and Adobe font obfuscation; `reader.embedded-fonts`, only where scalable fonts render) · ⬜ WOFF fonts, font sizes from CSS | G2 |
| Custom fonts, margins, spacing, hyphenation, justification | font/hyphenation contributions; reader settings | none |
| Per-book / per-publisher style tweaks | content filters; **per-book settings** | G3 |
| Scroll mode, RSVP speed reading, two-page spreads | alternative reader screen in `core.reader` reusing the layout engine | G8 |
| Comics/manga: panel zoom, right-to-left | fixed-layout reader screen; RTL key map layer | G8 |
| Rotation / landscape | runtime display reconfiguration | G7 |
| Custom home screens, shelves, dashboards | slots, layouts, widgets, library service | none |
| Collections, tags, series, smart shelves, metadata editing | library service with plugin-defined metadata fields and metadata providers | G10 |
| Library from Calibre / OPDS / Wallabag / RSS / web articles | `book-source` contributions; background jobs; network | G4 |
| Wireless transfer ("send to device" from a browser) | HTTP server provided by the shell as a platform service | G2 |
| Progress sync (KOReader, Readest, …), Goodreads/StoryGraph updates | `sync` service; events; network | G4 |
| Full-text search in book and library | search service; index jobs. ✅ search in the book (background scan, hits with snippets, the hit underlined on its page), go to page · ✅ library search (title, author, series, tags) | G4 |
| Sleep screen: cover, clock, quote, calendar | `core.sleep` slot; power events | G7 |
| Frontlight / warmth control and schedules | frontlight platform service; timers | G2, G4 |
| Gestures: swipes, multi-finger, corners, button chords | raw pointer stream; gesture recognizers as contributions | G6 |
| Pen notes, handwriting, sketching (reMarkable, Boox) | pointer capture with pressure; fast refresh; ink layer | G6 |
| Text-to-speech, audiobooks sync | audio/TTS platform service | G2 |
| Bluetooth page turners | shell maps them to named buttons; on Tier L input-source contributions | G2 |
| Multiple users, parental lock | settings scopes + storage namespaces per profile | G3 |
| Mini apps: notes, calendar, to-do, games (sudoku, chess) | screens + storage + home layout entries | none |
| Accessibility: large UI, high contrast, screen reader | theme tokens; **widget semantics tree** | G11 |
| Plugin store, enable/disable, updates | plugin management service + UI plugin | G9 |
| Backup/restore and sharing of setups | settings serialization; setups | G9 |
| Language learning: interlinear translations, vocabulary trainer | content filters; dictionary events; storage | none |
| Plugins extending other plugins (e.g. status-bar items for the reader) | **plugin-defined extension points** | G12 |

#### Gaps and how they are closed

| # | Gap | Architecture change | When |
|---|-----|--------------------|------|
| G1 | Only full screens existed | **Layers**: popups, dialogs, menus, toasts and keyboards are partial-screen surfaces stacked above screens (also slot-bound). Without a compositing buffer, the screen below redraws only the uncovered region. | M2 ✅ (ADR 0010) |
| G2 | Hardware features had no plugin API | **Platform services**: shells register services (frontlight, battery, network, HTTP server, audio/TTS, Bluetooth, USB) through the same service registry plugins use. | Storage ✅ M1 (ADR 0009), clock and battery ✅ (`Kernel::set_status`, `Env::status`, time zones as POSIX rules in `sdk::time`) / rest M2 to M6 |
| G3 | One global settings level | **Settings scopes**: global → device → user profile → collection → book; the most specific value wins. | Book ✅ (`SettingSpec::scoped`, `Command::SetScope`, "Only this book") / M3 |
| G4 | No time, no background work | **Time + jobs**: every kernel call carries a timestamp, `WakeAt` effects drive timers (M0); screens run background steps with `Cx::wake_at` and `on_timer`, events via `Command::Emit` + `Registrar::subscribe` (M1); job progress and cancellation; network requests as effects. | M0 ✅ / M1 ✅ / M5 |
| G5 | Rendered pages were opaque | **Content access**: display lists keep a mapping to content locators, so plugins can hit-test words and links, select text and decorate ranges. | M3 |
| G6 | Shells pre-interpreted input | **Raw input**: shells send button down/up and pointer samples; the kernel recognizes long presses and gestures; screens can capture pointers (done in M0). Gesture recognizers become contributions. | M0 ✅ / M5 |
| G7 | Display and power were static | **Lifecycle events**: rotation/resolution changes, suspend/resume, low battery, charging, USB. | M4 |
| G8 | Reader was one monolith | Reader split into a **layout engine** (`quireink-layout`, usable by every reader plugin) and interchangeable reader screens. | M1 ✅ |
| G9 | Plugins could not be managed at runtime | **Plugin management service** (list, enable, disable, install, update) and config write-back. | Config write-back ✅ M2 / list, enable, disable, restart ✅ M5 / install, update M5 |
| G10 | Fixed library schema | Library service with **plugin-defined metadata fields** and metadata providers; the library service itself is replaceable. | M2 to M5 |
| G11 | Widgets were pixels only | **Widget semantics** (role, label, state) for accessibility, automation and tests. | M2 |
| G12 | Only the kernel could define extension points | **Plugin-defined extension points**: a plugin publishes a service through which others contribute; a second init phase (`activate`) runs after all plugins registered. | M5 |

**Rule derived from this check:** before a feature is added to a first-party
plugin, ask whether a community plugin could build the same feature with the
public SDK. If not, the SDK is extended first.

---

## 8. File formats

Formats are plugins; availability depends on tier.

| Format | Tier S | Tier L | Notes / library candidates |
|--------|:------:|:------:|----------------------------|
| TXT (UTF-8, legacy encodings) | ✅ | ✅ | First format, proves the pipeline |
| EPUB 2/3 (reflowable) | ✅ | ✅ | Streaming ZIP + SAX + CSS subset |
| FB2 / FB2.zip | ✅ | ✅ | XML, similar pipeline to EPUB |
| Markdown | ✅ | ✅ | `pulldown-cmark` (no_std-capable **[VERIFY]**) |
| HTML (single file) | ✅ | ✅ | Shared with EPUB |
| XTC/XTCH (Xteink native) **[VERIFY]** | ✅ | ✅ | Pre-rendered pages, compatibility with stock firmware |
| CBZ (images) | ⚠️ later | ✅ | Strip-wise JPEG/PNG decoding on Tier S |
| MOBI / AZW3 (DRM-free) | ⚠️ | ✅ | Convert to internal model |
| RTF, DOCX, ODT | ❌ | ✅ | Later |
| PDF | ❌ | ✅ | `pdfium` (permissive) or MuPDF (AGPL, separate plugin) |
| DjVu | ❌ | ✅ | DjVuLibre (GPL → separate plugin) |
| CBR | ❌ | ✅ | unrar license is restrictive → evaluate alternatives |

### 8.1 Text rendering

Decided in [ADR 0008](adr/0008-fonts-outlines-and-bitmap-packs.md); status
after M1 step 1:

- ✅ **One `Font` type, two backends:** runtime outlines via `skrifa` with
  a built-in 4-bit anti-aliasing rasterizer and an LRU glyph cache (Tier M/L),
  and pre-rendered **bitmap font packs** (`.qifp`) read in place from flash
  (Tier S: `skrifa` cannot run on the ESP32-C3 because it needs atomics).
- ✅ `tools/fontgen` and the `fonts-default` build script produce packs with
  the same rasterizer; a test guarantees identical pixels.
- ✅ Fonts are contributions bound to roles (`core.font-ui`,
  `core.font-reading`), user-configurable in `[fonts]`.
- ✅ Default fonts: Literata (reading), Atkinson Hyperlegible Next (UI).
- ✅ Greedy line breaking (spaces, hyphens, overlong words).
- Next: Liang hyphenation from compact tries, kerning/shaping (HarfRust) on
  Tier L, optional Knuth-Plass line breaking, optical-size axis per size,
  pack compression.
- CJK on Tier S: subset packs on the SD card **[evaluate]**.
- Avoid `ttf-parser` (unmaintained, RUSTSEC-2026-0192).

---

## 9. Emulator and testing

### 9.1 Desktop emulator (`shells/desktop`)

- Window via `winit` + `softbuffer` (no SDL dependency, works on Windows).
- **Device profiles**: resolution, DPI, pixel format (incl. colour panels),
  rotation, input type,
  RAM budget, e.g. `xteink-x4`, `kobo-clara-bw`, `kindle-pw5`, `boox-note`.
- **E-ink simulation**: exact grayscale quantization of the target, visible
  partial vs. full refresh, configurable refresh latency, optional ghosting.
- Input: keyboard maps to device buttons; mouse = touch/tap zones.
- Debug overlay: dirty regions, heap usage, layout timings.

### 9.2 Memory budget enforcement

A counting global allocator with a hard cap (e.g. the X4 profile's heap budget)
and peak tracking. Running the emulator with `--profile xteink-x4` fails
exactly where the device would run out of memory.

### 9.3 Automated tests (CI)

| Kind | What |
|------|------|
| Unit tests | Parsers, layout, locators, config |
| Snapshot tests | Headless rendering of sample books → PNG, compared with golden images per device profile |
| Input replay | Scripted action sequences (`open book → next ×10 → menu → toc`) |
| Fuzzing | `cargo-fuzz` on ZIP, XML, EPUB, FB2 parsers (untrusted input!) |
| Build matrix | `riscv32imc-unknown-none-elf`, `wasm32`, `armv7-unknown-linux-musleabihf`, `aarch64-linux-android`, desktop |
| Size/RAM report | Firmware size and peak heap per PR, regressions flagged |
| ESP32 smoke test | Espressif QEMU (ESP32-C3) or Wokwi in CI **[evaluate]**; display not emulated, boot + logic only |

### 9.4 Web demo

The `web` shell compiles the same core to WASM → an online demo on GitHub
Pages ("try QuireInk in your browser"), also handy for UI reviews.

---

## 10. Platform shells in detail

### 10.1 ESP32 (Xteink)

- `esp-hal` + Embassy (async) inside the shell; core stays synchronous.
- SPI display driver, button GPIO/ADC, SD card (FAT via `embedded-sdmmc`),
  battery ADC, deep/light sleep between interactions.
- Wi-Fi via `esp-wifi`/`esp-radio` **[VERIFY current crate]**, only on
  demand: the radio is the largest consumer after the display, so it is
  off while reading. The shell switches it on for an HTTP request or a
  server (transfer, sync, catalogs, browser), keeps it on for a short idle
  time (e.g. 30 s, a setting) so a browsing session or a sync does not
  reconnect for every request, and switches it off afterwards; a server
  keeps it on only while its screen is open. `Env::network` then means
  "can connect", not "is connected". Background work that needs the
  network (rules, OPDS sync) runs only when asked or on wake-up, never by
  keeping the radio on. Battery use is measured on the device before 1.0.
- Flashing: `espflash`; OTA updates later.
- One shell for the X4 (ESP32-C3, RISC-V, upstream Rust target) and the X4
  Pro (ESP32-S3, Xtensa, `esp` toolchain via `espup`): chip-specific code
  (display, touch, frontlight, pogo-pin charging) behind features. CI builds
  the S3 variant once the toolchain is set up there.
- The X4 Pro adds touch (I²C controller **[VERIFY]**) and a frontlight
  (PWM, warm/cool), both offered to plugins as platform services. With
  PSRAM, scalable fonts and larger caches become possible; without it, the
  X4's memory techniques apply unchanged.
- QuireInk **replaces** the stock firmware (like CrossPoint); flashing tools
  must back up the original firmware and allow restoring it.

### 10.2 Linux framebuffer (Kobo, Kindle, PocketBook, reMarkable)

- **Fully static musl binaries** (`armv7-unknown-linux-musleabihf`, `aarch64`)
  → sidesteps old-glibc issues on Kindle.
- E-ink refresh: own Rust module for the vendor ioctls (mxcfb, sunxi, …);
  FBInk serves as documentation only, it is not linked (§3.2).
- Input via `/dev/input/event*` (evdev), per-model rotation and button maps.
- Device database (model → screen, rotation, input quirks, suspend method),
  modelled after KOReader's device layer.
- Power: suspend/resume and Wi-Fi handling per device.
- Launchers: KFMon/NickelMenu (Kobo), KUAL (Kindle), app menu (PocketBook),
  Toltec/launcher (reMarkable).

### 10.3 Android (Boox & co.)

- `android-activity` (NativeActivity/GameActivity) → no UI in Kotlin.
- Thin JNI shim only for vendor SDKs (Onyx EPD refresh modes).
- Built with `cargo-ndk`; APK via Gradle wrapper; F-Droid-compatible.

---

## 11. Data and configuration

- Plain, human-editable config files on storage (`quireink.cfg`), parsed by our
  own tiny no_std parser; users can edit settings on the SD card.
- Per-book data next to the library: `.quireink/books/<hash>/` containing
  progress, bookmarks, highlights and pagination caches.
- Book identity = KOReader-compatible partial MD5 → sync compatibility.
- Library index built incrementally and cached; rebuilt on demand.

---

## 12. Roadmap

If the X4 has not arrived by the end of M3, M6 (Boox) is done before M4.

| Milestone | Content | Exit criteria |
|-----------|---------|---------------|
| **M0 Foundation** ✅ | Workspace, CI, license, ADRs, platform vocabulary, config + i18n crates, SDK 0.1 with plugin/registry/slot/action/key-map model, raw input + gesture recognition, timers, kernel, emulator with device profiles and heap cap, `diagnostics` plugin | Emulator shows the diagnostics screen contributed by a plugin; CI green on all targets |
| **M1 Plain text** ✅ | Fonts, rasterizer, bitmap packs + `fontgen`, line breaking, storage service, document model, layout engine, TXT plugin, reader (progress, background page index, reopen last book), file browser, events, timers | ✅ A 5.3 MB TXT reads in the X4 profile within the heap budget (peak 159 KiB of 192 KiB); index of 18,844 pages built in the background |
| **M2 UI** 🔶 | ✅ home screen (continue reading, recently read, shortcuts as menu `home.actions`), standard reading events `core.book-opened`/`core.page-turned`, key map presets (two-button layout), device setting defaults, `core.next`/`core.previous`, overlays, menus as contributions (+ `[menus]`; edited on the device with the `menu-editor` plugin, which keeps the way to the settings open), generated settings screen, settings saved to `quireink.ini`, screen events, `ui` building blocks, `core.ui-size`, reader menu with live re-layout · ✅ settings structure (§6.7: groups, levels, descriptions, units, choosers) · ✅ library: covers, titles and authors from the books (read in the background for visible rows only, cached per book), reading progress, order by recently read or name, folders as collections, library folder chooser · ⬜ themes beyond `core.ui-size`, layout files for dashboards, library service, widget semantics, snapshot tests · ✅ on-screen keyboard (`core.keyboard`, letters per language, buttons or touch), headers with back and page arrows and a pager under long menus on touch screens | Full button-only flow ✅ (also with two buttons); home screen replaceable by config ✅; snapshot tests ⬜ |
| **M3 EPUB** 🔶 | ✅ streaming ZIP (`quireink-zip`), incremental XML tokenizer (`quireink-xml`), EPUB 2/3 package, spine, metadata, TOC (nav + NCX) with anchors, chapter conversion into a block cache (ADR 0011), TOC screen + `reader.goto`, `DocumentError::OutOfMemory`, hyphenation (Liang patterns read in place, `quireink-hyphen`; German and English as the `hyphenation` plugin; contribution `add_hyphenation`; setting `reader.hyphenate`), styled text: bold/italic runs (`<b>`, `<i>`, `<em>`, `<strong>`, inline `style`), alignment (`<center>`, `align`, `text-align`), headings in a larger bold face that never end a page, Literata bold/italic faces, CSS subset (element and class selectors from the book's stylesheets and `<style>`: `text-align`, `font-weight`, `font-style`, `display: none`; parsed once into the cache) , images (`quireink-image`: streaming baseline JPEG and PNG decoded to gray, cached, scaled and ordered-dithered when drawn), mutation tests and cargo-fuzz targets for every parser covers (`Document::cover`, decoded once to a small gray image) · ⬜ progressive JPEG, GIF, more CSS (margins, font sizes, small caps), cover on the home screen | Standard Ebooks / Gutenberg test corpus renders correctly; Gutenberg EPUBs within the X4 budget (✅ peak 115 of 192 KiB with banded rendering, ADR 0012) |
| **M4 Xteink X4** | ESP32 shell: display, buttons, SD, sleep, firmware backup/restore | Read an EPUB on real hardware; battery measurement |
| **M5 Plugins v1** 🔶 | ✅ plugin manager (`plugin-manager` plugin: every plugin with translated name, description, version and state; switches saved to `[plugins]`, applied by `Command::Restart`; plugins that alone provide a standard slot or all fonts stay on), data plugins from storage (`plugin.ini`, translations, key maps, setting defaults), progress sync with KOReader (`kosync`), OPDS catalogs (`opds`: browse, search, download, sync folders with catalogs) · ⬜ SDK 0.1 published + docs, fonts and CSS in data plugins, events/filters/services complete, web firmware builder (X4) | Progress syncs with KOReader; a sample community plugin lives outside the repo |
| **M6 Boox** 🔶 | ✅ native activity in Rust without Gradle; its only Java code is the speech listener (`shells/android`, `build-apk.sh`, APK as CI artifact), touch, page/volume keys, sample books, system language, shared `quireink-app` plugin set and `quireink-fs` storage, Onyx e-ink waveforms (`View.setDefaultUpdateMode`), frontlight (`DeviceController`), shared storage with any library folder, data plugins in `QuireInk/plugins`, app icon · ⬜ colour rendering path | APK runs on Go 7 (mono) ✅ (2026-10-14: library, EPUB, page keys, swipes, reopening the last book) and Air 6C (colour) ⬜ |
| **M7 Kobo** | linux-fb shell, device DB, touch, frontlight, suspend | Daily-driver usable on one Kobo |
| **M8 Expansion** | Runtime WASM plugins, PDF/CBZ/…, Kindle, PocketBook, reMarkable, web demo | none yet |

### 12.1 Customisable before the first release

Goal: before 1.0, everything a reader might want different can be changed,
in the settings or by a plugin. Each item names the building block that
makes it replaceable. ✅ done, 🔶 partly, ⬜ open.

| # | What | Building block | State |
|---|------|----------------|-------|
| C1 | Every screen (home, library, reader, menus, keyboard, settings, browser) | Slots | ✅ |
| C2 | Formats, fonts, hyphenation, translations, key maps, menus | Contributions, data plugins | ✅ |
| C3 | Status lines: own text size; whole lines from plugins; a tap switches between the chosen ones, above and below the text | `add_status_line`, `add_status_item` | ✅ |
| C4 | Short messages any plugin can show the same way ("Sync done"), how long they show | `Command::Notify`, slot `core.notice` | ✅ |
| C5 | Rules: "every day at 22:00: night mode on", "when QuireInk starts: sync the catalogs"; when: start, time of day, book opened, waking up; then: any action, any switch on or off | `rules` plugin, `Command::Schedule` (timers for plugins) | ✅ (more triggers, e.g. Wi-Fi, once the shells report them) |
| C6 | Reading profiles: named sets of settings ("Night", "Commute", "Large print"), switched with a tap, a button or a rule | `profiles` plugin (reading, display and status line settings; action "Next profile") | ✅ |
| C7 | Themes: menu text size, margins, row height, lines, dark or light headers; icon packs (icons are a font role) | Settings group "Appearance"; themes are data plugins with setting defaults (sample `samples/plugins/airy-theme`) | 🔶 (gray levels, corners and an icon pack sample open) |
| C8 | Menus as tiles with their icons besides lists | Setting `menu.style` (Appearance); other looks by replacing the `core.menu` slot | ✅ |
| C9 | Sleep screen: cover, own picture, calendar, statistics | Slot `core.sleep`, action `core.sleep` (power button), `Effect::Sleep` for shells that sleep themselves | ✅ (`sleep` plugin: the book's cover, own pictures, widgets as on the home screen (e.g. today's reading), a message, the page as it is) |
| C10 | Text filters between format and layout: bold word beginnings, replaced words; a style sheet per book, fonts for dyslexia by plugins | `Registrar::add_text_filter`, `Block::replace`; `aids` plugin | ✅ (own stylesheets for EPUB books: `styles.css` in a book's folder for all its books, `<book>.css` beside a book for it alone, applied after the book's own; a changed one applies at once) |
| C11 | Footnotes over the page or jumped to; note marks recognised by their text (1, [2], *) | Setting `reader.footnotes`, screen `reader.footnote`, event `reader.follow` | ✅ |
| C12 | Looking up beside StarDict: Wikipedia summaries online; translators or online dictionaries by plugins | Slot `core.lookup-online` (offered in the dictionary when bound and online), `wikipedia` plugin, `quireink_sdk::json` | ✅ |
| C13 | Reading aloud | `Command::Speak` to shells that speak (Android: the system's TextToSpeech); the reader reads sentence by sentence, underlined, with speed, page turning and a timer; menu entries and settings with a `MenuNeed` (`Speech`, `NoTouch`) show only where they work | ✅ (the voice is chosen in the system's speech settings, opened by the setting "Voice" (`android.voice`)) |
| C14 | Library order: recently read or by name, folders as collections, shelves by author, by series (also EPUB 3 and Calibre series) and by tag (`dc:subject`, where Calibre puts its tags) | `files` plugin: catalog made in the background, submenu `files.shelves` with `files.by-author`, `files.by-series`, `files.by-tag` | ✅ |
| C15 | Export of highlights and notes: Markdown in `Notes` beside the library; Obsidian, Joplin, Readwise, KOReader by plugins | Event `reader.notes-exported` for exporter plugins | 🔶 (Markdown done; exporter plugins open) |
| C16 | More sync: WebDAV folders (Nextcloud, ownCloud) browsed and synced like catalogs; Calibre wireless, Syncthing | `opds` plugin (catalogs of kind WebDAV, `PROPFIND`); Android sends any HTTP method | 🔶 (Calibre wireless needs sockets in the kernel) |
| C17 | Buttons, held buttons, tap zones, held taps and swipes changed on the device, everywhere or while reading; a left-handed layout | `keys` plugin, `Command::Bind`, `Env::keys`, action `core.nothing`, preset `core.left-handed` | ✅ (custom zone shapes open) |
| C18 | Home screen of pages of widgets: status line (time, battery), date, continue reading, recent covers, a grid of tools, a large clock, today's reading with a daily goal, the last 7 days, news headlines, a word of the day, any plugin's; swiped or paged through, edited on the device (pencil or long press; with buttons too) | `Registrar::add_widget` with targets (`WidgetTarget`), settings `home.page-1` to `home.page-4`, menu `core.tools` | ✅ |
| C19 | OPDS: covers in lists (devices with the memory), a folder per catalog, automatic sync (rules), OPDS 2 (JSON) | `opds` plugin, C5, C20 | ✅ (OPDS 2: JSON catalogs read as they stream, navigation, publications, groups, covers, search templates, next pages) |
| C20 | Large-memory devices may use more (covers in lists, caches) without separate code | `DeviceInfo::memory` (`MemoryClass::Small`/`Large`) | ✅ |

### 12.2 Ideas from KOReader and CrossPoint / CrossInk

Looked at on 2026-10-15: what the other readers offer that QuireInk does
not yet, and ideas of our own (U). Each idea stays simple by default and
replaceable by plugins.
⬜ open, ✅ done.

| # | Idea | Seen in | State |
|---|------|---------|-------|
| I1 | Time left in the chapter and the book, from one's reading speed, as status items | KOReader | ✅ (`stats.status.left-chapter`, `stats.status.left-book`) |
| I2 | A slider to skim through the book (chapter marks on it), back to where one was with one tap | KOReader (skim widget) | ✅ (More > "Skim through the book": bar with chapters, chapter by chapter, "Where I was", page number) |
| I3 | "Back" after any jump (contents, search, go to page, skimming), not only after links | KOReader | ✅ |
| I4 | Turning pages by itself every few seconds | KOReader (Autoturn), CrossPoint | ✅ (More > "Turn pages automatically", a clock in the status line) |
| I5 | Notes on highlights; highlight styles (marker, underline, strikeout) | KOReader | ✅ (Note in the selection bar, a sign in the margin, in the list and the export; setting "Highlights": marker or underlined) |
| I6 | Book information: description, subjects, series, file, from the library and the reader | KOReader | ✅ (hold a book in the library; tap the title in the reader's menu; slot `core.book-info`) |
| I7 | The printed edition's page numbers (EPUB `page-list`) | KOReader | ✅ (status item "Printed page", `Document::page_label`) |
| I8 | Contents made from the headings where a book has none or a poor one | KOReader | ✅ (found once when the contents open, kept per book) |
| I9 | Vocabulary: words looked up, with their sentence, to review later | KOReader (vocabulary builder) | ✅ (`vocabulary` plugin: every word looked up with its sentence and book (event `core.word-looked-up`), Vocabulary in the main menu: look up again, go to the place, delete, export for Anki) |
| I10 | Translating selected text online, by a plugin | KOReader | ✅ (`translate` plugin with DeepL (own free API key, kept as a secret setting), slot `core.translate` for other services; *Translate* for selected text where there is a network) |
| L1 | Favourites: a star on a book, a shelf of them | KOReader | ✅ (hold a book; "Shelves > Favourites") |
| L2 | The library as a grid of covers | KOReader (cover browser), CrossPoint | ✅ (setting "Show books as": list or covers; `ListView::columns` for any grid) |
| L3 | Searching the library by title and author | KOReader | ✅ (library menu: "Search the library", by title, author, series, tag or file name) |
| L4 | Marking a book finished, moving it to an archive folder | KOReader | ✅ (hold a book: "Mark as finished", "Move to the archive"; the event `core.book-moved` takes progress, notes and statistics along, also for moves in the file manager) |
| F1 | More formats: FB2, Markdown, CBZ, Xteink's own XTC/XTCH, pictures; PDF on Android through the system's renderer | KOReader, CrossPoint | 🔶 (FB2, FB2.zip, CBZ, XTC/XTCH done: `format-epub.fb2`, `format-epub.cbz`, `format-xtc`, Markdown (`format-epub.markdown`), comics and XTC shown edge to edge; PDF on Android through `PdfRenderer` (`android.pdf`, pages rendered to fit the screen, no text yet); pictures as books open) |
| D1 | Sleep screen: the cover, own pictures, minimal, today's reading (C9) | CrossPoint, CrossInk, KOReader | ✅ (cover, pictures from `sleep`, widgets with today's reading, message, page; in the emulator, the X4 firmware sleeps on `Effect::Sleep`) |
| D2 | Sleeping and switching off by themselves after a while | KOReader, X4 firmware | 🔶 (setting "Sleep after" (`core.sleep-after`, 10 min), not while reading aloud, downloading or serving; switching off open) |
| D3 | Turning the screen, also to landscape | CrossPoint | ⬜ |
| D4 | Firmware updates over Wi-Fi (X4) | CrossPoint | ⬜ |
| D5 | Screenshots | CrossPoint, KOReader | ✅ (action `core.screenshot` in the tools, bindable to any button or gesture: a gray PNG in `Screenshots` beside the library, written band by band as the frame is drawn) |
| S1 | Read-later articles (Wallabag), news feeds as books, highlights to Readwise and Joplin (C15), Calibre wireless (C16) | KOReader | 🔶 (news feeds: `news` plugin, RSS and Atom as books in `News`; the rest open) |
| U1 | Typing on a phone: passwords and keys sent from a phone's browser (QR code) into any text field; a store of credentials, replaceable by plugins | own idea | ✅ (keyboard "Phone", slot `core.remote-keyboard` bound by `transfer`; `Env::secrets` and `Command::SetSecret` keep every password and key in `.quireink/secrets.txt`, veiled, not encrypted; *Settings → Passwords and keys* types them anew or removes them; stats, kosync, opds and translate moved there) |
| U2 | Pasting with a long press in a text field | phones | ✅ |

---

## 13. Repository and project hygiene

- `README.md`, `LICENSE`, `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md`,
  `SECURITY.md`, `CHANGELOG.md` (from first release).
- Architecture Decision Records in `docs/adr/` (one file per decision).
- GitHub Actions: `fmt`, `clippy -D warnings`, tests, build matrix, `cargo-deny`
  (licenses + advisories), snapshot tests, size report.
- Conventional Commits; SemVer for releases and for the plugin API.
- Issue/PR templates; labels per tier, shell and plugin.
- All code, comments, docs and commit messages in English.

---

## 14. Risks

| Risk | Impact | Mitigation |
|------|--------|-----------|
| ESP32-C3 RAM too tight (esp. Wi-Fi + TLS + EPUB) | Medium | Budget allocator from M0; banded rendering, streaming and caching (§5: reading needs 31 of 192 KiB); sync in a separate "mode" that frees reader memory |
| Display controller / grayscale details of the X4 unknown | Medium | Study CrossPoint; get hardware early |
| Vendor e-ink quirks per Linux device | Medium | Device DB; learn from KOReader; Kobo first |
| License contamination | High | Decide license first; `cargo-deny`; copyleft only in separate plugins |
| Scope creep ("everything KOReader can") | High | Strict milestones; formats as plugins; tiers |
| Kindle jailbreak availability changes | Low | Kindle is a late target |
| Plugin API churn breaks community plugins | Medium | Keep API small; version it; experimental flag until 1.0 |
| Over-engineering the extension system slows progress | Medium | Structure (ids, slots, registries, SDK boundary) from day one; each concrete extension point is built when the first first-party plugin needs it |

---

## 15. Open questions

1. X4 hardware details: display controller, grayscale, SD card, RAM left for
   apps (clarify via CrossPoint sources and once the device arrives).
2. Android version of the Boox Go 7; exact Boox Air 6C specs (panel, resolution).
3. Tree syntax for layout files (§6.2); decide before M2.
4. Scripting for small user tweaks on Tier L in addition to WASM (e.g. Lua or
   Rhai), or WASM only? Current plan: WASM only.
