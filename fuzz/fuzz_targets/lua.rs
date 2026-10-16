//! Any text as one of KOReader's settings files, read and, where it
//! reads, written and read back the same.

#![no_main]

use libfuzzer_sys::fuzz_target;
use quireink_plugin_koreader::lua;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    if let Some(value) = lua::parse(&text) {
        assert_eq!(lua::parse(&lua::write(&value)), Some(value));
    }
});
