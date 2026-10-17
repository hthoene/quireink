//! Links with esp-hal's linker script. Here rather than in
//! `.cargo/config.toml`: `RUSTFLAGS` (as CI sets it) replaces the
//! configuration's flags, not this.
//!
//! The memory map is QuireInk's own (`memory.x`): esp-hal's maps 4 MB of
//! flash for code and read-only data together, and the fonts take much of it.

use std::path::PathBuf;

fn main() {
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    std::fs::copy("memory.x", out.join("memory.x")).expect("memory.x");
    println!("cargo::rustc-link-search={}", out.display());
    println!("cargo::rerun-if-changed=memory.x");
    println!("cargo::rustc-link-arg=-Tlinkall.x");
}
