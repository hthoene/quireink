//! A streaming ZIP reader.
//!
//! Only an index of the central directory is kept in memory: 8 bytes per
//! entry (name hash and directory offset); details are read from storage
//! on lookup. Entries are inflated chunk by chunk, so a large EPUB never has
//! to fit into RAM. Supports the "stored" and
//! "deflate" methods, which cover EPUB and most other ZIP-based formats.

#![no_std]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use miniz_oxide::inflate::TINFLStatus;
use miniz_oxide::inflate::core::{DecompressorOxide, decompress, inflate_flags};
use quireink_platform::{Storage, StorageError};

/// Size of the end-of-central-directory record without comment.
const EOCD_SIZE: u64 = 22;
/// Largest ZIP comment, which may follow the EOCD record.
const MAX_COMMENT: u64 = 0xffff;
/// Chunk size for reading compressed data.
const CHUNK: usize = 1024;
/// The deflate history: back-references reach at most this far.
const WINDOW: usize = 32 * 1024;

/// Why an archive could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ZipError {
    Storage(StorageError),
    /// Not a ZIP archive, or a damaged one.
    Invalid(&'static str),
    /// A feature this reader does not implement (encryption, ZIP64, other
    /// compression methods).
    Unsupported(&'static str),
    /// The entry is larger than allowed by the caller.
    TooLarge,
    /// Memory for buffers could not be allocated.
    OutOfMemory,
}

impl fmt::Display for ZipError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ZipError::Storage(e) => write!(f, "storage: {e}"),
            ZipError::Invalid(what) => write!(f, "invalid archive: {what}"),
            ZipError::Unsupported(what) => write!(f, "unsupported: {what}"),
            ZipError::TooLarge => f.write_str("entry too large"),
            ZipError::OutOfMemory => f.write_str("not enough memory"),
        }
    }
}

impl core::error::Error for ZipError {}

impl From<StorageError> for ZipError {
    fn from(e: StorageError) -> Self {
        ZipError::Storage(e)
    }
}

/// Compression method of an entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Stored,
    Deflate,
}

/// An entry of the central directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub method: Method,
    pub compressed_size: u32,
    pub size: u32,
    /// Offset of the local file header.
    header_offset: u32,
    /// Offset of the central directory record.
    record: u32,
}

impl Entry {
    /// Identifies the entry within its archive; see
    /// [`ZipArchive::entry_by_record`].
    pub fn record(&self) -> u32 {
        self.record
    }
}

/// An open ZIP archive.
#[derive(Debug, Clone)]
pub struct ZipArchive {
    path: String,
    /// `(name hash, offset of the central directory record)`, sorted.
    index: Vec<(u32, u32)>,
}

/// FNV-1a, to find entries without keeping their names in memory.
fn hash(name: &[u8]) -> u32 {
    name.iter().fold(0x811c_9dc5, |hash, &b| {
        (hash ^ u32::from(b)).wrapping_mul(0x0100_0193)
    })
}

/// Size of a central directory record without name, extra and comment.
const RECORD: usize = 46;

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn read_exact(
    storage: &dyn Storage,
    path: &str,
    offset: u64,
    len: usize,
) -> Result<Vec<u8>, ZipError> {
    let mut buf = Vec::new();
    buf.try_reserve_exact(len)
        .map_err(|_| ZipError::OutOfMemory)?;
    buf.resize(len, 0);
    let mut filled = 0;
    while filled < len {
        let n = storage.read_at(path, offset + filled as u64, &mut buf[filled..])?;
        if n == 0 {
            return Err(ZipError::Invalid("unexpected end of file"));
        }
        filled += n;
    }
    Ok(buf)
}

/// A zero-filled buffer, or [`ZipError::OutOfMemory`].
fn zeroed(len: usize) -> Result<Vec<u8>, ZipError> {
    let mut buffer = Vec::new();
    buffer
        .try_reserve_exact(len)
        .map_err(|_| ZipError::OutOfMemory)?;
    buffer.resize(len, 0);
    Ok(buffer)
}

