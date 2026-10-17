//! A disk of 512-byte blocks (an SD card) as the bytes a FAT file system
//! reads and writes: [`BlockDisk`] keeps the [`SLOTS`] blocks used last in
//! memory (the FAT, folders and the files being read stay at hand: each
//! block read from a card costs a command and its wait) and writes changed
//! ones back when their place is needed or on flush. Whole blocks of a
//! file read or written pass by them, several in one command: a picture
//! streamed through would push the FAT and the folders out.
//!
//! [`BlockDisk::volume`] finds the FAT volume on a card: most cards hold a
//! partition table (MBR, sometimes GPT) and the file system in a partition
//! after it; some hold the file system from the first block.

use alloc::vec::Vec;

use fatfs::{IoBase, Read, Seek, SeekFrom, Write};

/// Size of a block.
pub const BLOCK: usize = 512;
/// Blocks kept in memory.
pub const SLOTS: usize = 24;

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
    /// Reads blocks from `index` on into `data` (whole blocks), in one
    /// command where the disk can.
    fn read_blocks(&mut self, index: u32, data: &mut [u8]) -> Result<(), DiskError> {
        (index..)
            .zip(data.chunks_exact_mut(BLOCK))
            .try_for_each(|(i, block)| {
                let mut one = [0u8; BLOCK];
                self.read_block(i, &mut one)?;
                block.copy_from_slice(&one);
                Ok(())
            })
    }
    /// Writes `data` (whole blocks) as blocks from `index` on, in one
    /// command where the disk can.
    fn write_blocks(&mut self, index: u32, data: &[u8]) -> Result<(), DiskError> {
        (index..)
            .zip(data.chunks_exact(BLOCK))
            .try_for_each(|(i, block)| {
                let mut one = [0u8; BLOCK];
                one.copy_from_slice(block);
                self.write_block(i, &one)
            })
    }
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

/// A block kept in memory.
#[derive(Debug, Clone, Copy)]
struct Slot {
    /// The disk's block it holds, if any.
    index: Option<u32>,
    /// Changed since it was read.
    dirty: bool,
    /// When it was used last (a count of uses).
    used: u32,
    data: [u8; BLOCK],
}

const EMPTY: Slot = Slot {
    index: None,
    dirty: false,
    used: 0,
    data: [0; BLOCK],
};

/// The bytes of a [`Blocks`] disk, or of the part of it a volume takes.
#[derive(Debug)]
pub struct BlockDisk<B: Blocks> {
    disk: B,
    /// The disk's block the bytes start at.
    first: u32,
    position: u64,
    size: u64,
    slots: [Slot; SLOTS],
    /// Uses so far, to find the block used longest ago.
    uses: u32,
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
            slots: [EMPTY; SLOTS],
            uses: 0,
        }
    }

    /// The slot holding block `index`, read into the one used longest ago
    /// (written back first if it changed); `read` false for a block about
    /// to be written whole.
    fn load(&mut self, index: u32, read: bool) -> Result<usize, DiskError> {
        self.uses = self.uses.wrapping_add(1);
        if let Some(i) = self.slots.iter().position(|s| s.index == Some(index)) {
            self.slots[i].used = self.uses;
            return Ok(i);
        }
        let i = (0..SLOTS)
            .min_by_key(|&i| self.slots[i].index.map_or(0, |_| self.slots[i].used))
            .unwrap_or(0);
        self.write_back(i)?;
        let slot = &mut self.slots[i];
        slot.index = None;
        if read {
            self.disk.read_block(index, &mut slot.data)?;
        }
        slot.index = Some(index);
        slot.used = self.uses;
        Ok(i)
    }

    /// The disk, for tests.
    #[cfg(test)]
    pub(crate) fn disk(&self) -> &B {
        &self.disk
    }

    /// Whether block `index` is kept in a slot.
    fn kept(&self, index: u32) -> bool {
        self.slots.iter().any(|s| s.index == Some(index))
    }

    /// Whole blocks from `index` on, at most `whole`, that are not kept.
    fn passing(&self, index: u32, whole: usize) -> usize {
        (0..whole as u32)
            .take_while(|&k| !self.kept(index + k))
            .count()
    }

    fn write_back(&mut self, i: usize) -> Result<(), DiskError> {
        let slot = &mut self.slots[i];
        if let (true, Some(index)) = (slot.dirty, slot.index) {
            self.disk.write_block(index, &slot.data)?;
            slot.dirty = false;
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
            let whole = if offset == 0 { (len - done) / BLOCK } else { 0 };
            let passing = self.passing(index, whole);
            if passing > 0 {
                let bytes = &mut buf[done..done + passing * BLOCK];
                self.disk.read_blocks(index, bytes)?;
                done += passing * BLOCK;
                self.position += (passing * BLOCK) as u64;
                continue;
            }
            let slot = self.load(index, true)?;
            let n = (BLOCK - offset).min(len - done);
            buf[done..done + n].copy_from_slice(&self.slots[slot].data[offset..offset + n]);
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
            let whole = if offset == 0 { (len - done) / BLOCK } else { 0 };
            let passing = self.passing(index, whole);
            if passing > 0 {
                let bytes = &buf[done..done + passing * BLOCK];
                self.disk.write_blocks(index, bytes)?;
                done += passing * BLOCK;
                self.position += (passing * BLOCK) as u64;
                continue;
            }
            let n = (BLOCK - offset).min(len - done);
            // A whole block need not be read first.
            let slot = &mut self.slots[self.load(index, n < BLOCK)?];
            slot.data[offset..offset + n].copy_from_slice(&buf[done..done + n]);
            slot.dirty = true;
            done += n;
            self.position += n as u64;
        }
        Ok(done)
    }

    fn flush(&mut self) -> Result<(), DiskError> {
        (0..SLOTS).try_for_each(|i| self.write_back(i))
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
