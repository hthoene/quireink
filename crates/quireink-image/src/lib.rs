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

/// Decodes an image of `format`, downscaled so that neither side exceeds
/// `max_side` (as far as the format's scaling steps allow; JPEG scales by
/// 1/2, 1/4 or 1/8).
pub fn decode(
    format: Format,
    source: &mut dyn Source,
    max_side: u32,
    sink: &mut dyn ImageSink,
) -> Result<(), ImageError> {
    let mut reader = Reader::new(source);
    match format {
        Format::Jpeg => jpeg::decode(&mut reader, max_side, sink),
        Format::Png => png::decode(&mut reader, max_side, sink),
        Format::Gif => gif::decode(&mut reader, max_side, sink),
    }
}

/// Buffered byte reader over a [`Source`].
pub(crate) struct Reader<'a> {
    source: &'a mut dyn Source,
    buf: [u8; 512],
    pos: usize,
    len: usize,
}

impl<'a> Reader<'a> {
    fn new(source: &'a mut dyn Source) -> Self {
        Self {
            source,
            buf: [0; 512],
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

/// Averages `factor`×`factor` blocks of rows into downscaled rows, of
/// gray or (with three channels) RGB.
pub(crate) struct Downscaler {
    factor: u32,
    width: u32,
    channels: usize,
    out_width: u32,
    sums: alloc::vec::Vec<u32>,
    rows: u32,
    out: alloc::vec::Vec<u8>,
}

impl Downscaler {
    /// For rows of `channels` bytes a pixel: 1 (gray) or 3 (RGB).
    pub fn new(width: u32, factor: u32, channels: usize) -> Result<Self, ImageError> {
        let factor = factor.max(1);
        let out_width = width.div_ceil(factor);
        let len = out_width as usize * channels;
        Ok(Self {
            factor,
            width,
            channels,
            out_width,
            sums: zeroed(len)?,
            rows: 0,
            out: zeroed(len)?,
        })
    }

    pub fn out_width(&self) -> u32 {
        self.out_width
    }

    fn emit(&self, row: &[u8], sink: &mut dyn ImageSink) -> Result<(), ImageError> {
        if self.channels == 3 {
            sink.row_rgb(row)
        } else {
            sink.row(row)
        }
    }

    /// Adds a full-resolution row; emits a downscaled row every `factor`.
    pub fn row(&mut self, pixels: &[u8], sink: &mut dyn ImageSink) -> Result<(), ImageError> {
        let ch = self.channels;
        if self.factor == 1 {
            return self.emit(&pixels[..self.width as usize * ch], sink);
        }
        let factor = self.factor as usize;
        for (i, &v) in pixels.iter().take(self.width as usize * ch).enumerate() {
            let (x, c) = (i / ch, i % ch);
            self.sums[x / factor * ch + c] += u32::from(v);
        }
        self.rows += 1;
        if self.rows == self.factor {
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
            let x = (i / ch) as u32;
            let columns = (self.width - x * self.factor).min(self.factor);
            *out = (self.sums[i] / (columns * self.rows)) as u8;
        }
        self.sums.fill(0);
        self.rows = 0;
        let out = core::mem::take(&mut self.out);
        let result = self.emit(&out, sink);
        self.out = out;
        result
    }
}

/// The smallest integer factor that brings the larger side to `max_side`.
pub(crate) fn factor_for(width: u32, height: u32, max_side: u32) -> u32 {
    width.max(height).div_ceil(max_side.max(1)).max(1)
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
