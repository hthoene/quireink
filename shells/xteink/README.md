# QuireInk on the Xteink X4

The firmware for the Xteink X4 (ESP32-C3). See
[ADR 0013](../../docs/adr/0013-x4-firmware.md) for how it is built; its
hardware parts are in [`quireink-x4`](../../crates/quireink-x4) and
[`quireink-fat`](../../crates/quireink-fat), tested on a computer.

> **Not yet run on a device.** It builds, and its parts are tested
> against simulated hardware; the first runs on an X4 are still to come.
> Save the stock firmware first (below), so the X4 can always go back.

It is for the X4 with an SSD1677 display controller. Not for the X4 Pro,
which has another chip.

## Build

The stable Rust toolchain of the repository builds it (the target
`riscv32imc-unknown-none-elf` comes with `rust-toolchain.toml`):

```sh
cd shells/xteink
cargo build --release
```

## Save the stock firmware

Install [espflash](https://github.com/esp-rs/espflash)
(`cargo install espflash`), connect the X4 by USB and read its whole
flash (16 MB) into a file. Keep that file.

```sh
espflash read-flash 0x0 0x1000000 x4-stock-firmware.bin
```

To go back to it later:

```sh
espflash write-bin 0x0 x4-stock-firmware.bin
```

## Flash and run

```sh
cargo run --release
```

flashes the X4 and shows its log. Books go on a microSD card formatted
with FAT32, e.g. in `Books`.

## Remote control

Over USB, [`tools/x4-remote.py`](../../tools/x4-remote.py) presses the
X4's buttons, takes screenshots and prints what the screen shows, with
the steps of the emulator's scripts:

```sh
python tools/x4-remote.py --script "page-forward,right,describe" --screenshot out/x4.png
```

## What works, what is to come

- The screen (black and white, fast and full refreshes), the seven
  buttons, the battery, the SD card with long file names, sleep (the
  power button switches it off and on again, where it was).
- To come: Wi-Fi (time, sync, transfers), lighter sleep between button
  reads, 4 gray levels, the UC8179 controller of newer X4s.
