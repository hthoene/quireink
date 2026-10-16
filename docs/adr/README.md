# Architecture Decision Records

Significant, hard-to-reverse decisions are recorded here, one file each,
numbered in order. A decision is never edited after acceptance; it is
superseded by a new record that links back.

| ADR | Title | Status |
|-----|-------|--------|
| [0001](0001-rust-no-std-core.md) | Rust with a `no_std` core | Accepted |
| [0002](0002-apache-2-license.md) | Apache License 2.0 | Accepted |
| [0003](0003-sans-io-kernel.md) | Sans-I/O kernel | Accepted |
| [0004](0004-everything-is-a-plugin.md) | Kernel plus plugins: everything is a plugin | Accepted |
| [0005](0005-raw-input-and-layered-keymaps.md) | Raw input and layered key maps in the kernel | Accepted |
| [0006](0006-one-text-config-format.md) | One human-editable text format for configuration and data | Accepted |
| [0007](0007-content-based-locators.md) | Content-based reading positions | Accepted |
| [0008](0008-fonts-outlines-and-bitmap-packs.md) | Fonts: runtime outlines and bitmap packs | Accepted |
| [0009](0009-documents-formats-and-storage.md) | Documents, formats and local storage | Accepted |
| [0010](0010-overlays-menus-and-settings-ui.md) | Overlays, menus and the generated settings UI | Accepted |
| [0011](0011-epub-streaming-conversion.md) | EPUB: streaming conversion into a block cache | Accepted |
| [0012](0012-banded-rendering.md) | Rendering in bands | Accepted |
| [0013](0013-x4-firmware.md) | The Xteink X4's firmware | Accepted |
| [0014](0014-sockets-for-plugins.md) | Sockets for plugins | Accepted |

## Template

```markdown
# NNNN. Title

- Status: Proposed | Accepted | Superseded by NNNN
- Date: YYYY-MM-DD

## Context
What forces are at play?

## Decision
What we do.

## Consequences
What becomes easier, what becomes harder.
```
