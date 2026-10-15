# QuireInk

**A lean, fast and endlessly customizable e-reader for e-ink devices, from a
380 KB microcontroller to Linux and Android e-readers, from one codebase.**

> **Status: early development.** EPUB and plain text books can be read in
> the emulator and on Android e-readers (tested on a Boox Go 7), with a
> table of contents, menus, a settings screen, page index, saved reading
> position and reopening of the last book. See the
> [project plan](docs/PLANNING.md)
> and the [roadmap](docs/PLANNING.md#12-roadmap).

## Why QuireInk?

- **Runs where others can't.** The smallest target is the Xteink X4 with an
  ESP32-C3 and no PSRAM. Everything is designed for that budget, so larger
  devices get a fast, battery-friendly reader for free.
- **Everything is a plugin.** The core is a small kernel. The home screen,
  library, reader, menus, file formats, themes, key maps and translations are
  plugins. First-party ones use exactly the same SDK as community plugins.
  Anything can be added, replaced, reconfigured or removed.
- **Simple by default.** Turn it on and keep reading. Every function is at
  most two steps away, and it works with buttons only or with touch.
- **One codebase, many devices.** Xteink X4/X3/X4 Pro, Kobo, Kindle (jailbroken),
  PocketBook, reMarkable, Boox and other Android e-readers, desktop and web.
  Grayscale and colour e-ink.

## Customization at a glance

A plain text file (`quireink.ini`) on the device changes behaviour without
any programming:

```ini
[core]
language = de
full-refresh-interval = 8

[slots]
# Replace the home screen with any screen a plugin provides.
core.home = diagnostics.screen

[keymap.global]
# Bind any input to any action; `none` removes a binding.
long.page-forward = core.toggle-night-mode
tap.center = none
swipe.up = core.menu

[menus]
# Redefine any menu: which actions, in which order.
reader.menu = reader.font-larger, reader.font-smaller, core.toggle-night-mode, core.home

[plugins]
# Switch any plugin off by its id (or use Settings → Plugins).
some-plugin = off
```

Plugins contribute screens, actions, settings, key maps and translations
through the [`quireink-sdk`](crates/quireink-sdk) crate. See
[docs/PLANNING.md §7](docs/PLANNING.md#7-extensibility) for the full
extension model.

### Plugins without code

A folder with a few text files is a plugin too. Copy it into the `plugins`
folder of the device (on Android: `QuireInk/plugins` on the device's
storage) and restart; it shows up in *Settings → Plugins*, where it can be
switched off again. These work on every device, including the Xteink X4.

```text
plugins/large-print/
  plugin.ini     [plugin] id, name, description, version
  i18n/*.ini     translations, also whole new languages
  keymap.ini     key bindings, as in quireink.ini
  settings.ini   new defaults for any setting, e.g. [reader] font-size = 14
```

[`samples/plugins/large-print`](samples/plugins/large-print) is a complete
example. Plugins with code are Rust crates using the SDK; they are built
into the app (a web firmware builder for the X4 is planned) or, on larger
devices, will be loadable as WebAssembly.

## Try it in the emulator

You need a recent stable [Rust toolchain](https://rustup.rs). The required
targets are installed automatically from `rust-toolchain.toml`.

```bash
cargo run -p quireink-emulator -- --profile xteink-x4
```

```bash
cargo run -p quireink-emulator -- --list-profiles
```

Keys: `PageDown`/`Space` next page, `PageUp` previous page, arrow keys,
`Enter` confirm, `Esc` back, `M` menu; hold a key for a long press (holding
`Enter` opens the menu on devices without a menu button). The mouse acts as
the touch screen: tap the left or right edge to turn pages, the centre for
the menu. `F2` shows refreshed regions, `F12` saves a screenshot.

Devices with only two buttons (and Bluetooth page turners) can operate
everything with the *Two buttons* layout (`core.keymap-preset`): a short
press moves forward or back, a long press on forward confirms or opens the
menu, a long press on back goes back. Try it with
`--profile two-buttons`.

Settings changed in the UI are saved to `quireink.ini` in the library
folder, the same file you can edit by hand.

The emulator uses `samples/library` as the device storage; two
public-domain books are included as EPUB and TXT. Put your own `.epub` and
`.txt` files into
`samples/library/books` or point `--library` at any folder.

The emulator enforces each device's heap budget on the kernel, so memory
problems show up on the desktop before they show up on hardware
(`--heap-budget none` shows the real peak, `--trace-alloc 1024` lists the
large allocations behind it, `--heap-report` groups the live heap by call
site). Headless
runs produce screenshots for tests and bug reports:

```bash
cargo run -p quireink-emulator -- --headless --profile boox-go-7 --set core.language=de --script "page-forward,long:page-forward" --screenshot out/go7.png
```

## Repository layout

| Path | Contents |
|------|----------|
| `crates/quireink-platform` | Vocabulary shared by shells and kernel: display info, pixel formats, raw input |
| `crates/quireink-config` | The human-editable config format used for settings, key maps and translations |
| `crates/quireink-i18n` | Message catalogs, fallback chains and plural rules |
| `crates/quireink-graphics` | Framebuffers in native pixel formats, clipped canvas |
| `crates/quireink-text` | Fonts (runtime outlines or bitmap packs), rasterizer, glyph cache, line breaking |
| `crates/quireink-document` | Format-independent document model: locators, blocks, offset maps |
| `crates/quireink-layout` | Deterministic page layout usable by any reader plugin |
| `crates/quireink-zip` | Streaming ZIP reader with small, bounded memory use |
| `crates/quireink-xml` | Incremental XML/XHTML tokenizer |
| `crates/quireink-stardict` | StarDict dictionaries looked up in place (dictzip, synonyms) |
| `crates/quireink-sdk` | **The plugin SDK**: the only API plugins use |
| `crates/quireink-core` | The kernel: plugin host, registries, input routing, refresh policy |
| `crates/quireink-app` | The standard app: the first-party plugin set shells install |
| `crates/quireink-fs` | Storage in a host directory, for shells with an operating system |
| `plugins/` | First-party plugins: home screen, reader, library, file manager, menu, settings, plugin manager, menu editor, keyboard, profiles, dictionary, vocabulary, translation, Readwise, Wikipedia, news feeds, sleep screen, reading statistics, progress sync, catalogs (OPDS), browser, Wi-Fi transfer, formats (EPUB, FB2, CBZ, Markdown, TXT, XTC), hyphenation, reading aids, default fonts, diagnostics |
| `tools/fontgen` | Converts fonts into bitmap font packs for small devices |
| `shells/desktop` | Desktop emulator with device profiles |
| `shells/android` | Android app for e-readers such as Boox ([build and install](shells/android/README.md)) |
| `samples/` | Emulator library with public-domain books |
| `docs/` | [Plan](docs/PLANNING.md) and [architecture decisions](docs/adr/) |

## Contributing

Contributions are very welcome: code, translations, device profiles and
testing on real hardware. Please read [CONTRIBUTING.md](CONTRIBUTING.md).

## License

QuireInk is licensed under the [Apache License, Version 2.0](LICENSE).
The bundled fonts, [Literata](https://github.com/googlefonts/literata) and
[Atkinson Hyperlegible Next](https://github.com/googlefonts/atkinson-hyperlegible-next),
are licensed under the [SIL Open Font License 1.1](plugins/fonts-default/fonts/).
