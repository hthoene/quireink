//! Images for e-ink: JPEG, PNG and GIF decoded to grayscale, row by row.
//!
//! Decoders pull bytes from a [`Source`] and push rows of 8-bit gray to an
//! [`ImageSink`], downscaling while they decode, so memory stays at a few
//! rows (plus 43 KiB for PNG's inflate state) whatever the image size.
//! Brightness is all e-ink shows, so colour images are converted on the
//! fly; a sink that wants colour ([`ImageSink::wants_color`], for colour
//! screens, ADR 0015) gets RGB rows instead ([`ColorFile`]).
//!
//! [`GrayFile`] stores decoded images in a simple format (`QIGI`, width
//! and height as u32, then rows), so readers draw them straight from
//! storage without decoding again.
//!
//! Supported: baseline and progressive JPEG (1 or 3 components, any
//! sampling; progressive ones keep their coefficients until the last
//! scan, as few as the output size needs), non-interlaced PNG (all
//! colour types and depths, transparency over white) and GIF (the first
//! picture of the file, transparency over white). Interlaced PNG and
//! other formats return [`ImageError::Unsupported`].

#![no_std]

extern crate alloc;

mod color;
mod gif;
mod gray;
mod jpeg;
mod png;
mod png_write;
mod progressive;

use core::fmt;

pub use color::{ColorFile, ColorWriter};
pub use gray::{GrayFile, GrayWriter};
pub use png_write::PngWriter;

/// Why an image could not be decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ImageError {
    /// The data ended early or is malformed.
    Invalid(&'static str),
    /// A valid image in a variant this decoder does not handle.
    Unsupported(&'static str),
    /// Reading the source or writing the result failed.
    Io,
    OutOfMemory,
    /// The sink stopped decoding.
    Stopped,
}

impl fmt::Display for ImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImageError::Invalid(what) => write!(f, "invalid image: {what}"),
            ImageError::Unsupported(what) => write!(f, "unsupported image: {what}"),
            ImageError::Io => f.write_str("image could not be read or written"),
            ImageError::OutOfMemory => f.write_str("not enough memory for the image"),
            ImageError::Stopped => f.write_str("decoding stopped"),
        }
    }
}

/// Where encoded image bytes come from.
pub trait Source {
    /// Reads up to `buf.len()` bytes; 0 at the end.
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, ImageError>;
}

impl Source for &[u8] {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, ImageError> {
        let n = buf.len().min(self.len());
        buf[..n].copy_from_slice(&self[..n]);
        *self = &self[n..];
        Ok(n)
    }
}

/// Receives a decoded image.
pub trait ImageSink {
    /// The size of the (downscaled) image, before its first row.
    fn start(&mut self, width: u32, height: u32) -> Result<(), ImageError>;
    /// The next row, `width` bytes of gray (0 black, 255 white).
    fn row(&mut self, gray: &[u8]) -> Result<(), ImageError>;

    /// Whether rows are wanted in colour ([`ImageSink::row_rgb`]), where
    /// the image has colour; gray otherwise.
    fn wants_color(&self) -> bool {
        false
    }

    /// The next row in colour, `width` pixels of three bytes (red, green,
    /// blue). Only for sinks that want colour; otherwise it is passed on
    /// as gray.
    fn row_rgb(&mut self, rgb: &[u8]) -> Result<(), ImageError> {
        let gray: alloc::vec::Vec<u8> = rgb
            .chunks_exact(3)
            .map(|p| luma(u32::from(p[0]), u32::from(p[1]), u32::from(p[2])))
            .collect();
        self.row(&gray)
    }
}

/// The gray of a colour, as e-ink shows it (ITU-R 601).
pub(crate) fn luma(r: u32, g: u32, b: u32) -> u8 {
    ((r * 299 + g * 587 + b * 114 + 500) / 1000) as u8
}

/// Image formats by their first bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Jpeg,
    Png,
    Gif,
}

impl Format {
    pub fn detect(head: &[u8]) -> Option<Format> {
        if head.starts_with(&[0xff, 0xd8]) {
            Some(Format::Jpeg)
        } else if head.starts_with(b"\x89PNG\r\n\x1a\n") {
            Some(Format::Png)
        } else if head.starts_with(b"GIF8") {
            Some(Format::Gif)
        } else {
            None
        }
    }
}

/// The size pictures are decoded to at most: as large as they show on a
/// screen of these sides, upright or turned, and never larger than they
/// are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fit {
    pub short: u32,
    pub long: u32,
}

impl Fit {
    /// A screen of `width` × `height` pixels, either way up.
    pub const fn screen(width: u32, height: u32) -> Self {
        if width < height {
            Self {
                short: width,
                long: height,
            }
        } else {
            Self {
                short: height,
                long: width,
            }
        }
    }

    /// At most `side` pixels on either side.
    pub const fn side(side: u32) -> Self {
        Self {
            short: side,
            long: side,
        }
    }