/// Finds the end-of-central-directory record, searching backwards from
/// the end of the file in small steps: it usually is the last 22 bytes,
/// but an archive comment may follow it.
fn find_end_record(storage: &dyn Storage, path: &str, size: u64) -> Result<u64, ZipError> {
    const STEP: u64 = 1024;
    if size < EOCD_SIZE {
        return Err(ZipError::Invalid("too small"));
    }
    let lowest = size.saturating_sub(EOCD_SIZE + MAX_COMMENT);
    let mut end = size;
    while end > lowest {
        let start = end.saturating_sub(STEP).max(lowest);
        // Overlap by three bytes so a signature across steps is found.
        let len = ((end + 3).min(size) - start) as usize;
        let window = read_exact(storage, path, start, len)?;
        let found =
            window.windows(4).enumerate().rev().find(|(i, w)| {
                *w == [0x50, 0x4b, 0x05, 0x06] && start + *i as u64 + EOCD_SIZE <= size
            });
        if let Some((i, _)) = found {
            return Ok(start + i as u64);
        }
        end = start;
    }
    Err(ZipError::Invalid("no end of central directory"))
}

impl ZipArchive {
    /// Reads the central directory of the archive at `path`, entry by
    /// entry, so even large directories need little memory.
    pub fn open(storage: &dyn Storage, path: &str) -> Result<ZipArchive, ZipError> {
        let size = storage.size(path)?;
        let eocd = find_end_record(storage, path, size)?;
        let record = read_exact(storage, path, eocd, EOCD_SIZE as usize)?;
        let truncated = ZipError::Invalid("truncated");
        let count = u16_at(&record, 10).ok_or(truncated.clone())?;
        let cd_size = u32_at(&record, 12).ok_or(truncated.clone())?;
        let cd_offset = u32_at(&record, 16).ok_or(truncated)?;
        if count == 0xffff || cd_offset == 0xffff_ffff {
            return Err(ZipError::Unsupported("ZIP64"));
        }
        let cd_end = u64::from(cd_offset) + u64::from(cd_size);
        if cd_end > size {
            return Err(ZipError::Invalid("central directory out of range"));
        }

        let mut index = Vec::new();
        index
            .try_reserve_exact(usize::from(count))
            .map_err(|_| ZipError::OutOfMemory)?;
        let mut at = u64::from(cd_offset);
        let bad = ZipError::Invalid("bad central directory entry");
        for _ in 0..count {
            if at + RECORD as u64 > cd_end || at > u64::from(u32::MAX) {
                return Err(bad);
            }
            let head = read_exact(storage, path, at, RECORD)?;
            let field = |offset| u16_at(&head, offset).map(u64::from).ok_or(bad.clone());
            if u32_at(&head, 0) != Some(0x0201_4b50) {
                return Err(bad);
            }
            let name_len = field(28)?;
            let name = read_exact(storage, path, at + RECORD as u64, name_len as usize)?;
            index.push((hash(&name), at as u32));
            at += RECORD as u64 + name_len + field(30)? + field(32)?;
        }
        index.sort_unstable();
        Ok(ZipArchive {
            path: String::from(path),
            index,
        })
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.index.len()
    }

    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    /// All entries, read from the central directory one at a time (in no
    /// particular order), e.g. to list the images of a comic archive.
    pub fn entries<'a>(
        &'a self,
        storage: &'a dyn Storage,
    ) -> impl Iterator<Item = Result<Entry, ZipError>> + 'a {
        self.index
            .iter()
            .map(move |&(_, offset)| self.entry_by_record(storage, offset))
    }

    /// The entry named `name` (exact, case-sensitive path inside the
    /// archive), read from the central directory.
    pub fn entry(&self, storage: &dyn Storage, name: &str) -> Result<Option<Entry>, ZipError> {
        let wanted = hash(name.as_bytes());
        let first = self.index.partition_point(|&(h, _)| h < wanted);
        for &(h, offset) in &self.index[first..] {
            if h != wanted {
                break;
            }
            let entry = self.entry_by_record(storage, offset)?;
            if entry.name == name {
                return Ok(Some(entry));
            }
        }
        Ok(None)
    }

    /// The entry whose [`Entry::record`] is `offset`, a compact way to
    /// remember entries without their names.
    pub fn entry_by_record(&self, storage: &dyn Storage, offset: u32) -> Result<Entry, ZipError> {
        let bad = ZipError::Invalid("bad central directory entry");
        let head = read_exact(storage, &self.path, u64::from(offset), RECORD)?;
        let flags = u16_at(&head, 8).ok_or(bad.clone())?;
        let method = match u16_at(&head, 10).ok_or(bad.clone())? {
            0 => Method::Stored,
            8 => Method::Deflate,
            _ => return Err(ZipError::Unsupported("compression method")),
        };
        if flags & 1 != 0 {
            return Err(ZipError::Unsupported("encrypted entries"));
        }
        let name_len = usize::from(u16_at(&head, 28).ok_or(bad.clone())?);
        let name = read_exact(
            storage,
            &self.path,
            u64::from(offset) + RECORD as u64,
            name_len,
        )?;
        Ok(Entry {
            name: String::from_utf8_lossy(&name).into_owned(),
            method,
            compressed_size: u32_at(&head, 20).ok_or(bad.clone())?,
            size: u32_at(&head, 24).ok_or(bad.clone())?,
            header_offset: u32_at(&head, 42).ok_or(bad)?,
            record: offset,
        })
    }

    /// Where a stored (uncompressed) entry's bytes are in the archive file,
    /// as `(offset, length)`, to read them in place; `None` for compressed
    /// entries.
    pub fn stored_range(
        &self,
        storage: &dyn Storage,
        entry: &Entry,
    ) -> Result<Option<(u64, u64)>, ZipError> {
        if entry.method != Method::Stored {
            return Ok(None);
        }
        let offset = self.data_offset(storage, entry)?;
        Ok(Some((offset, u64::from(entry.size))))
    }

    /// The path of the archive file.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Offset of an entry's data, after its local header.
    fn data_offset(&self, storage: &dyn Storage, entry: &Entry) -> Result<u64, ZipError> {
        let header = read_exact(storage, &self.path, u64::from(entry.header_offset), 30)?;
        if u32_at(&header, 0) != Some(0x0403_4b50) {
            return Err(ZipError::Invalid("bad local header"));
        }
        let name_len = u16_at(&header, 26).unwrap_or(0);
        let extra_len = u16_at(&header, 28).unwrap_or(0);
        Ok(u64::from(entry.header_offset) + 30 + u64::from(name_len) + u64::from(extra_len))
    }

    /// Streams the uncompressed content of `entry` to `sink` in chunks.
    /// The sink returns `false` to stop early.
    ///
    /// Inflating needs about 10 KiB of decoder state plus a window that
    /// holds the entry, at most 32 KiB (the deflate history); small entries
    /// such as package documents therefore need very little memory.
    pub fn stream(
        &self,
        storage: &dyn Storage,
        entry: &Entry,
        sink: &mut dyn FnMut(&[u8]) -> bool,
    ) -> Result<(), ZipError> {
        let mut offset = self.data_offset(storage, entry)?;
        let mut remaining = u64::from(entry.compressed_size);
        let mut input = zeroed(CHUNK.min(remaining as usize).max(1))?;
        let fill = |offset: &mut u64, remaining: &mut u64, input: &mut [u8]| {
            let n = (*remaining as usize).min(input.len());
            let read = storage.read_at(&self.path, *offset, &mut input[..n])?;
            if read == 0 {
                return Err(ZipError::Invalid("unexpected end of file"));
            }
            *offset += read as u64;
            *remaining -= read as u64;
            Ok(read)
        };
        match entry.method {
            Method::Stored => {
                while remaining > 0 {
                    let read = fill(&mut offset, &mut remaining, &mut input)?;
                    if !sink(&input[..read]) {
                        return Ok(());
                    }
                }
                Ok(())
            }
            Method::Deflate => {
                let mut decoder: Vec<DecompressorOxide> = Vec::new();
                decoder
                    .try_reserve_exact(1)
                    .map_err(|_| ZipError::OutOfMemory)?;
                decoder.push(DecompressorOxide::new());
                let decoder = &mut decoder[0];
                // A power of two (required for a wrapping window) that holds
                // the whole entry, or the full deflate history.
                let window_len = (entry.size as usize).next_power_of_two().clamp(64, WINDOW);
                let mut window = zeroed(window_len)?;
                let (mut start, mut end, mut position) = (0usize, 0usize, 0usize);
                loop {
                    if start == end && remaining > 0 {
                        end = fill(&mut offset, &mut remaining, &mut input)?;
                        start = 0;
                    }
                    let flags = if remaining > 0 {
                        inflate_flags::TINFL_FLAG_HAS_MORE_INPUT
                    } else {
                        0
                    };
                    let (status, consumed, written) =
                        decompress(decoder, &input[start..end], &mut window, position, flags);
                    start += consumed;
                    if written > 0 && !sink(&window[position..position + written]) {
                        return Ok(());
                    }
                    position = (position + written) & (window_len - 1);
                    match status {
                        TINFLStatus::Done => return Ok(()),
                        TINFLStatus::HasMoreOutput => {}
                        TINFLStatus::NeedsMoreInput if remaining > 0 || start < end => {}
                        TINFLStatus::NeedsMoreInput => {
                            return Err(ZipError::Invalid("truncated deflate data"));
                        }
                        _ => return Err(ZipError::Invalid("corrupt deflate data")),
                    }
                }
            }
        }
    }

    /// Reads a whole entry, refusing entries larger than `limit` bytes.
    pub fn read(
        &self,
        storage: &dyn Storage,
        entry: &Entry,
        limit: usize,
    ) -> Result<Vec<u8>, ZipError> {
        if entry.size as usize > limit {
            return Err(ZipError::TooLarge);
        }
        let mut data = Vec::new();
        data.try_reserve_exact(entry.size as usize)
            .map_err(|_| ZipError::OutOfMemory)?;
        let mut too_large = false;
        self.stream(storage, entry, &mut |chunk| {
            if data.len() + chunk.len() > limit {
                too_large = true;
                return false;
            }
            data.extend_from_slice(chunk);
            true
        })?;
        if too_large {
            return Err(ZipError::TooLarge);
        }
        Ok(data)
    }
}

