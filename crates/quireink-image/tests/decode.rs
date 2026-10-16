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

/// A file in memory, for writing PNGs.
#[derive(Default)]
struct OneFile(std::cell::RefCell<Vec<u8>>);

impl quireink_platform::Storage for OneFile {
    fn read_at(
        &self,
        _: &str,
        offset: u64,
        buf: &mut [u8],
    ) -> Result<usize, quireink_platform::StorageError> {
        let data = self.0.borrow();
        let start = (offset as usize).min(data.len());
        let n = buf.len().min(data.len() - start);
        buf[..n].copy_from_slice(&data[start..start + n]);
        Ok(n)
    }
    fn size(&self, _: &str) -> Result<u64, quireink_platform::StorageError> {
        Ok(self.0.borrow().len() as u64)
    }
    fn write(&self, _: &str, data: &[u8]) -> Result<(), quireink_platform::StorageError> {
        *self.0.borrow_mut() = data.to_vec();
        Ok(())
    }
    fn append(&self, _: &str, data: &[u8]) -> Result<(), quireink_platform::StorageError> {
        self.0.borrow_mut().extend_from_slice(data);
        Ok(())
    }
    fn list(
        &self,
        _: &str,
        _: &mut dyn FnMut(quireink_platform::DirEntry<'_>),
    ) -> Result<(), quireink_platform::StorageError> {
        Ok(())
    }
    fn remove(&self, _: &str) -> Result<(), quireink_platform::StorageError> {
        Ok(())
    }
}

#[test]
fn written_png_reads_back() {
    let file = OneFile::default();
    let (w, h) = (37u32, 5u32);
    let mut writer = quireink_image::PngWriter::new(&file, "shot.png", w, h).unwrap();
    for y in 0..h {
        let row: Vec<u8> = (0..w).map(|x| ((x * 7 + y * 40) % 256) as u8).collect();
        writer.row(&file, "shot.png", &row).unwrap();
    }
    assert!(writer.is_complete());
    let data = file.0.borrow().clone();
    let image = decoded(&data, 1000);
    assert_eq!((image.width, image.height), (w, h));
    assert_eq!(image.pixels[w as usize + 3], 3 * 7 + 40);
}

#[test]
fn gif_matches_reference() {
    let image = decoded(include_bytes!("data/alice-cover.gif"), 1000);
    assert_eq!((image.width, image.height), (160, 221));
    let diff = difference(&image.pixels, include_bytes!("data/alice-cover-gif.gray"));
    assert!(diff < 0.5, "mean difference {diff}");
}

#[test]
fn interlaced_gif_matches_the_same_picture() {
    let image = decoded(include_bytes!("data/alice-cover-interlaced.gif"), 1000);
    assert_eq!((image.width, image.height), (160, 221));
    let diff = difference(&image.pixels, include_bytes!("data/alice-cover-gif.gray"));
    assert!(diff < 0.5, "mean difference {diff}");
}

#[test]
fn gif_downscales() {
    // By whole factors, as PNG: 160×221 by 3 fits 80.
    let image = decoded(include_bytes!("data/alice-cover.gif"), 80);
    assert_eq!((image.width, image.height), (54, 74));
}

#[test]
fn gif_transparency_is_white_and_the_first_frame_shows() {
    let image = decoded(include_bytes!("data/transparent.gif"), 100);
    assert_eq!((image.width, image.height), (24, 16));
    assert_eq!(image.pixels, include_bytes!("data/transparent.gray"));
    // The second frame's top row is not drawn.
    assert_ne!(image.pixels[10], image.pixels[2]);
}

#[test]
fn damaged_gifs_fail_without_panicking() {
    let data = include_bytes!("data/alice-cover.gif");
    for cut in [6, 13, 400, 800, 5000, data.len() - 1] {
        let mut source = &data[..cut];
        let mut out = Collect::default();
        let _ = decode(Format::Gif, &mut source, 100, &mut out);
    }
}

/// Collects RGB rows, as a sink for colour screens does.
#[derive(Default)]
struct CollectRgb {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl ImageSink for CollectRgb {
    fn start(&mut self, width: u32, height: u32) -> Result<(), ImageError> {
        self.width = width;
        self.height = height;
        Ok(())
    }