    /// What fits both.
    pub fn min(self, other: Fit) -> Fit {
        Fit {
            short: self.short.min(other.short),
            long: self.long.min(other.long),
        }
    }

    /// The size a picture of `width` × `height` is decoded to: as large as
    /// it shows on the screen upright or turned, whichever is larger, in
    /// its proportions.
    pub fn size(&self, width: u32, height: u32) -> (u32, u32) {
        let upright = scaled(width, height, self.short, self.long);
        let turned = scaled(width, height, self.long, self.short);
        if turned.0 > upright.0 {
            turned
        } else {
            upright
        }
    }
}

/// `width` × `height` made just small enough for `max_width` ×
/// `max_height`, in its proportions (a pixel at least on each side).
fn scaled(width: u32, height: u32, max_width: u32, max_height: u32) -> (u32, u32) {
    let (max_width, max_height) = (max_width.max(1), max_height.max(1));
    if width == 0 || height == 0 || (width <= max_width && height <= max_height) {
        return (width, height);
    }
    let (w, h) = (u64::from(width), u64::from(height));
    let (mw, mh) = (u64::from(max_width), u64::from(max_height));
    if w * mh > h * mw {
        // As wide as it may be.
        let h = ((h * mw + w / 2) / w).clamp(1, mh);
        (max_width, h as u32)
    } else {
        let w = ((w * mh + h / 2) / h).clamp(1, mw);
        (w as u32, max_height)
    }
}

/// The pixels of each 8×8 block's side a JPEG is decoded at (8, 4, 2 or
/// 1): the fewest that still give `size` at least.
pub(crate) fn block_side(width: u32, height: u32, size: (u32, u32)) -> usize {
    for n in [1u32, 2, 4] {
        let scale = 8 / n;
        if width.div_ceil(scale) >= size.0 && height.div_ceil(scale) >= size.1 {
            return n as usize;
        }
    }
    8
}

/// Decodes an image of `format`, made as small as `fit` allows.
pub fn decode(
    format: Format,
    source: &mut dyn Source,
    fit: Fit,
    sink: &mut dyn ImageSink,
) -> Result<(), ImageError> {
    let mut reader = Reader::new(source);
    match format {
        Format::Jpeg => jpeg::decode(&mut reader, fit, sink),
        Format::Png => png::decode(&mut reader, fit, sink),
        Format::Gif => gif::decode(&mut reader, fit, sink),
    }
}

/// Buffered byte reader over a [`Source`].
pub(crate) struct Reader<'a> {
    source: &'a mut dyn Source,
    /// Read several blocks of a card at a time.
    buf: [u8; 4096],
    pos: usize,
    len: usize,
}

impl<'a> Reader<'a> {
    fn new(source: &'a mut dyn Source) -> Self {
        Self {
            source,
            buf: [0; 4096],
            pos: 0,
            len: 0,
        }
    }

    /// The next byte, `None` at the end.
    fn next(&mut self) -> Result<Option<u8>, ImageError> {
        if self.pos == self.len {
            self.len = self.source.read(&mut self.buf)?;
            self.pos = 0;
            if self.len == 0 {
                return Ok(None);
            }
        }
        self.pos += 1;
        Ok(Some(self.buf[self.pos - 1]))
    }

    fn byte(&mut self) -> Result<u8, ImageError> {
        self.next()?.ok_or(ImageError::Invalid("unexpected end"))
    }

    fn u16_be(&mut self) -> Result<u16, ImageError> {
        Ok(u16::from(self.byte()?) << 8 | u16::from(self.byte()?))
    }

    fn u32_be(&mut self) -> Result<u32, ImageError> {
        Ok(u32::from(self.u16_be()?) << 16 | u32::from(self.u16_be()?))
    }

    fn skip(&mut self, mut n: usize) -> Result<(), ImageError> {
        while n > 0 {
            if self.pos == self.len {
                self.byte()?;
                self.pos -= 1;
            }
            let step = n.min(self.len - self.pos);
            self.pos += step;
            n -= step;
        }
        Ok(())
    }

    fn fill(&mut self, out: &mut [u8]) -> Result<(), ImageError> {
        for b in out {
            *b = self.byte()?;
        }
        Ok(())
    }
}

/// A zeroed buffer, or [`ImageError::OutOfMemory`].
pub(crate) fn zeroed<T: Clone + Default>(len: usize) -> Result<alloc::vec::Vec<T>, ImageError> {
    let mut v = alloc::vec::Vec::new();
    v.try_reserve_exact(len)
        .map_err(|_| ImageError::OutOfMemory)?;
    v.resize(len, T::default());
    Ok(v)
}

/// Makes rows smaller, to a size no larger than theirs: each pixel the
/// mean of the pixels it covers, of gray or (with three channels) RGB.
pub(crate) struct Downscaler {
    width: u32,
    height: u32,
    channels: usize,
    out_width: u32,
    out_height: u32,
    /// The pixel of the smaller row each pixel goes to.
    columns: alloc::vec::Vec<u16>,
    /// Pixels of a row each pixel of the smaller one covers.
    counts: alloc::vec::Vec<u16>,
    sums: alloc::vec::Vec<u32>,
    /// Rows summed, and rows come.
    rows: u32,
    seen: u32,
    out: alloc::vec::Vec<u8>,
}

