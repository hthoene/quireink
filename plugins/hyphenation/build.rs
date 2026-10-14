//! Compiles the hyphenation patterns into tables that are read in place.

use std::path::PathBuf;

/// (output name, patterns, exceptions, characters before / after a hyphen)
const LANGUAGES: &[(&str, &str, Option<&str>, u8, u8)] = &[
    ("de", "patterns/hyph-de-1996.pat.txt", None, 2, 2),
    (
        "en",
        "patterns/hyph-en-us.pat.txt",
        Some("patterns/hyph-en-us.hyp.txt"),
        2,
        3,
    ),
];

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=patterns");
    let out = PathBuf::from(
        std::env::var_os("OUT_DIR").unwrap_or_else(|| panic!("OUT_DIR is set by cargo")),
    );
    let read = |path: &str| std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    for &(name, patterns, exceptions, left, right) in LANGUAGES {
        let table = quireink_hyphen::compile(
            &read(patterns),
            &exceptions.map(read).unwrap_or_default(),
            left,
            right,
        );
        let path = out.join(format!("{name}.qihp"));
        std::fs::write(&path, table).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    }
}
