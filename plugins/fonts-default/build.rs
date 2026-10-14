//! Generates bitmap font packs when the `packs` feature is enabled or the
//! `outline` feature is disabled.

use std::path::{Path, PathBuf};

/// Pixel sizes for packs; covers UI and reading text on ~220 dpi screens.
const DEFAULT_SIZES: &str = "20,24,28,32,36,40,48";

/// (pack name, font file, weight)
const FONTS: &[(&str, &str, f32)] = &[
    ("literata", "fonts/Literata.ttf", 400.0),
    ("literata-bold", "fonts/Literata.ttf", 700.0),
    ("literata-italic", "fonts/Literata-Italic.ttf", 400.0),
    ("literata-bold-italic", "fonts/Literata-Italic.ttf", 700.0),
    ("atkinson", "fonts/AtkinsonHyperlegibleNext.ttf", 400.0),
];

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=fonts");
    println!("cargo::rerun-if-env-changed=QUIREINK_FONT_SIZES");
    if std::env::var_os("CARGO_FEATURE_OUTLINE").is_some()
        && std::env::var_os("CARGO_FEATURE_PACKS").is_none()
    {
        return;
    }

    let sizes_text = std::env::var("QUIREINK_FONT_SIZES").unwrap_or(DEFAULT_SIZES.into());
    let sizes: Vec<f32> = sizes_text
        .split(',')
        .map(|s| {
            s.trim()
                .parse()
                .unwrap_or_else(|_| panic!("invalid font size `{s}`"))
        })
        .collect();

    let out = PathBuf::from(
        std::env::var_os("OUT_DIR").unwrap_or_else(|| panic!("OUT_DIR is set by cargo")),
    );
    for &(name, path, weight) in FONTS {
        let data = std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let font = quireink_text::OutlineFont::new(data, &[("wght", weight)])
            .unwrap_or_else(|e| panic!("{path}: {e}"));
        let chars = quireink_text::charset::chars(quireink_text::charset::LATIN);
        let pack = quireink_text::build_pack(&font, &sizes, chars);
        write(&out.join(format!("{name}.qifp")), &pack);
    }
}

fn write(path: &Path, data: &[u8]) {
    std::fs::write(path, data).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
}