impl Downscaler {
    /// For `width` × `height` made `size` (at most as large), in rows of
    /// `channels` bytes a pixel: 1 (gray) or 3 (RGB).
    pub fn new(
        width: u32,
        height: u32,
        size: (u32, u32),
        channels: usize,
    ) -> Result<Self, ImageError> {
        let out_width = size.0.clamp(1, width.max(1));
        let out_height = size.1.clamp(1, height.max(1));
        if out_width > u32::from(u16::MAX) {
            return Err(ImageError::Unsupported("image this large"));
        }
        let mut columns = alloc::vec::Vec::new();
        let mut counts: alloc::vec::Vec<u16> = zeroed(out_width as usize)?;
        if out_width != width {
            columns
                .try_reserve_exact(width as usize)
                .map_err(|_| ImageError::OutOfMemory)?;
            for x in 0..u64::from(width) {
                let to = (x * u64::from(out_width) / u64::from(width)) as usize;
                columns.push(to as u16);
                counts[to] = counts[to].saturating_add(1);
            }
        } else {
            counts.fill(1);
        }
        let len = out_width as usize * channels;
        Ok(Self {
            width,
            height,
            channels,
            out_width,
            out_height,
            columns,
            counts,
            sums: zeroed(len)?,
            rows: 0,
            seen: 0,
            out: zeroed(len)?,
        })
    }

    pub fn out_width(&self) -> u32 {
        self.out_width
    }

    pub fn out_height(&self) -> u32 {
        self.out_height
    }

    /// The row of the smaller image row `y` goes to.
    fn out_row(&self, y: u32) -> u64 {
        u64::from(y) * u64::from(self.out_height) / u64::from(self.height.max(1))
    }

    fn emit(&self, row: &[u8], sink: &mut dyn ImageSink) -> Result<(), ImageError> {
        if self.channels == 3 {
            sink.row_rgb(row)
        } else {
            sink.row(row)
        }
    }

    /// Adds a row; emits a smaller one when the rows it covers are in.
    pub fn row(&mut self, pixels: &[u8], sink: &mut dyn ImageSink) -> Result<(), ImageError> {
        let ch = self.channels;
        if self.seen >= self.height {
            return Ok(());
        }
        self.seen += 1;
        if self.out_width == self.width && self.out_height == self.height {
            return self.emit(&pixels[..self.width as usize * ch], sink);
        }
        let pixels = &pixels[..self.width as usize * ch];
        if self.columns.is_empty() {
            for (sum, &v) in self.sums.iter_mut().zip(pixels) {
                *sum += u32::from(v);
            }
        } else if ch == 1 {
            for (&to, &v) in self.columns.iter().zip(pixels) {
                self.sums[usize::from(to)] += u32::from(v);
            }
        } else {
            for (&to, pixel) in self.columns.iter().zip(pixels.chunks_exact(ch)) {
                let at = usize::from(to) * ch;
                for (sum, &v) in self.sums[at..at + ch].iter_mut().zip(pixel) {
                    *sum += u32::from(v);
                }
            }
        }
        self.rows += 1;
        if self.seen == self.height || self.out_row(self.seen) != self.out_row(self.seen - 1) {
            self.flush(sink)?;
        }
        Ok(())
    }

    /// Emits a partial last row, if any.
    pub fn finish(&mut self, sink: &mut dyn ImageSink) -> Result<(), ImageError> {
        if self.rows > 0 {
            self.flush(sink)?;
        }
        Ok(())
    }

    fn flush(&mut self, sink: &mut dyn ImageSink) -> Result<(), ImageError> {
        let ch = self.channels;
        for (i, out) in self.out.iter_mut().enumerate() {
            let covered = u32::from(self.counts[i / ch]).max(1) * self.rows;
            *out = (self.sums[i] / covered) as u8;
        }
        self.sums.fill(0);
        self.rows = 0;
        let out = core::mem::take(&mut self.out);
        let result = self.emit(&out, sink);
        self.out = out;
        result
    }
}

/// A decoded image in storage, gray ([`GrayFile`]) or colour
/// ([`ColorFile`]), whichever the file is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageFile {
    pub path: alloc::string::String,
    pub width: u32,
    pub height: u32,
    pub color: bool,
}

impl ImageFile {
    /// Opens a decoded image of either kind, checking its size.
    pub fn open(storage: &dyn quireink_platform::Storage, path: &str) -> Option<ImageFile> {
        if let Some(gray) = GrayFile::open(storage, path) {
            return Some(ImageFile {
                path: gray.path,
                width: gray.width,
                height: gray.height,
                color: false,
            });
        }
        ColorFile::open(storage, path).map(|file| ImageFile {
            path: file.path,
            width: file.width,
            height: file.height,
            color: true,
        })
    }
}
