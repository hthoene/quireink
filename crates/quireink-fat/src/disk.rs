//! A disk of 512-byte blocks (an SD card) as the bytes a FAT file system
//! reads and writes: [`BlockDisk`] keeps one block in memory and writes
//! it back when another is needed or on flush.
//!
//! [`BlockDisk::volume`] finds the FAT volume on a card: most cards hold a
//! partition table (MBR, sometimes GPT) and the file system in a partition
//! after it; some hold the file system from the first block.

use alloc::vec::Vec;

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

/// Why no FAT volume was found on a disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeError {
    /// The disk could not be read.
    Disk,
    /// It is formatted as exFAT (or NTFS), not FAT32.
    ExFat,
    /// Neither a FAT volume nor a partition with one.
    NoFat,
}

impl From<DiskError> for VolumeError {
    fn from(_: DiskError) -> Self {
        Self::Disk
    }
}

/// What the first block of a volume says it is.
#[derive(PartialEq)]
enum Boot {
    Fat,
    ExFat,
    Other,
}

fn boot(block: &[u8; BLOCK]) -> Boot {
    if matches!(&block[3..11], b"EXFAT   " | b"NTFS    ") {
        return Boot::ExFat;
    }
    let jump = matches!(block[0], 0xEB | 0xE9);
    // FAT12 and FAT16 name their type at 0x36, FAT32 at 0x52.
    let named = &block[0x36..0x39] == b"FAT" || &block[0x52..0x55] == b"FAT";
    let sector = u16::from_le_bytes([block[11], block[12]]);
    if jump && named && sector.is_power_of_two() && (512..=4096).contains(&sector) {
        Boot::Fat
    } else {
        Boot::Other
    }
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    u64::from(u32_at(bytes, at)) | u64::from(u32_at(bytes, at + 4)) << 32
}

/// The partitions in the partition table `mbr` (the disk's first block):
/// the first block of each and its number of blocks.
fn partitions<B: Blocks>(disk: &mut B, mbr: &[u8; BLOCK]) -> Result<Vec<(u64, u64)>, DiskError> {
    let mut found = Vec::new();
    if mbr[510..] != [0x55, 0xAA] {
        return Ok(found);
    }
    for entry in mbr[446..510].chunks_exact(16) {
        match entry[4] {
            0 => {}
            // A protective entry: the table is a GPT.
            0xEE => return gpt(disk),
            _ => found.push((u64::from(u32_at(entry, 8)), u64::from(u32_at(entry, 12)))),
        }
    }
    Ok(found)
}

/// The partitions of a GPT, whose header is in the second block.
fn gpt<B: Blocks>(disk: &mut B) -> Result<Vec<(u64, u64)>, DiskError> {
    let mut found = Vec::new();
    let mut block = [0u8; BLOCK];
    disk.read_block(1, &mut block)?;
    if &block[..8] != b"EFI PART" {
        return Ok(found);
    }
    let first_entry = u64_at(&block, 72);
    let count = u64::from(u32_at(&block, 80).min(128));
    let size = u64::from(u32_at(&block, 84));
    if !(128..=BLOCK as u64).contains(&size) || BLOCK as u64 % size != 0 {
        return Ok(found);
    }
    let per_block = BLOCK as u64 / size;
    for i in 0..count {
        if i % per_block == 0 {
            let index = u32::try_from(first_entry + i / per_block).map_err(|_| DiskError)?;
            disk.read_block(index, &mut block)?;
        }
        let at = ((i % per_block) * size) as usize;
        let entry = &block[at..at + size as usize];
        if entry[..16].iter().any(|&b| b != 0) {
            let (first, last) = (u64_at(entry, 32), u64_at(entry, 40));
            if last >= first {
                found.push((first, last - first + 1));
            }
        }
    }
    Ok(found)
}

/// The bytes of a [`Blocks`] disk, or of the part of it a volume takes.
#[derive(Debug)]
pub struct BlockDisk<B: Blocks> {
    disk: B,
    /// The disk's block the bytes start at.
    first: u32,
    position: u64,
    size: u64,
    block: [u8; BLOCK],
    /// The block in `block`, and whether it changed.
    cached: Option<u32>,
    dirty: bool,
}

impl<B: Blocks> BlockDisk<B> {
    /// The whole disk.
    pub fn new(mut disk: B) -> Result<Self, DiskError> {
        let blocks = disk.blocks()?;
        Ok(Self::part(disk, 0, blocks))
    }

    /// The FAT volume on the disk: the whole of it, or the first partition
    /// that holds one.
    pub fn volume(mut disk: B) -> Result<Self, VolumeError> {
        let blocks = disk.blocks()?;
        let mut block = [0u8; BLOCK];
        disk.read_block(0, &mut block)?;
        match boot(&block) {
            Boot::Fat => return Ok(Self::part(disk, 0, blocks)),
            Boot::ExFat => return Err(VolumeError::ExFat),
            Boot::Other => {}
        }
        let mut exfat = false;
        for (first, count) in partitions(&mut disk, &block)? {
            if first == 0 || count == 0 || first.saturating_add(count) > u64::from(blocks) {
                continue;
            }
            // Within the disk, so both fit in a u32.
            let (first, count) = (first as u32, count as u32);
            disk.read_block(first, &mut block)?;
            match boot(&block) {
                Boot::Fat => return Ok(Self::part(disk, first, count)),
                Boot::ExFat => exfat = true,
                Boot::Other => {}
            }
        }
        Err(if exfat {
            VolumeError::ExFat
        } else {
            VolumeError::NoFat
        })
    }

    /// `blocks` blocks of the disk from block `first` on.
    fn part(disk: B, first: u32, blocks: u32) -> Self {
        Self {
            disk,
            first,
            position: 0,
            size: u64::from(blocks) * BLOCK as u64,
            block: [0; BLOCK],
            cached: None,
            dirty: false,
        }
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

    /// The disk's block at the position and the offset in it.
    fn place(&self) -> (u32, usize) {
        (
            self.first + (self.position / BLOCK as u64) as u32,
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
