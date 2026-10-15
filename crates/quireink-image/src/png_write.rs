//! Writing gray PNG files row by row, e.g. screenshots: each row goes to
//! storage as it comes, uncompressed (stored deflate blocks), so writing
//! needs memory for one row only.

use alloc::vec::Vec;

use quireink_platform::Storage;

use crate::ImageError;

/// The CRC-32 of PNG chunks, over `data` continuing `crc`.
fn crc32(mut crc: u32, data: &[u8]) -> u32 {
    crc = !crc;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// Writes a gray PNG to storage, a row at a time; the storage and path
/// are given with each call, so nothing is held between them.
#[derive(Debug)]
pub struct PngWriter {
    width: u32,
    rows_left: u32,
    /// The running Adler-32 of the image data.
    adler: (u32, u32),
}

impl PngWriter {
    /// Starts `path` for a `width`×`height` image.
    pub fn new(
        storage: &dyn Storage,
        path: &str,
        width: u32,
        height: u32,
    ) -> Result<Self, ImageError> {
        if width == 0 || height == 0 || width + 1 > 0xffff {
            return Err(ImageError::Unsupported("PNG size"));
        }
        let mut head = Vec::new();
        head.extend_from_slice(b"\x89PNG\r\n\x1a\n");
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        // 8-bit gray, deflate, no filter method or interlace.
        ihdr.extend_from_slice(&[8, 0, 0, 0, 0]);
        chunk(&mut head, b"IHDR", &ihdr);
        // The zlib header: deflate, no dictionary, fastest.
        chunk(&mut head, b"IDAT", &[0x78, 0x01]);
        storage.write(path, &head).map_err(|_| ImageError::Io)?;
        Ok(Self {
            width,
            rows_left: height,
            adler: (1, 0),
        })
    }

    /// The next row of `path`: `width` gray values (0 black, 255 white).
    pub fn row(
        &mut self,
        storage: &dyn Storage,
        path: &str,
        gray: &[u8],
    ) -> Result<(), ImageError> {
        if self.rows_left == 0 || gray.len() < self.width as usize {
            return Err(ImageError::Invalid("PNG row"));
        }
        self.rows_left -= 1;
        let last = self.rows_left == 0;
        let len = (self.width + 1) as u16;
        let mut block = Vec::with_capacity(5 + len as usize);
        block.push(u8::from(last));
        block.extend_from_slice(&len.to_le_bytes());
        block.extend_from_slice(&(!len).to_le_bytes());
        // Filter type: none.
        block.push(0);
        block.extend_from_slice(&gray[..self.width as usize]);
        for &byte in &block[5..] {
            self.adler.0 = (self.adler.0 + u32::from(byte)) % 65_521;
            self.adler.1 = (self.adler.1 + self.adler.0) % 65_521;
        }
        if last {
            let adler = (self.adler.1 << 16) | self.adler.0;
            block.extend_from_slice(&adler.to_be_bytes());
        }
        let mut out = Vec::with_capacity(block.len() + 12);
        chunk(&mut out, b"IDAT", &block);
        if last {
            chunk(&mut out, b"IEND", &[]);
        }
        storage.append(path, &out).map_err(|_| ImageError::Io)
    }

    /// Whether every row was written.
    pub fn is_complete(&self) -> bool {
        self.rows_left == 0
    }
}

/// Appends a chunk of `kind` with `data` to `out`.
fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32(crc32(0, kind), data);
    out.extend_from_slice(&crc.to_be_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_of_iend() {
        // The well-known CRC of an empty IEND chunk.
        assert_eq!(crc32(0, b"IEND"), 0xae42_6082);
    }
}
