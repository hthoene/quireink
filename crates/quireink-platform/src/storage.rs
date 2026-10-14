//! Access to the device's storage (SD card, flash file system, disk).
//!
//! Paths are virtual, `/`-separated and relative to the storage root, e.g.
//! `books/alice.txt` or `.quireink/settings.ini`; `""` denotes the root in
//! [`Storage::list`]. Shells map paths to the real file system and must
//! reject paths that escape the root (see [`is_valid_path`]).
//!
//! Local storage is accessed synchronously: reads from an SD card or flash
//! are fast enough on every target, and it keeps the kernel simple. Slow,
//! remote I/O (network) is modelled as effects instead.

use core::fmt;

/// Why a storage operation failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum StorageError {
    NotFound,
    /// The path is malformed or escapes the storage root.
    InvalidPath,
    /// The storage is read-only or the operation is not supported.
    Unsupported,
    /// The medium is full.
    Full,
    /// Any other I/O failure.
    Io,
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            StorageError::NotFound => "not found",
            StorageError::InvalidPath => "invalid path",
            StorageError::Unsupported => "not supported",
            StorageError::Full => "storage full",
            StorageError::Io => "input/output error",
        })
    }
}

impl core::error::Error for StorageError {}

/// One entry of a directory listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirEntry<'a> {
    pub name: &'a str,
    pub is_dir: bool,
    /// Size in bytes (0 for directories).
    pub size: u64,
}

/// Storage provided by a shell.
///
/// All methods take `&self` so the storage can be shared with plugins;
/// implementations use interior mutability where needed (e.g. to keep the
/// last file open).
pub trait Storage {
    /// Reads up to `buf.len()` bytes at `offset`; returns the number read
    /// (0 at the end of the file).
    fn read_at(&self, path: &str, offset: u64, buf: &mut [u8]) -> Result<usize, StorageError>;

    /// Size of a file in bytes.
    fn size(&self, path: &str) -> Result<u64, StorageError>;

    /// Replaces the file's content, creating it and missing parent
    /// directories. Implementations write atomically where the medium
    /// allows it, so power loss never leaves a half-written file.
    fn write(&self, path: &str, data: &[u8]) -> Result<(), StorageError>;

    /// Appends to a file, creating it and missing parent directories.
    fn append(&self, path: &str, data: &[u8]) -> Result<(), StorageError>;

    /// Calls `visit` for each entry of a directory, in no particular order.
    fn list(&self, dir: &str, visit: &mut dyn FnMut(DirEntry<'_>)) -> Result<(), StorageError>;

    /// Deletes a file; deleting a missing file is not an error.
    fn remove(&self, path: &str) -> Result<(), StorageError>;

    /// Whether a file exists.
    fn exists(&self, path: &str) -> bool {
        self.size(path).is_ok()
    }
}

/// Storage for devices without any: everything is empty and read-only.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoStorage;

impl Storage for NoStorage {
    fn read_at(&self, _: &str, _: u64, _: &mut [u8]) -> Result<usize, StorageError> {
        Err(StorageError::NotFound)
    }

    fn size(&self, _: &str) -> Result<u64, StorageError> {
        Err(StorageError::NotFound)
    }

    fn write(&self, _: &str, _: &[u8]) -> Result<(), StorageError> {
        Err(StorageError::Unsupported)
    }

    fn append(&self, _: &str, _: &[u8]) -> Result<(), StorageError> {
        Err(StorageError::Unsupported)
    }

    fn list(&self, _: &str, _: &mut dyn FnMut(DirEntry<'_>)) -> Result<(), StorageError> {
        Err(StorageError::NotFound)
    }

    fn remove(&self, _: &str) -> Result<(), StorageError> {
        Ok(())
    }
}

/// Whether `path` is a valid virtual file path: relative, `/`-separated,
/// without empty, `.` or `..` segments and without backslashes, colons or
/// NUL bytes.
pub fn is_valid_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains(['\\', '\0', ':'])
        && path
            .split('/')
            .all(|segment| !segment.is_empty() && segment != "." && segment != "..")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_paths() {
        for ok in ["a", "books/alice.txt", ".quireink/settings.ini"] {
            assert!(is_valid_path(ok), "{ok}");
        }
        for bad in [
            "", "/abs", "a//b", "a/../b", "..", "a/./b", "c:\\x", "a\\b", "x/",
        ] {
            assert!(!is_valid_path(bad), "{bad}");
        }
    }
}
