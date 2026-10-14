# 0010. Overlays, menus and the generated settings UI

- Status: Accepted
- Date: 2026-10-14

## Context

Menus, dialogs and popups cover only part of the screen; redrawing the
whole page for them is slow on e-ink. Menus must be extensible by any plugin
and customizable by users. Every plugin declares settings; writing a
settings screen per plugin would duplicate work and fragment the UI. On the
ESP32-C3 there is no memory for an off-screen compositing buffer.

## Decision

- **Overlays are screens with a region.** `Screen::region` returns `None`
  for full screens and the covered area for overlays. The kernel redraws
  only invalidated regions: the topmost full screen and all overlays above
  it are drawn clipped to the region — no compositing buffer. Opening or
  closing an overlay refreshes just its area (partial refresh); tapping
  outside an overlay closes it.
- **Menus are contributions.** `Registrar::add_menu_entry` puts an action
  into a named menu with an order; labels are the actions' titles. Users
  redefine any menu in `[menus]` (`reader.menu = core.home, …`). The menu
  overlay is an ordinary plugin bound to the `core.menu` slot and runs the
  chosen action with `Command::RunAction`.
- **The settings screen is generated** from `SettingSpec`s, in registration
  order, by a plugin bound to `core.settings`. Kinds define the editing
  (switch, number range, choice, language).
- **Changed settings are persisted** by the kernel into `quireink.ini` on the
  device, editing the file minimally so comments and order survive
  (`quireink_config::set`), and announced with `core.setting-changed`.
  Events now also reach every open screen (`Screen::on_event`), so screens
  re-layout without polling.
- **Shared UI building blocks** (`quireink_sdk::ui`: style, header, paged
  list, messages) derive all sizes from `core.ui-size`, so plugins look
  consistent and scale together.

## Consequences

- Menus open and close quickly on e-ink and work identically with buttons
  and touch.
- New plugins get a settings UI and persistence for free.
- Hand-edited configuration files stay readable after UI changes.
- Text settings still need an on-screen keyboard to be editable in the UI.
