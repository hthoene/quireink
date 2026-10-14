//! Reading definitions from `.dict` or `.dict.dz` files.
//!
//! A `.dict.dz` is a dictzip file: gzip whose deflate stream is flushed
//! every `chunk_len` bytes, with the compressed size of each chunk in the
//! gzip header's `RA` field. One chunk can therefore be inflated on its
//! own; this is done as a stream through a 32 KiB window, keeping only the
//! bytes asked for.

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use miniz_oxide::inflate::TINFLStatus;
use miniz_oxide::inflate::core::{DecompressorOxide, decompress, inflate_flags};
use quireink_platform::Storage;

use crate::{Error, malformed};

/// The deflate history: the largest window needed.
const WINDOW: usize = 32 * 1024;
/// Compressed bytes read at a time.
const INPUT: usize = 4096;

#[derive(Debug)]
pub(crate) enum DictFile {
    Plain(String),
    Zip(Dictzip),
}

#[derive(Debug)]
pub(crate) struct Dictzip {
    path: String,
    chunk_len: u64,
    /// Where the deflate data starts.
    data: u64,
    /// Start of each chunk relative to `data`, and the end of the last.
    chunks: Vec<u64>,
}

impl DictFile {
    pub fn open(storage: &dyn Storage, base: &str) -> Result<DictFile, Error> {
        let plain = format!("{base}.dict");
        if storage.exists(&plain) {
            return Ok(DictFile::Plain(plain));
        }
        let zip = format!("{base}.dict.dz");
        if !storage.exists(&zip) {
            return Err(malformed("no .dict or .dict.dz file"));
        }
        Ok(DictFile::Zip(Dictzip::open(storage, zip)?))
    }

    /// `size` bytes of the uncompressed definitions from `offset`.
    pub fn read(&self, storage: &dyn Storage, offset: u64, size: usize) -> Result<Vec<u8>, Error> {
        match self {
            DictFile::Plain(path) => {
                let mut data = vec![0u8; size];
                let mut done = 0;
                while done < size {
                    let n = storage.read_at(path, offset + done as u64, &mut data[done..])?;
                    if n == 0 {
                        break;
                    }
                    done += n;
                }
                data.truncate(done);
                Ok(data)
            }
            DictFile::Zip(zip) => zip.read(storage, offset, size),
        }
    }
}

fn u16_at(data: &[u8], at: usize) -> Option<u64> {
    Some(u64::from(u16::from_le_bytes([
        *data.get(at)?,
        *data.get(at + 1)?,
    ])))
}

impl Dictzip {
    fn open(storage: &dyn Storage, path: String) -> Result<Dictzip, Error> {
        let mut head = [0u8; 12];
        if storage.read_at(&path, 0, &mut head)? != 12 || head[..3] != [0x1f, 0x8b, 8] {
            return Err(malformed("not a gzip file"));
        }
        let flags = head[3];
        if flags & 4 == 0 {
            return Err(malformed("gzip without dictzip chunks"));
        }
        let extra_len = u16_at(&head, 10).unwrap_or(0) as usize;
        let mut extra = vec![0u8; extra_len];
        if storage.read_at(&path, 12, &mut extra)? != extra_len {
            return Err(malformed("gzip header cut short"));
        }
        // The `RA` subfield: version, chunk length, chunk count, sizes.
        let mut at = 0;
        let mut table = None;
        while at + 4 <= extra.len() {
            let len = u16_at(&extra, at + 2).unwrap_or(0) as usize;
            if &extra[at..at + 2] == b"RA" {
                table = extra.get(at + 4..at + 4 + len);
                break;
            }
            at += 4 + len;
        }
        let table = table.ok_or_else(|| malformed("gzip without dictzip chunks"))?;
        let chunk_len = u16_at(table, 2).ok_or_else(|| malformed("dictzip table"))?;
        let count = u16_at(table, 4).ok_or_else(|| malformed("dictzip table"))? as usize;
        let mut chunks = Vec::with_capacity(count + 1);
        let mut start = 0;
        chunks.push(0);
        for i in 0..count {
            start += u16_at(table, 6 + 2 * i).ok_or_else(|| malformed("dictzip table"))?;
            chunks.push(start);
        }
        // The deflate data follows the optional name, comment and CRC.
        let mut data = 12 + extra_len as u64;
        for flag in [8u8, 16] {
            if flags & flag != 0 {
                data = skip_string(storage, &path, data)?;
            }
        }
        if flags & 2 != 0 {
            data += 2;
        }
        if chunk_len == 0 {
            return Err(malformed("dictzip table"));
        }
        Ok(Dictzip {
            path,
            chunk_len,
            data,
            chunks,
        })
    }