/// Builds ZIP archives in memory, for tests of ZIP-based formats.
#[cfg(any(test, feature = "testing"))]
pub mod testing {
    use alloc::string::String;
    use alloc::vec::Vec;

    /// Collects files and writes a ZIP archive. `deflate` compresses with
    /// the given level; otherwise entries are stored.
    #[derive(Debug, Default)]
    pub struct ZipBuilder {
        files: Vec<(String, Vec<u8>, bool)>,
    }

    impl ZipBuilder {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn stored(mut self, name: &str, data: impl Into<Vec<u8>>) -> Self {
            self.files.push((String::from(name), data.into(), false));
            self
        }

        pub fn deflated(mut self, name: &str, data: impl Into<Vec<u8>>) -> Self {
            self.files.push((String::from(name), data.into(), true));
            self
        }

        pub fn build(&self) -> Vec<u8> {
            let mut out = Vec::new();
            let mut central = Vec::new();
            for (name, data, deflate) in &self.files {
                let payload = if *deflate {
                    miniz_oxide::deflate::compress_to_vec(data, 6)
                } else {
                    data.clone()
                };
                let method: u16 = if *deflate { 8 } else { 0 };
                let offset = out.len() as u32;
                let mut header = Vec::new();
                header.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
                header.extend_from_slice(&20u16.to_le_bytes());
                header.extend_from_slice(&0u16.to_le_bytes());
                header.extend_from_slice(&method.to_le_bytes());
                header.extend_from_slice(&[0; 8]); // time, date, crc (unchecked)
                header.extend_from_slice(&(payload.len() as u32).to_le_bytes());
                header.extend_from_slice(&(data.len() as u32).to_le_bytes());
                header.extend_from_slice(&(name.len() as u16).to_le_bytes());
                header.extend_from_slice(&0u16.to_le_bytes());
                out.extend_from_slice(&header);
                out.extend_from_slice(name.as_bytes());
                out.extend_from_slice(&payload);

                central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
                central.extend_from_slice(&20u16.to_le_bytes());
                central.extend_from_slice(&20u16.to_le_bytes());
                central.extend_from_slice(&0u16.to_le_bytes());
                central.extend_from_slice(&method.to_le_bytes());
                central.extend_from_slice(&[0; 8]);
                central.extend_from_slice(&(payload.len() as u32).to_le_bytes());
                central.extend_from_slice(&(data.len() as u32).to_le_bytes());
                central.extend_from_slice(&(name.len() as u16).to_le_bytes());
                central.extend_from_slice(&[0; 12]); // extra, comment, disk, attrs
                central.extend_from_slice(&offset.to_le_bytes());
                central.extend_from_slice(name.as_bytes());
            }
            let cd_offset = out.len() as u32;
            out.extend_from_slice(&central);
            out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
            out.extend_from_slice(&[0; 4]);
            let count = self.files.len() as u16;
            out.extend_from_slice(&count.to_le_bytes());
            out.extend_from_slice(&count.to_le_bytes());
            out.extend_from_slice(&(central.len() as u32).to_le_bytes());
            out.extend_from_slice(&cd_offset.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::ZipBuilder;
    use super::*;
    use alloc::borrow::ToOwned;
    use alloc::string::ToString;
    use quireink_platform::DirEntry;

    /// A single-file storage for tests.
    struct OneFile(Vec<u8>);

    impl Storage for OneFile {
        fn read_at(&self, _: &str, offset: u64, buf: &mut [u8]) -> Result<usize, StorageError> {
            let start = (offset as usize).min(self.0.len());
            let n = buf.len().min(self.0.len() - start);
            buf[..n].copy_from_slice(&self.0[start..start + n]);
            Ok(n)
        }
        fn size(&self, _: &str) -> Result<u64, StorageError> {
            Ok(self.0.len() as u64)
        }
        fn write(&self, _: &str, _: &[u8]) -> Result<(), StorageError> {
            Err(StorageError::Unsupported)
        }
        fn append(&self, _: &str, _: &[u8]) -> Result<(), StorageError> {
            Err(StorageError::Unsupported)
        }
        fn list(&self, _: &str, _: &mut dyn FnMut(DirEntry<'_>)) -> Result<(), StorageError> {
            Ok(())
        }
        fn remove(&self, _: &str) -> Result<(), StorageError> {
            Ok(())
        }
    }

    #[test]
    fn reads_stored_and_deflated_entries() {
        let big: String = (0..20_000)
            .map(|i| (b'a' + (i % 26) as u8) as char)
            .collect();
        let zip = ZipBuilder::new()
            .stored("mimetype", "application/epub+zip")
            .deflated("OEBPS/big.txt", big.clone())
            .build();
        let storage = OneFile(zip);
        let archive = ZipArchive::open(&storage, "x.zip").unwrap();
        assert_eq!(archive.len(), 2);
        assert_eq!(archive.entry(&storage, "missing").unwrap(), None);

        let mimetype = archive.entry(&storage, "mimetype").unwrap().unwrap();
        assert_eq!(
            archive.read(&storage, &mimetype, 100).unwrap(),
            b"application/epub+zip"
        );
        let entry = archive.entry(&storage, "OEBPS/big.txt").unwrap().unwrap();
        let entry = &entry;
        assert_eq!(entry.method, Method::Deflate);
        assert!(entry.compressed_size < entry.size);
        let data = archive.read(&storage, entry, 1 << 20).unwrap();
        assert_eq!(data, big.as_bytes());
        assert_eq!(archive.read(&storage, entry, 100), Err(ZipError::TooLarge));

        // Streaming can stop early.
        let mut seen = 0;
        archive
            .stream(&storage, entry, &mut |chunk| {
                seen += chunk.len();
                false
            })
            .unwrap();
        assert!(seen > 0 && seen <= WINDOW);
    }

    #[test]
    fn rejects_non_zip_data() {
        let storage = OneFile(b"this is not a zip archive at all, sorry".to_vec());
        assert!(matches!(
            ZipArchive::open(&storage, "x"),
            Err(ZipError::Invalid(_))
        ));
        let _ = "x".to_owned().to_string();
    }
}
