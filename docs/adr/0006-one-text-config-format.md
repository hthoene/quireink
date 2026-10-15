# 0006. One human-editable text format for configuration and data

- Status: Accepted
- Date: 2026-10-14

## Context

Settings, key maps, translations and plugin manifests should be editable
with any text editor directly on the device's storage. The parser must run
on the ESP32-C3. TOML and JSON parsers with serde are comparatively large
and allocation-heavy; Fluent (for translations) needs `std`.

## Decision

QuireInk uses one small INI dialect (`quireink-config`) everywhere:
`[section]`, `key = value`, `#`/`;` comments, quoted values with escapes.
The parser is zero-copy and `no_std`. Settings are namespaced by section
(`[core] language = de` → `core.language`). Translations use the same format
with `{placeholder}` interpolation and CLDR plural categories.

Hierarchical data (UI layouts) needs a tree syntax; that will be decided in
a separate ADR.

## Consequences

- Users learn one format; editors highlight it.
- A tiny parser with good error messages (line numbers) on every device.
- Limited expressiveness by design: complex data belongs in plugins.
