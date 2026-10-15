# 0013. The Xteink X4's firmware

- Status: Accepted
- Date: 2026-10-15

## Context

The Xteink X4 is QuireInk's smallest target: an ESP32-C3 (RISC-V, about
400 KB of RAM, 16 MB of flash) with a 4.26" 800 × 480 panel driven by an
SSD1677 controller, seven buttons, a microSD card and a battery. Its
hardware is documented by the community that writes open firmware for it
(the MIT-licensed [community SDK](https://github.com/open-x4-epaper/community-sdk),
[CrossPoint](https://github.com/crosspoint-reader/crosspoint-reader) and
the [FreeInk SDK](https://github.com/Free-Ink/freeink-sdk)): pins,
controller commands, button levels, the battery divider and the power
latch.

The hardware is not at hand while the firmware is written. Its parts must
therefore be testable without it, and the firmware must be thin enough
that what remains untested is little.

Related devices differ: the **X4 Pro** is an ESP32-S3 (Xtensa) with touch
and a front light, and newer batches of the X4 may carry an UltraChip
UC8179 controller instead of the SSD1677.

## Decision

- **Three crates.**
  - `quireink-x4` (`no_std`, published): the display, the buttons and the
    battery against `embedded-hal`, tested on a computer against a
    simulated controller.
  - `quireink-fat` (`no_std`): QuireInk's `Storage` on a FAT file system
    with long file names, and a disk of 512-byte blocks as bytes; tested
    on disks in memory. Its FAT library is not released with `no_std`
    support yet, so it is pinned to a commit and not published.
  - `shells/xteink`: the firmware, built for the chip only (outside the
    workspace). It connects the chip's pins to the two crates and runs the
    kernel with the standard app.
- **No frame in RAM.** The kernel draws in bands of 96 rows, aligned to 8
  (`KernelBuilder::row_align`), and the display writes each band turned
  into the controller's memory. For a fast refresh, which compares with
  the frame shown, the bands go into the memory of the new frame, then
  after the refresh again into that of the frame shown
  (`Kernel::rerender`). Drawing twice costs time after the page is shown,
  not memory; a full frame would take a quarter of the heap.
- **Buttons are polled** every 10 ms, read from two resistor ladders by the
  ADC, and debounced; the kernel recognises long presses as on every
  device.
- **Sleep is power off.** After the sleep screen the firmware releases the
  battery latch (GPIO 13), which cuts the power on battery; the power
  button starts the X4 again, and QuireInk reopens the book. On USB power
  it sleeps deeply until the button. A power button still down at start
  counts only once released.
- **Fonts as packs in flash**, up to 36 px: code and data share a 4 MB
  window of the flash.
- **The SSD1677 first.** A driver for the UC8179 follows once a device
  shows which controller it has; the display is the only part that
  differs. The X4 Pro gets its own firmware (another chip and toolchain),
  sharing the crates where its hardware is the same.

## Consequences

- Almost all of the firmware's logic runs in CI and on every computer;
  what needs the device is the wiring in `shells/xteink` and the panel's
  behaviour (waveforms, timing).
- Before the first flash, the stock firmware is saved (see
  `shells/xteink/README.md`), so the X4 can always go back.
- Still to come on the device: Wi-Fi (time, sync, transfers), lighter
  sleep between button reads (power), 4 gray levels, the UC8179 and a
  larger flash window.
