# 0012. Rendering in bands

- Status: Accepted
- Date: 2026-10-14

## Context

On the Xteink X4 a 2-bit framebuffer for 480×800 pixels takes 94 KiB, about
half of the heap budget, although the panel controller keeps the whole
frame in its own memory anyway. Event handlers could also draw directly
into the framebuffer, which ties screens to a framebuffer that holds the
whole screen.

## Decision

- **Drawing happens only in `Screen::draw`.** Event handlers request a
  redraw of the screen (`Cx::request_redraw`) or of an area
  (`Cx::request_redraw_area`); `Cx` no longer offers a canvas. `draw` may
  run several times per frame for different parts of the screen, so it
  must not change state.
- **The kernel draws when the shell asks.** `start`, `handle` and `tick`
  only collect what changed and return the refresh in their `Effects`. The
  shell then calls `Kernel::render`, which draws the changed region and
  passes it on as full-width rows of pixels, then refreshes the panel.
- **Bands for small devices.** With `KernelBuilder::frame_rows(n)` the
  framebuffer holds only `n` rows. `render` moves this band down the
  changed region, draws each part and hands it to the shell before drawing
  the next. Coordinates stay screen coordinates; canvases are clipped to
  the band. Without `frame_rows` the band is the whole screen, and nothing
  changes for large devices.

## Consequences

- The X4 profile uses bands of 100 rows: 11.7 KiB instead of 94 KiB. The
  home screen needs 43 KiB of heap instead of 132 KiB, and EPUB chapters
  of any size can be converted within the budget.
- Each band costs one more call to the visible screens' `draw`. Screens
  should skip what lies outside the canvas clip; layout belongs in event
  handlers, not in `draw`.
- Shells write pixels to the panel as they arrive. A shell that needs the
  whole frame (e.g. the emulator, or a screenshot feature) keeps its own
  copy with `Framebuffer::write_rows`.
- Effects that read back pixels, such as blending over what is already on
  screen, only see the current band. Everything drawn in one band is drawn
  completely in it, so this does not matter for drawing; full-screen
  post-processing (e.g. dithering across bands) must run per band.
