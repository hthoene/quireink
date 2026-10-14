//! Sorting files of fixed-size records bytewise with little memory: runs
//! that fit in memory are sorted and written to files, then merged.
//! Records are big-endian, so bytewise order is numeric order.

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use quireink_sdk::{Storage, StorageError};

/// Bytes sorted in memory at once.
const RUN: usize = 32 * 1024;
/// Bytes read ahead from each run while merging.
const AHEAD: usize = 1024;

/// Sorts the records of `size` bytes in `input` into `output` (which may
/// be `input`); `temp` names the files of the runs.
pub(crate) fn sort(
    storage: &dyn Storage,
    input: &str,
    output: &str,
    size: usize,
    temp: &str,
) -> Result<(), StorageError> {
    let per_run = (RUN / size).max(1) * size;
    let mut runs: Vec<String> = Vec::new();
    let mut buffer = vec![0u8; per_run];
    let mut at = 0u64;
    loop {
        let mut filled = 0;
        while filled < per_run {
            let n = match storage.read_at(input, at + filled as u64, &mut buffer[filled..]) {
                Ok(n) => n,
                Err(StorageError::NotFound) if at == 0 && filled == 0 => 0,
                Err(e) => return Err(e),
            };
            if n == 0 {
                break;
            }
            filled += n;
        }
        let whole = filled - filled % size;
        if whole == 0 {
            break;
        }
        let mut records: Vec<&[u8]> = buffer[..whole].chunks_exact(size).collect();
        records.sort_unstable();
        let sorted: Vec<u8> = records.concat();
        let run = format!("{temp}.{}", runs.len());
        storage.write(&run, &sorted)?;
        runs.push(run);
        at += whole as u64;
        if filled < per_run {
            break;
        }
    }
    let result = merge(storage, &runs, output, size);
    for run in &runs {
        let _ = storage.remove(run);
    }
    result
}

/// A run being read.
struct Reader<'a> {
    path: &'a str,
    at: u64,
    buffer: Vec<u8>,
    start: usize,
}

impl Reader<'_> {
    fn head(&mut self, storage: &dyn Storage, size: usize) -> Result<Option<&[u8]>, StorageError> {
        if self.start + size > self.buffer.len() {
            let mut chunk = vec![0u8; AHEAD.max(size)];
            let n = storage.read_at(self.path, self.at, &mut chunk)?;
            let whole = n - n % size;
            self.at += whole as u64;
            chunk.truncate(whole);
            self.buffer = chunk;
            self.start = 0;
            if whole == 0 {
                return Ok(None);
            }
        }
        Ok(Some(&self.buffer[self.start..self.start + size]))
    }
}

fn merge(
    storage: &dyn Storage,
    runs: &[String],
    output: &str,
    size: usize,
) -> Result<(), StorageError> {
    storage.write(output, &[])?;
    let mut readers: Vec<Reader<'_>> = runs
        .iter()
        .map(|path| Reader {
            path,
            at: 0,
            buffer: Vec::new(),
            start: 0,
        })
        .collect();
    let mut out: Vec<u8> = Vec::with_capacity(AHEAD * 4);
    loop {
        // The run with the smallest head (few runs: a scan is fine).
        let mut best: Option<(usize, Vec<u8>)> = None;
        for (i, reader) in readers.iter_mut().enumerate() {
            if let Some(head) = reader.head(storage, size)?
                && best.as_ref().is_none_or(|(_, b)| head < b.as_slice())
            {
                best = Some((i, head.to_vec()));
            }
        }
        let Some((i, record)) = best else {
            break;
        };
        readers[i].start += size;
        out.extend_from_slice(&record);
        if out.len() >= AHEAD * 4 {
            storage.append(output, &out)?;
            out.clear();
        }
    }
    storage.append(output, &out)
}

/// Calls `visit` with every record of `path`, in order.
pub(crate) fn each(
    storage: &dyn Storage,
    path: &str,
    size: usize,
    mut visit: impl FnMut(&[u8]),
) -> Result<(), StorageError> {
    let mut chunk = vec![0u8; (AHEAD * 4 / size).max(1) * size];
    let mut at = 0u64;
    loop {
        let n = match storage.read_at(path, at, &mut chunk) {
            Ok(n) => n,
            Err(StorageError::NotFound) if at == 0 => 0,
            Err(e) => return Err(e),
        };
        let whole = n - n % size;
        for record in chunk[..whole].chunks_exact(size) {
            visit(record);
        }
        if whole == 0 || n < chunk.len() {
            return Ok(());
        }
        at += whole as u64;
    }
}

/// The records of a file, read a chunk at a time. A read error ends them
/// early and is kept in [`Records::failed`].
pub(crate) struct Records<'s> {
    storage: &'s dyn Storage,
    path: String,
    size: usize,
    at: u64,
    buffer: Vec<u8>,
    start: usize,
    pub failed: Option<StorageError>,
}

impl<'s> Records<'s> {
    pub(crate) fn new(storage: &'s dyn Storage, path: &str, size: usize) -> Self {
        Self {
            storage,
            path: String::from(path),
            size,
            at: 0,
            buffer: Vec::new(),
            start: 0,
            failed: None,
        }
    }
}

impl Iterator for Records<'_> {
    type Item = Vec<u8>;

    fn next(&mut self) -> Option<Vec<u8>> {
        if self.start + self.size > self.buffer.len() {
            let mut chunk = vec![0u8; (AHEAD * 4 / self.size).max(1) * self.size];
            let n = match self.storage.read_at(&self.path, self.at, &mut chunk) {
                Ok(n) => n,
                Err(StorageError::NotFound) if self.at == 0 => 0,
                Err(e) => {
                    self.failed = Some(e);
                    0
                }
            };
            let whole = n - n % self.size;
            chunk.truncate(whole);
            self.at += whole as u64;
            self.buffer = chunk;
            self.start = 0;
            if whole == 0 {
                return None;
            }
        }
        let record = self.buffer[self.start..self.start + self.size].to_vec();
        self.start += self.size;
        Some(record)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quireink_sdk::MemoryStorage;

    #[test]
    fn sorts_large_files_in_runs() {
        let storage = MemoryStorage::new();
        // 10 000 records of 8 bytes: three runs.
        let mut data = Vec::new();
        let mut x: u32 = 12_345;
        for i in 0..10_000u32 {
            x = x.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            data.extend_from_slice(&(x % 5000).to_be_bytes());
            data.extend_from_slice(&i.to_be_bytes());
        }
        storage.write("in", &data).unwrap();
        sort(&storage, "in", "in", 8, "tmp").unwrap();
        let mut records = Vec::new();
        each(&storage, "in", 8, |r| records.push(r.to_vec())).unwrap();
        assert_eq!(records.len(), 10_000);
        assert!(records.windows(2).all(|w| w[0] <= w[1]));
        assert!(storage.size("tmp.0").is_err(), "runs are removed");
        let streamed: Vec<Vec<u8>> = Records::new(&storage, "in", 8).collect();
        assert_eq!(streamed, records);
        // Nothing to sort.
        sort(&storage, "none", "out", 8, "tmp").unwrap();
        assert_eq!(storage.size("out").unwrap(), 0);
    }
}
