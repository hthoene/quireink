# QuireInk

**A lean, fast and endlessly customizable e-reader for e-ink devices — from a
380 KB microcontroller to Linux and Android e-readers, from one codebase.**

> **Status: early development.** Plain text books can be read in the
> emulator — with menus, a settings screen, page index, saved reading
> position and reopening of the last book. EPUB comes next. See the [project plan](docs/PLANNING.md)
> and the [roadmap](docs/PLANNING.md#12-roadmap).

## Why QuireInk?

- **Runs where others can't.** The smallest target is the Xteink X4 with an
  ESP32-C3 and no PSRAM. Everything is designed for that budget, so larger
  devices get a fast, battery-friendly reader for free.
- **Everything is a plugin.** The core is a small kernel. The home screen,
  library, reader, menus, file formats, themes, key maps and translations are
  plugins — first-party ones use exactly the same SDK as community plugins.
  Anything can be added, replaced, reconfigured or removed.
- **Simple by default.** Turn it on and keep reading. Every function is at
  most two steps away, and it works with buttons only or with touch.
- **One codebase, many devices.** Xteink X4/X3, Kobo, Kindle (jailbroken),
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
# Switch any plugin off by its id.
some-plugin = off
```

Plugins contribute screens, actions, settings, key maps and translations
through the [`quireink-sdk`](crates/quireink-sdk) crate. See
[docs/PLANNING.md §7](docs/PLANNING.md#7-extensibility) for the full
extension model.

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

Settings changed in the UI are saved to `quireink.ini` in the library
folder — the same file you can edit by hand.

The emulator uses `samples/library` as the device storage; two
public-domain books are included. Put your own `.txt` files into
`samples/library/books` or point `--library` at any folder.

The emulator enforces each device's heap budget on the kernel, so memory
problems show up on the desktop before they show up on hardware. Headless
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
| `crates/quireink-sdk` | **The plugin SDK** — the only API plugins use |
| `crates/quireink-core` | The kernel: plugin host, registries, input routing, refresh policy |
| `plugins/` | First-party plugins: reader, file browser, menu, settings, TXT format, default fonts, diagnostics |
| `tools/fontgen` | Converts fonts into bitmap font packs for small devices |
| `shells/desktop` | Desktop emulator with device profiles |
| `samples/` | Emulator library with public-domain books |
| `docs/` | [Plan](docs/PLANNING.md) and [architecture decisions](docs/adr/) |

## Contributing

Contributions are very welcome — code, translations, device profiles and
testing on real hardware. Please read [CONTRIBUTING.md](CONTRIBUTING.md).

## License

QuireInk is licensed under the [Apache License, Version 2.0](LICENSE).
The bundled fonts — [Literata](https://github.com/googlefonts/literata) and
[Atkinson Hyperlegible Next](https://github.com/googlefonts/atkinson-hyperlegible-next) —
are licensed under the [SIL Open Font License 1.1](plugins/fonts-default/fonts/).
