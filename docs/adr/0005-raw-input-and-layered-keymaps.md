# 0005. Raw input and layered key maps in the kernel

- Status: Accepted
- Date: 2026-10-14

## Context

The first draft let shells translate keys into a fixed set of actions. That
made input mapping shell-specific, prevented plugins from adding actions and
made gestures, long-press tuning and pen input impossible to customize.

## Decision

- Shells report raw input only: named buttons going down and up, and pointer
  samples (down/move/up, tool, pressure). Standard button names are defined
  in `quireink-platform`.
- The kernel recognizes long presses, taps, long taps and swipes, with
  thresholds from settings. Screens may capture a pointer to receive raw
  samples (drawing, handwriting).
- Recognized inputs resolve to **action ids** through **key map layers**:
  the active screen's layer, its slot's layer, then the global layer.
  Within a layer, user bindings override device bindings, which override
  plugin defaults. `none` removes a binding.
- Actions are contributions; any plugin can add actions and users can bind
  any action to any input.

## Consequences

- Identical, user-editable input behaviour on every device.
- Shells become simpler.
- The kernel needs time (`now`) and timers (`WakeAt`) for long presses —
  which other features (statistics, schedules) need anyway.