    fn row(&mut self, _: &[u8]) -> Result<(), ImageError> {
        panic!("gray row for a sink that wants colour");
    }

    fn wants_color(&self) -> bool {
        true
    }

    fn row_rgb(&mut self, rgb: &[u8]) -> Result<(), ImageError> {
        assert_eq!(rgb.len(), self.width as usize * 3);
        self.pixels.extend_from_slice(rgb);
        Ok(())
    }
}

fn decoded_rgb(data: &[u8], max_side: u32) -> CollectRgb {
    let format = Format::detect(data).expect("known format");
    let mut source = data;
    let mut out = CollectRgb::default();
    decode(format, &mut source, max_side, &mut out).expect("decodes");
    assert_eq!(out.pixels.len(), (out.width * out.height * 3) as usize);
    out
}

#[test]
fn colour_jpeg_matches_reference_in_colour() {
    let image = decoded_rgb(include_bytes!("data/alice-cover.jpg"), 140);
    // 800×1104 by 1/8 fits 140.
    assert_eq!((image.width, image.height), (100, 138));
    let diff = difference(&image.pixels, include_bytes!("data/alice-cover.rgb8"));
    assert!(diff < 6.0, "mean difference {diff}");
}

#[test]
fn colour_pngs_match_references() {
    for (png, rgb) in [
        (
            &include_bytes!("data/colors-rgb.png")[..],
            &include_bytes!("data/colors-rgb.rgb")[..],
        ),
        (
            include_bytes!("data/colors-rgba.png"),
            include_bytes!("data/colors-rgba.rgb"),
        ),
        (
            include_bytes!("data/colors-palette.png"),
            include_bytes!("data/colors-palette.rgb"),
        ),
    ] {
        let image = decoded_rgb(png, 100);
        assert_eq!((image.width, image.height), (20, 12));
        let diff = difference(&image.pixels, rgb);
        assert!(diff < 1.0, "mean difference {diff}");
    }
}

#[test]
fn colour_gif_matches_reference() {
    let image = decoded_rgb(include_bytes!("data/alice-cover.gif"), 111);
    assert_eq!((image.width, image.height), (80, 111));
    let diff = difference(&image.pixels, include_bytes!("data/alice-cover-gif.rgb2"));
    assert!(diff < 1.0, "mean difference {diff}");
}

#[test]
fn gray_sinks_still_get_gray_from_colour_images() {
    // The same pictures for e-ink: gray as before.
    let image = decoded(include_bytes!("data/colors-rgb.png"), 100);
    assert_eq!(image.pixels.len(), 20 * 12);
}

/// `image` reduced by averaging `factor`×`factor` blocks, channel by
/// channel.
fn reduce_rgb(image: &CollectRgb, factor: u32) -> Vec<u8> {
    let channels: Vec<Vec<u8>> = (0..3)
        .map(|c| {
            let plane = Collect {
                width: image.width,
                height: image.height,
                pixels: image.pixels.iter().skip(c).step_by(3).copied().collect(),
            };
            reduce(&plane, factor)
        })
        .collect();
    (0..channels[0].len())
        .flat_map(|i| channels.iter().map(move |c| c[i]))
        .collect()
}

#[test]
fn progressive_jpeg_matches_reference_in_colour() {
    let data = include_bytes!("data/alice-progressive.jpg");
    let reference = include_bytes!("data/alice-progressive.rgb4");
    // A quarter: the 2×2 lowest frequencies of each block, chroma too.
    let image = decoded_rgb(data, 300);
    assert_eq!((image.width, image.height), (200, 276));
    let diff = difference(&image.pixels, reference);
    assert!(diff < 5.0, "mean difference {diff}");
    // Full size: all frequencies.
    let image = decoded_rgb(data, 2000);
    assert_eq!((image.width, image.height), (800, 1104));
    let diff = difference(&reduce_rgb(&image, 4), reference);
    assert!(diff < 4.0, "mean difference {diff}");
}
