//! Decoded images compared with Pillow's reference conversion to gray
//! (`L` mode; transparency composited over white). Small differences are
//! expected from rounding and, when downscaling, from Pillow's box filter.

// Tests panic on purpose when something is wrong.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use quireink_image::{Format, ImageError, ImageSink, decode};

#[derive(Default)]
struct Collect {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl ImageSink for Collect {
    fn start(&mut self, width: u32, height: u32) -> Result<(), ImageError> {
        self.width = width;
        self.height = height;
        Ok(())
    }

    fn row(&mut self, gray: &[u8]) -> Result<(), ImageError> {
        assert_eq!(gray.len(), self.width as usize);
        self.pixels.extend_from_slice(gray);
        Ok(())
    }
}

fn decoded(data: &[u8], max_side: u32) -> Collect {
    let format = Format::detect(data).expect("known format");
    let mut source = data;
    let mut out = Collect::default();
    decode(format, &mut source, max_side, &mut out).expect("decodes");
    assert_eq!(out.pixels.len(), (out.width * out.height) as usize);
    out
}

/// Mean absolute difference.
fn difference(a: &[u8], b: &[u8]) -> f64 {
    assert_eq!(a.len(), b.len());
    let sum: u64 = a
        .iter()
        .zip(b)
        .map(|(&x, &y)| u64::from(x.abs_diff(y)))
        .sum();
    sum as f64 / a.len() as f64
}

#[test]
fn colour_jpeg_matches_reference() {
    let image = decoded(include_bytes!("data/alice-cover.jpg"), 300);
    // 800×1104 by 1/4 fits 300.
    assert_eq!((image.width, image.height), (200, 276));
    let diff = difference(&image.pixels, include_bytes!("data/alice-cover.gray4"));
    assert!(diff < 4.0, "mean difference {diff}");
}

#[test]
fn gray_jpeg_matches_reference() {
    let image = decoded(include_bytes!("data/kafka-title.jpg"), 150);
    assert_eq!((image.width, image.height), (85, 150));
    let diff = difference(&image.pixels, include_bytes!("data/kafka-title.gray4"));
    assert!(diff < 6.0, "mean difference {diff}");
}

/// `image` reduced by averaging `factor`×`factor` blocks (whole ones).
fn reduce(image: &Collect, factor: u32) -> Vec<u8> {
    let (w, h) = (image.width / factor, image.height / factor);
    let mut out = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        for x in 0..w {
            let mut sum = 0u32;
            for dy in 0..factor {
                for dx in 0..factor {
                    let at = (y * factor + dy) * image.width + x * factor + dx;
                    sum += u32::from(image.pixels[at as usize]);
                }
            }
            out.push((sum / (factor * factor)) as u8);
        }
    }
    out
}

#[test]
fn progressive_jpeg_matches_reference() {
    // 4:2:0 colour, ten scans, restart markers.
    let data = include_bytes!("data/alice-progressive.jpg");
    let reference = include_bytes!("data/alice-progressive.gray4");
    // A quarter: the 2×2 lowest frequencies of each block.
    let image = decoded(data, 300);
    assert_eq!((image.width, image.height), (200, 276));
    let diff = difference(&image.pixels, reference);
    assert!(diff < 4.0, "mean difference {diff}");
    // Full size: all frequencies.
    let image = decoded(data, 2000);
    assert_eq!((image.width, image.height), (800, 1104));
    let diff = difference(&reduce(&image, 4), reference);
    assert!(diff < 3.0, "mean difference {diff}");
    // Half size.
    let image = decoded(data, 600);
    assert_eq!((image.width, image.height), (400, 552));
    let diff = difference(&reduce(&image, 2), reference);
    assert!(diff < 3.0, "mean difference {diff}");
    // Very small: the DC only.
    let image = decoded(data, 120);
    assert_eq!((image.width, image.height), (50, 69));
    let small = Collect {
        width: 200,
        height: 276,
        pixels: reference.to_vec(),
    };
    let diff = difference(&image.pixels, &reduce(&small, 4)[..50 * 69]);
    assert!(diff < 6.0, "mean difference {diff}");
}

#[test]
fn progressive_gray_jpeg_matches_reference() {
    let image = decoded(include_bytes!("data/kafka-progressive.jpg"), 150);
    assert_eq!((image.width, image.height), (85, 150));
    let diff = difference(
        &image.pixels,
        include_bytes!("data/kafka-progressive.gray4"),
    );
    assert!(diff < 6.0, "mean difference {diff}");
}

#[test]
fn full_size_jpeg() {
    let image = decoded(include_bytes!("data/alice-cover.jpg"), 2000);
    assert_eq!((image.width, image.height), (800, 1104));
}

#[test]
fn png_colour_types_match_reference() {
    let cases: [(&[u8], &[u8]); 5] = [
        (
            include_bytes!("data/rgba.png"),
            include_bytes!("data/rgba.gray"),
        ),
        (
            include_bytes!("data/palette.png"),
            include_bytes!("data/palette.gray"),
        ),
        (
            include_bytes!("data/gray-alpha.png"),
            include_bytes!("data/gray-alpha.gray"),
        ),
        (
            include_bytes!("data/gray.png"),
            include_bytes!("data/gray.gray"),
        ),
        (
            include_bytes!("data/bilevel.png"),
            include_bytes!("data/bilevel.gray"),
        ),
    ];
    for (i, (png, reference)) in cases.iter().enumerate() {
        let image = decoded(png, 1000);
        assert_eq!((image.width, image.height), (37, 23), "case {i}");
        let diff = difference(&image.pixels, reference);
        assert!(diff < 1.5, "case {i}: mean difference {diff}");
    }
}

#[test]
fn damaged_images_fail_cleanly() {
    for data in [
        &include_bytes!("data/alice-cover.jpg")[..],
        &include_bytes!("data/rgba.png")[..],
        &include_bytes!("data/alice-progressive.jpg")[..],
        &include_bytes!("data/kafka-progressive.jpg")[..],
    ] {
        let step = data.len() / 60 + 1;
        for cut in (0..data.len()).step_by(step) {
            let mut source = &data[..cut];
            let mut out = Collect::default();
            if let Some(format) = Format::detect(&data[..cut]) {
                let _ = decode(format, &mut source, 100, &mut out);
            }
        }
        for at in (0..data.len()).step_by(step / 2 + 1) {
            let mut damaged = data.to_vec();
            damaged[at] ^= 0x5a;
            // Small and large: progressive JPEG keeps more for larger.
            for side in [100, 1000] {
                let mut source = &damaged[..];
                let mut out = Collect::default();
                let _ = decode(
                    Format::detect(&damaged).unwrap_or(Format::Jpeg),
                    &mut source,
                    side,
                    &mut out,
                );
                if out.height > 0 {
                    assert!(out.pixels.len() <= (out.width * out.height) as usize);
                }
            }
        }
    }
}
