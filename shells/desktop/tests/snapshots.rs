//! Snapshot tests: the emulator draws screens headless on several device
//! profiles, and each picture is compared with its reference in
//! `tests/snapshots`. Layout faults (text over a button, a cut line, a
//! widget out of place) then show as a failing test.
//!
//! The emulator's clock and battery are fixed, and every run starts from
//! a fresh library with the sample books, so the pictures repeat. A few
//! pixels may differ between computers where glyph edges round
//! differently; more than that fails.
//!
//! After a change meant to alter a screen, write the references anew:
//!
//! ```sh
//! QUIREINK_BLESS=1 cargo test -p quireink-emulator --test snapshots
//! ```
//!
//! and look at the changed pictures before committing them. A failing
//! test leaves what it drew in `target/snapshots`.

// Tests panic on purpose when something is wrong.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs::{self, File};
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A screen to draw: its name, the device profile, settings, input steps.
struct Case {
    name: &'static str,
    profile: &'static str,
    /// The book open at start, if any.
    book: Option<&'static str>,
    sets: &'static [&'static str],
    script: &'static str,
}

const CASES: &[Case] = &[
    Case {
        name: "x4-home",
        profile: "xteink-x4",
        book: None,
        sets: &[],
        script: "",
    },
    Case {
        name: "x4-reader-epub",
        profile: "xteink-x4",
        book: Some("books/alice-in-wonderland.epub"),
        sets: &[],
        script: "page-forward,page-forward",
    },
    Case {
        name: "boox-home",
        profile: "boox-go-7",
        book: None,
        sets: &[],
        script: "",
    },
    Case {
        name: "boox-home-arranged",
        profile: "boox-go-7",
        book: None,
        sets: &[
            "home.page-1=home.status, core.fill, home.clock, core.space, home.date, core.fill, home.tools",
            "home.clock.align=core.align.center",
            "home.date.align=core.align.center",
            "home.status.align=core.align.right",
        ],
        script: "",
    },
    Case {
        name: "boox-reader-epub",
        profile: "boox-go-7",
        book: Some("books/die-verwandlung.epub"),
        sets: &[],
        script: "",
    },
    Case {
        name: "boox-reader-txt",
        profile: "boox-go-7",
        book: Some("books/alice-in-wonderland.txt"),
        sets: &["core.language=de"],
        script: "",
    },
    Case {
        name: "boox-settings",
        profile: "boox-go-7",
        book: None,
        sets: &["slots.core.home=settings.list"],
        script: "",
    },
    Case {
        name: "boox-widget-editor",
        profile: "boox-go-7",
        book: None,
        // The pencil, then the second widget.
        sets: &["home.page-1=home.status, home.clock, home.date"],
        script: "tap:1185:1635,wait:300,tap:600:246,wait:300",
    },
];

/// Pixels whose gray differs by more than this count as different.
const LEVELS: u8 = 48;
/// The share of pixels that may differ, in parts per ten thousand.
const ALLOWED: usize = 20;

/// The repository: two folders above this crate.
fn root() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest
        .ancestors()
        .nth(2)
        .unwrap_or(manifest)
        .to_path_buf()
}

/// A library with only the sample books (and the book to reopen).
fn library(case: &Case) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "quireink-snapshot-{}-{}",
        case.name,
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    let books = dir.join("books");
    fs::create_dir_all(&books).unwrap();
    for entry in fs::read_dir(root().join("samples/library/books")).unwrap() {
        let entry = entry.unwrap();
        fs::copy(entry.path(), books.join(entry.file_name())).unwrap();
    }
    if let Some(book) = case.book {
        let state = dir.join(".quireink/plugins/reader");
        fs::create_dir_all(&state).unwrap();
        fs::write(
            state.join("state.ini"),
            format!("[reader]\nlast = {book}\n"),
        )
        .unwrap();
    }
    dir
}

/// The gray of each pixel of a PNG, with its size.
fn grays(path: &Path) -> (u32, u32, Vec<u8>) {
    let decoder = png::Decoder::new(File::open(path).unwrap());
    let mut reader = decoder.read_info().unwrap();
    let mut data = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut data).unwrap();
    let data = &data[..info.buffer_size()];
    let gray = match info.color_type {
        png::ColorType::Grayscale => data.to_vec(),
        png::ColorType::Rgb => data
            .chunks_exact(3)
            .map(|p| {
                ((u32::from(p[0]) * 299 + u32::from(p[1]) * 587 + u32::from(p[2]) * 114) / 1000)
                    as u8
            })
            .collect(),
        other => panic!("{}: unexpected {other:?}", path.display()),
    };
    (info.width, info.height, gray)
}

fn write_gray(path: &Path, width: u32, height: u32, gray: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut encoder = png::Encoder::new(BufWriter::new(File::create(path).unwrap()), width, height);
    encoder.set_color(png::ColorType::Grayscale);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Best);
    let mut writer = encoder.write_header().unwrap();
    writer.write_image_data(gray).unwrap();
}

/// Draws `case` and returns its picture in grays.
fn draw(case: &Case) -> (u32, u32, Vec<u8>) {
    let library = library(case);
    let shot = library.join("shot.png");
    let mut command = Command::new(env!("CARGO_BIN_EXE_quireink-emu"));
    command
        .args(["--profile", case.profile, "--headless", "--library"])
        .arg(&library)
        .args(["--screenshot"])
        .arg(&shot);
    if !case.script.is_empty() {
        command.args(["--script", case.script]);
    }
    for set in case.sets {
        command.args(["--set", set]);
    }
    if case.book.is_none() {
        command.args(["--set", "reader.open-last=false"]);
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}: {}",
        case.name,
        String::from_utf8_lossy(&output.stderr)
    );
    let picture = grays(&shot);
    let _ = fs::remove_dir_all(&library);
    picture
}

#[test]
fn screens_look_as_before() {
    let bless = std::env::var_os("QUIREINK_BLESS").is_some();
    let references = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots");
    let failed_dir = root().join("target/snapshots");
    let mut failures = Vec::new();
    for case in CASES {
        let (width, height, gray) = draw(case);
        let reference = references.join(format!("{}.png", case.name));
        if bless || !reference.exists() {
            write_gray(&reference, width, height, &gray);
            continue;
        }
        let (w, h, expected) = grays(&reference);
        let differing = if (w, h) == (width, height) {
            gray.iter()
                .zip(&expected)
                .filter(|(a, b)| a.abs_diff(**b) > LEVELS)
                .count()
        } else {
            usize::MAX
        };
        if differing > gray.len() * ALLOWED / 10_000 {
            let drawn = failed_dir.join(format!("{}.png", case.name));
            write_gray(&drawn, width, height, &gray);
            failures.push(format!(
                "{}: {} pixels differ (drawn: {})",
                case.name,
                if differing == usize::MAX {
                    format!("size {width}x{height} instead of {w}x{h}, all")
                } else {
                    differing.to_string()
                },
                drawn.display()
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "screens changed; if meant, run with QUIREINK_BLESS=1:\n{}",
        failures.join("\n")
    );
}
