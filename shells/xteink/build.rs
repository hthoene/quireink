//! Links with esp-hal's linker script. Here rather than in
//! `.cargo/config.toml`: `RUSTFLAGS` (as CI sets it) replaces the
//! configuration's flags, not this.

fn main() {
    println!("cargo::rustc-link-arg=-Tlinkall.x");
}
