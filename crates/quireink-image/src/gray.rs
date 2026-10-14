//! Decoded images on storage: `QIGI`, width and height (u32), rows of
//! 8-bit gray.

use alloc::string::String;
use alloc::vec::Vec;

use quireink_platform::Storage;

use crate::{ImageError, ImageSink};

const MAGIC: &[u8; 4] = b"QIGI";
const HEADER: u64 = 12;
/// Rows are appended in chunks of about this size.
const FLUSH: usize = 2048;

/// A decoded image in storage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrayFile {
    pub path: String,
    pub width: u32,
    pub height: u32,
}

impl GrayFile {
    /// Opens a decoded image, checking its size.
    pub fn open(storage: &dyn Storage, path: &str) -> Option<GrayFile> {
        let mut header = [0u8; HEADER as usize];
        if storage.read_at(path, 0, &mut header).ok()? != header.len() || &header[..4] != MAGIC {
            return None;
        }
        let width = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
        let height = u32::from_le_bytes([header[8], header[9], header[10], header[11]]);
        let expected = HEADER + u64::from(width) * u64::from(height);
        (width > 0 && height > 0 && storage.size(path).ok()? == expected).then(|| GrayFile {
            path: String::from(path),
            width,
            height,
        })
    }

    /// Reads row `y` into `out` (at most `width` bytes).
    pub fn read_row(&self, storage: &dyn Storage, y: u32, out: &mut [u8]) -> bool {
        let len = out.len().min(self.width as usize);
        let at = HEADER + u64::from(y.min(self.height - 1)) * u64::from(self.width);
        let mut filled = 0;
        while filled < len {
            match storage.read_at(&self.path, at + filled as u64, &mut out[filled..len]) {
                Ok(0) | Err(_) => return false,
                Ok(n) => filled += n,
            }
        }
        true
    }
}

/// Writes a decoded image to storage as it is decoded.
pub struct GrayWriter<'a> {
    storage: &'a dyn Storage,
    path: &'a str,
    buffer: Vec<u8>,
}

impl core::fmt::Debug for GrayWriter<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("GrayWriter")
            .field("path", &self.path)
            .finish()
    }
}

impl<'a> GrayWriter<'a> {
    pub fn new(storage: &'a dyn Storage, path: &'a str) -> Self {
        Self {
            storage,
            path,
            buffer: Vec::new(),
        }
    }

    /// Writes what is buffered; call when decoding is complete.
    pub fn finish(mut self) -> Result<(), ImageError> {
        self.flush()
    }

    fn flush(&mut self) -> Result<(), ImageError> {
        if !self.buffer.is_empty() {
            self.storage
                .append(self.path, &self.buffer)
                .map_err(|_| ImageError::Io)?;
            self.buffer.clear();
        }
        Ok(())
    }
}

impl ImageSink for GrayWriter<'_> {
    fn start(&mut self, width: u32, height: u32) -> Result<(), ImageError> {
        let mut header = Vec::with_capacity(HEADER as usize);
        header.extend_from_slice(MAGIC);
        header.extend_from_slice(&width.to_le_bytes());
        header.extend_from_slice(&height.to_le_bytes());
        self.storage
            .write(self.path, &header)
            .map_err(|_| ImageError::Io)
    }

    fn row(&mut self, gray: &[u8]) -> Result<(), ImageError> {
        self.buffer.extend_from_slice(gray);
        if self.buffer.len() >= FLUSH {
            self.flush()?;
        }
        Ok(())
    }
}
