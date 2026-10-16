//! Decoded colour images on storage: `QICI`, width and height (u32), rows
//! of RGB, three bytes a pixel (see ADR 0015). Written by a sink that
//! asks decoders for colour; gray rows it is given are stored as gray
//! RGB, so an image without colour, or a decoder that only gives gray,
//! still makes a file.

use alloc::string::String;
use alloc::vec::Vec;

use quireink_platform::Storage;

use crate::{ImageError, ImageSink};

const MAGIC: &[u8; 4] = b"QICI";
const HEADER: u64 = 12;
/// Rows are appended in chunks of about this size.
const FLUSH: usize = 6144;

/// A decoded colour image in storage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColorFile {
    pub path: String,
    pub width: u32,
    pub height: u32,
}

impl ColorFile {
    /// Opens a decoded colour image, checking its size.
    pub fn open(storage: &dyn Storage, path: &str) -> Option<ColorFile> {
        let mut header = [0u8; HEADER as usize];
        if storage.read_at(path, 0, &mut header).ok()? != header.len() || &header[..4] != MAGIC {
            return None;
        }
        let width = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
        let height = u32::from_le_bytes([header[8], header[9], header[10], header[11]]);
        let expected = HEADER + u64::from(width) * u64::from(height) * 3;
        (width > 0 && height > 0 && storage.size(path).ok()? == expected).then(|| ColorFile {
            path: String::from(path),
            width,
            height,
        })
    }

    /// Reads row `y` into `out` (at most `width` pixels, three bytes each).
    pub fn read_row(&self, storage: &dyn Storage, y: u32, out: &mut [u8]) -> bool {
        let len = out.len().min(self.width as usize * 3);
        let at = HEADER + u64::from(y.min(self.height - 1)) * u64::from(self.width) * 3;
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

/// Writes a decoded colour image to storage as it is decoded.
pub struct ColorWriter<'a> {
    storage: &'a dyn Storage,
    path: &'a str,
    buffer: Vec<u8>,
}

impl core::fmt::Debug for ColorWriter<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ColorWriter")
            .field("path", &self.path)
            .finish()
    }
}

impl<'a> ColorWriter<'a> {
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

    fn added(&mut self) -> Result<(), ImageError> {
        if self.buffer.len() >= FLUSH {
            self.flush()?;
        }
        Ok(())
    }
}

impl ImageSink for ColorWriter<'_> {
    fn start(&mut self, width: u32, height: u32) -> Result<(), ImageError> {
        let mut header = Vec::with_capacity(HEADER as usize);
        header.extend_from_slice(MAGIC);
        header.extend_from_slice(&width.to_le_bytes());
        header.extend_from_slice(&height.to_le_bytes());
        self.storage
            .write(self.path, &header)
            .map_err(|_| ImageError::Io)
    }

    fn wants_color(&self) -> bool {
        true
    }

    fn row(&mut self, gray: &[u8]) -> Result<(), ImageError> {
        for &g in gray {
            self.buffer.extend_from_slice(&[g, g, g]);
        }
        self.added()
    }

    fn row_rgb(&mut self, rgb: &[u8]) -> Result<(), ImageError> {
        self.buffer.extend_from_slice(rgb);
        self.added()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::cell::RefCell;
    use quireink_platform::{DirEntry, StorageError};

    /// One file in memory.
    #[derive(Default)]
    struct OneFile(RefCell<Vec<u8>>);

    impl Storage for OneFile {
        fn read_at(&self, _: &str, offset: u64, buf: &mut [u8]) -> Result<usize, StorageError> {
            let data = self.0.borrow();
            let start = (offset as usize).min(data.len());
            let n = buf.len().min(data.len() - start);
            buf[..n].copy_from_slice(&data[start..start + n]);
            Ok(n)
        }
        fn size(&self, _: &str) -> Result<u64, StorageError> {
            Ok(self.0.borrow().len() as u64)
        }
        fn write(&self, _: &str, data: &[u8]) -> Result<(), StorageError> {
            *self.0.borrow_mut() = data.to_vec();
            Ok(())
        }
        fn append(&self, _: &str, data: &[u8]) -> Result<(), StorageError> {
            self.0.borrow_mut().extend_from_slice(data);
            Ok(())
        }
        fn list(&self, _: &str, _: &mut dyn FnMut(DirEntry<'_>)) -> Result<(), StorageError> {
            Ok(())
        }
        fn remove(&self, _: &str) -> Result<(), StorageError> {
            Ok(())
        }
    }

    #[test]
    fn rows_round_trip_gray_or_colour() {
        let storage = OneFile::default();
        let mut writer = ColorWriter::new(&storage, "c.rgb");
        writer.start(2, 2).unwrap();
        writer.row_rgb(&[1, 2, 3, 4, 5, 6]).unwrap();
        writer.row(&[7, 8]).unwrap();
        writer.finish().unwrap();
        let file = ColorFile::open(&storage, "c.rgb").unwrap();
        assert_eq!((file.width, file.height), (2, 2));
        let mut row = [0u8; 6];
        assert!(file.read_row(&storage, 0, &mut row));
        assert_eq!(row, [1, 2, 3, 4, 5, 6]);
        assert!(file.read_row(&storage, 1, &mut row));
        assert_eq!(row, [7, 7, 7, 8, 8, 8]);
        // A gray file is not a colour one.
        assert!(crate::GrayFile::open(&storage, "c.rgb").is_none());
    }
}
