# 0004. Kernel plus plugins: everything is a plugin

- Status: Accepted
- Date: 2026-10-14

## Context

QuireInk should be the most customizable e-reader software: users want
different home screens (not just a file browser), reading modes, gestures,
sync services and many more features. A core with a few "extension points"
would leave everything else fixed, and first-party features would drift
ahead of what plugins can do.

## Decision

- The core is a **kernel**: plugin host, registries, slot bindings, input
  routing, commands, settings store, refresh policy and rendering. It
  contains no product UI.
- Every screen, action, setting, key map, translation, format, theme and
  service is a **contribution** with a namespaced id (`plugin.name`),
  registered by a plugin. The kernel registers its own contributions as the
  `core` plugin through the same interface.
- UI roles are **slots** (`core.home`, `core.reader`, …). Plugins propose
  screens for slots with a priority; the user's configuration overrides any
  binding. Plugins can define new slots.
- Plugins depend on **`quireink-sdk` only**. First-party plugins have no
  privileged access, so the SDK is always sufficient to replace them.
- Registration is **transactional**: a plugin whose registration fails
  leaves no contributions behind.
- Plugins are compiled in (all tiers) or, later, loaded as sandboxed
  WebAssembly modules (larger devices). Declarative contributions (key maps,
  translations, layouts, themes) are data and work on every tier.

### Switching plugins on and off

- Every plugin given to the kernel is listed in `Env::plugins` with its
  manifest (name, **description**, version) and state: running, off or
  failed. Names and descriptions are translated with the keys
  `<id>.plugin.name` and `<id>.plugin.description`; for plugins that are
  off, the kernel loads only their translations.
- `Command::SetPluginEnabled` writes `[plugins] id = on|off` to
  `quireink.ini`; the change applies after `Command::Restart`, which shells
  answer by building a new kernel. Contributions are never removed from a
  running kernel, which keeps registries simple and small.
- To keep users from locking themselves out, the kernel refuses to switch
  off `core` and any plugin that alone provides a standard slot
  (`core.home`, `core.reader`, `core.library`, `core.menu`,
  `core.settings`) or every font. Another plugin has to take over first.
- The plugin manager is itself a replaceable plugin (`plugin-manager`) and
  keeps itself on.

## Consequences

- Any part of QuireInk can be replaced or removed by configuration.
- The SDK becomes the most important API and is versioned with SemVer.
- Some indirection (registries, ids) where a direct call would be simpler;
  kept at coarse granularity so the cost is negligible.
- Before adding a feature to a first-party plugin, we check that a community
  plugin could build it; otherwise the SDK is extended first.
- Switching plugins needs a restart. On e-readers that takes about a
  second and keeps the reading position.
