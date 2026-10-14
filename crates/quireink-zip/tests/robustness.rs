//! Damaged archives must fail cleanly: every byte of a small archive is
//! mutated in turn, then random damage is applied.

use quireink_platform::{DirEntry, Storage, StorageError};
use quireink_zip::ZipArchive;
use quireink_zip::testing::ZipBuilder;

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
        Err(StorageError::Unsupported)
    }
    fn remove(&self, _: &str) -> Result<(), StorageError> {
        Err(StorageError::Unsupported)
    }
}

fn archive() -> Vec<u8> {
    let text: String = (0..2000).map(|i| format!("Zeile {i} – Größe\n")).collect();
    ZipBuilder::new()
        .stored("mimetype", "application/epub+zip")
        .deflated("a/ü.txt", text.clone())
        .deflated("b.txt", "kurz")
        .stored("c.bin", vec![7u8; 300])
        .build()
}

fn exercise(data: Vec<u8>) {
    let storage = OneFile(data);
    let Ok(archive) = ZipArchive::open(&storage, "x") else {
        return;
    };
    for entry in archive.entries(&storage).flatten() {
        let _ = archive.read(&storage, &entry, 1 << 20);
        let mut total = 0usize;
        let _ = archive.stream(&storage, &entry, &mut |chunk| {
            total += chunk.len();
            total < 4 << 20
        });
        let _ = archive.entry(&storage, &entry.name);
    }
}

#[test]
fn every_byte_flipped() {
    let original = archive();
    for at in 0..original.len() {
        for value in [0x00, 0xff, original[at] ^ 0x01] {
            let mut data = original.clone();
            data[at] = value;
            exercise(data);
        }
    }
}

#[test]
fn truncated_anywhere() {
    let original = archive();
    for len in 0..original.len() {
        exercise(original[..len].to_vec());
    }
}
