# 0015. Colour pictures

- Status: Accepted
- Date: 2026-10-16

## Context

QuireInk decodes every picture to gray ([ADR 0012](0012-banded-rendering.md),
`quireink-image`): e-ink shows gray, and gray takes a third of the memory.
Colour screens are common among QuireInk's devices: Android phones and
tablets, colour e-ink (Kaleido and its successors, on Boox, Kobo and
PocketBook). The plan (§3.7) keeps the reader e-ink-first: text and the
interface in gray at full resolution, colour where it adds something,
book covers, pictures in books, comics. Small devices (the X4) must not
pay for colour in flash or RAM.

## Decision

- **Gray stays the base.** Text, lines and the interface are drawn in
  gray as before; only pictures may be in colour. Nothing that matters is
  shown by colour alone.
- **The device says it shows colour** by its pixel format
  (`PixelFormat::is_color`): the Android shell uses an RGB frame on
  devices that are not monochrome e-ink, the emulator on its colour
  profiles. Gray devices are unchanged.
- **Decoders give colour when asked.** An `ImageSink` that wants colour
  (`ImageSink::wants_color`) receives RGB rows (`ImageSink::row_rgb`);
  otherwise gray rows as before. PNG and GIF have their colours anyway;
  JPEG decodes its chroma too, upsampled to luma's resolution, and
  progressive JPEG keeps the chroma coefficients as it keeps luma's.
- **Colour pictures have a cache of their own** (`ColorFile`, `QICI`:
  width, height, then RGB rows) beside the gray one (`GrayFile`), so a
  book read on two devices, or a device switched, keeps both right. A
  format opened with `OpenOptions::color` decodes its pictures in colour.
- **Drawing**: the canvas takes RGB rows (`Canvas::draw_rgb_row`) and
  writes them as they are on colour frames and as gray on gray ones
  (dithered like gray rows on panels with few grays). The colours of
  text and paper ([C22](../PLANNING.md)) tint grays only: a colour pixel
  keeps its colour; night mode inverts both.
- **Colour e-ink** gets a profile per panel (`ColorPanel`): stronger
  saturation where the colour filter mutes it. Dithering for the filter
  follows once a panel is at hand to tune it on.
- **Tier S** builds without the `color` feature of `quireink-graphics`
  and never asks for colour: no colour code, no colour buffers.

## Consequences

- Covers, pictures and comics are in colour on phones, tablets and
  colour e-ink; e-ink in gray stays exactly as it was (the snapshot
  tests).
- Colour pictures take three times the memory of gray ones while
  decoded and in the cache; only devices of tier L ask for them.
- The cache of a book holds pictures for the mode it was read in; the
  other mode's are made when needed.
