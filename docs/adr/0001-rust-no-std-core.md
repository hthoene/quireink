# 0001. Rust with a `no_std` core

- Status: Accepted
- Date: 2026-10-14

## Context

The smallest target, the Xteink X4, has an ESP32-C3 with about 380 KB of RAM
and no PSRAM. Languages with a garbage collector or a large runtime are ruled
out. The same code must also run on ARM Linux, Android, desktop and in the
browser. QuireInk parses untrusted files and hosts third-party plugins.

The alternative was C++ building on the CrossPoint Reader firmware, which
would give a faster start on the X4.

## Decision

QuireInk is written in Rust. All core crates are `#![no_std]` with `alloc`
and must build for `riscv32imc-unknown-none-elf`. Shells may use `std`.
CrossPoint Reader (MIT) serves as a reference for hardware details; ported
algorithms are attributed.

## Consequences

- One Cargo workspace builds every target, including WASM and static musl
  binaries for old Kindle firmware.
- Memory safety for parsers of untrusted input.
- Traits make compile-time plugins natural; `wasmi` (pure Rust) can host
  runtime plugins.
- No existing e-reader code base to start from; the rendering pipeline is
  written from scratch.
