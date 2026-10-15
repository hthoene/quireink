//! A disk of 512-byte blocks (an SD card) as the bytes a FAT file system
//! reads and writes: [`BlockDisk`] keeps one block in memory and writes
//! it back when another is needed or on flush.

use fatfs::{IoBase, Read, Seek, SeekFrom, Write};

/// Size of a block.
pub const BLOCK: usize = 512;

/// The disk failed to read or write a block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiskError;

impl fatfs::IoError for DiskError {
    fn is_interrupted(&self) -> bool {
        false
    }
    fn new_unexpected_eof_error() -> Self {
        Self
    }
    fn new_write_zero_error() -> Self {
        Self
    }
}

/// A disk read and written in whole blocks, e.g. an SD card.
pub trait Blocks {
    /// Reads block `index` into `block`.
    fn read_block(&mut self, index: u32, block: &mut [u8; BLOCK]) -> Result<(), DiskError>;
    /// Writes `block` as block `index`.
    fn write_block(&mut self, index: u32, block: &[u8; BLOCK]) -> Result<(), DiskError>;
    /// Number of blocks.
    fn blocks(&mut self) -> Result<u32, DiskError>;
}

/// The bytes of a [`Blocks`] disk.
#[derive(Debug)]
pub struct BlockDisk<B: Blocks> {
    disk: B,
    position: u64,
    size: u64,
    block: [u8; BLOCK],
    /// The block in `block`, and whether it changed.
    cached: Option<u32>,
    dirty: bool,
}

impl<B: Blocks> BlockDisk<B> {
    pub fn new(mut disk: B) -> Result<Self, DiskError> {
        let size = u64::from(disk.blocks()?) * BLOCK as u64;
        Ok(Self {
            disk,
            position: 0,
            size,
            block: [0; BLOCK],
            cached: None,
            dirty: false,
        })
    }

    /// Makes block `index` the one in memory, writing back the one there.
    fn load(&mut self, index: u32, read: bool) -> Result<(), DiskError> {
        if self.cached == Some(index) {
            return Ok(());
        }
        self.write_back()?;
        if read {
            self.disk.read_block(index, &mut self.block)?;
        }
        self.cached = Some(index);
        Ok(())
    }

    fn write_back(&mut self) -> Result<(), DiskError> {
        if let (true, Some(index)) = (self.dirty, self.cached) {
            self.disk.write_block(index, &self.block)?;
            self.dirty = false;
        }
        Ok(())
    }

    /// The block at the position and the offset in it.
    fn place(&self) -> (u32, usize) {
        (
            (self.position / BLOCK as u64) as u32,
            (self.position % BLOCK as u64) as usize,
        )
    }
}

impl<B: Blocks> IoBase for BlockDisk<B> {
    type Error = DiskError;
}

impl<B: Blocks> Read for BlockDisk<B> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, DiskError> {
        let left = self.size.saturating_sub(self.position);
        let len = buf.len().min(left as usize);
        let mut done = 0;
        while done < len {
            let (index, offset) = self.place();
            self.load(index, true)?;
            let n = (BLOCK - offset).min(len - done);
            buf[done..done + n].copy_from_slice(&self.block[offset..offset + n]);
            done += n;
            self.position += n as u64;
        }
        Ok(done)
    }
}

impl<B: Blocks> Write for BlockDisk<B> {
    fn write(&mut self, buf: &[u8]) -> Result<usize, DiskError> {
        let left = self.size.saturating_sub(self.position);
        let len = buf.len().min(left as usize);
        let mut done = 0;
        while done < len {
            let (index, offset) = self.place();
            let n = (BLOCK - offset).min(len - done);
            // A whole block need not be read first.
            self.load(index, n < BLOCK)?;
            self.block[offset..offset + n].copy_from_slice(&buf[done..done + n]);
            self.dirty = true;
            done += n;
            self.position += n as u64;
        }
        Ok(done)
    }

    fn flush(&mut self) -> Result<(), DiskError> {
        self.write_back()
    }
}

impl<B: Blocks> Seek for BlockDisk<B> {
    fn seek(&mut self, to: SeekFrom) -> Result<u64, DiskError> {
        let position = match to {
            SeekFrom::Start(n) => i128::from(n),
            SeekFrom::End(n) => i128::from(self.size) + i128::from(n),
            SeekFrom::Current(n) => i128::from(self.position) + i128::from(n),
        };
        if position < 0 || position > i128::from(self.size) {
            return Err(DiskError);
        }
        self.position = position as u64;
        Ok(self.position)
    }
}