    fn read(&self, storage: &dyn Storage, offset: u64, size: usize) -> Result<Vec<u8>, Error> {
        let end = offset + size as u64;
        let mut out = Vec::new();
        out.try_reserve_exact(size)
            .map_err(|_| Error::OutOfMemory)?;
        if size == 0 {
            return Ok(out);
        }
        let first = (offset / self.chunk_len) as usize;
        let last = ((end - 1) / self.chunk_len) as usize;
        for chunk in first..=last.min(self.chunks.len().saturating_sub(2)) {
            let base = chunk as u64 * self.chunk_len;
            self.inflate(storage, chunk, &mut |at, bytes| {
                // `bytes` are the chunk's uncompressed bytes from `at`.
                let from = base + at;
                let to = from + bytes.len() as u64;
                let (lo, hi) = (from.max(offset), to.min(end));
                if lo < hi {
                    out.extend_from_slice(&bytes[(lo - from) as usize..(hi - from) as usize]);
                }
                to < end
            })?;
        }
        Ok(out)
    }

    /// Inflates chunk `chunk`, passing its bytes to `sink` with their
    /// position in the chunk; the sink returns `false` to stop.
    fn inflate(
        &self,
        storage: &dyn Storage,
        chunk: usize,
        sink: &mut dyn FnMut(u64, &[u8]) -> bool,
    ) -> Result<(), Error> {
        let mut offset = self.data + self.chunks[chunk];
        let mut remaining = self.chunks[chunk + 1] - self.chunks[chunk];
        let mut decoder = alloc::boxed::Box::new(DecompressorOxide::new());
        let mut window = vec![0u8; WINDOW];
        let mut input = vec![0u8; INPUT];
        let (mut start, mut stop, mut position, mut produced) = (0usize, 0usize, 0usize, 0u64);
        loop {
            if start == stop && remaining > 0 {
                let n = (remaining as usize).min(INPUT);
                let read = storage.read_at(&self.path, offset, &mut input[..n])?;
                if read == 0 {
                    return Err(malformed("dictzip data cut short"));
                }
                offset += read as u64;
                remaining -= read as u64;
                start = 0;
                stop = read;
            }
            // Chunks end with a flush, not with the end of the stream, so
            // the decoder is always told that more may follow.
            let flags = inflate_flags::TINFL_FLAG_HAS_MORE_INPUT;
            let (status, consumed, written) = decompress(
                &mut decoder,
                &input[start..stop],
                &mut window,
                position,
                flags,
            );
            start += consumed;
            if written > 0 {
                if !sink(produced, &window[position..position + written]) {
                    return Ok(());
                }
                produced += written as u64;
            }
            position = (position + written) & (WINDOW - 1);
            match status {
                TINFLStatus::Done => return Ok(()),
                TINFLStatus::HasMoreOutput => {}
                // The end of the chunk.
                TINFLStatus::NeedsMoreInput if remaining == 0 && start == stop => return Ok(()),
                TINFLStatus::NeedsMoreInput => {}
                _ => return Err(malformed("corrupt dictzip data")),
            }
        }
    }
}

/// The offset after the NUL-terminated string at `at`.
fn skip_string(storage: &dyn Storage, path: &str, mut at: u64) -> Result<u64, Error> {
    let mut buf = [0u8; 256];
    loop {
        let n = storage.read_at(path, at, &mut buf)?;
        if n == 0 {
            return Err(malformed("gzip header cut short"));
        }
        if let Some(zero) = buf[..n].iter().position(|&b| b == 0) {
            return Ok(at + zero as u64 + 1);
        }
        at += n as u64;
    }
}
