//! What the file manager does to files: list, copy, move, rename, delete.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use quireink_sdk::{Storage, StorageError};

/// An entry of a folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Entry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
}

/// `dir/name`, or `name` at the top.
pub(crate) fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

/// The folder a path is in.
pub(crate) fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

/// The last part of a path.
pub(crate) fn name(path: &str) -> &str {
    path.rsplit_once('/').map_or(path, |(_, name)| name)
}

/// A folder's entries: folders first, then by name (case ignored);
/// names starting with `.` only with `hidden`.
pub(crate) fn list(
    storage: &dyn Storage,
    dir: &str,
    hidden: bool,
) -> Result<Vec<Entry>, StorageError> {
    let mut entries = Vec::new();
    storage.list(dir, &mut |e| {
        if hidden || !e.name.starts_with('.') {
            entries.push(Entry {
                name: e.name.to_string(),
                is_dir: e.is_dir,
                size: e.size,
            });
        }
    })?;
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(entries)
}

/// Whether `path` is a folder.
pub(crate) fn is_dir(storage: &dyn Storage, path: &str) -> bool {
    !storage.exists(path) && storage.list(path, &mut |_| {}).is_ok()
}

/// A free path for `name` in `dir`: `name`, else `name (2)`, `name (3)`, …
/// (before the extension).
pub(crate) fn free_path(storage: &dyn Storage, dir: &str, name: &str) -> String {
    let taken = |path: &str| storage.exists(path) || is_dir(storage, path);
    let first = join(dir, name);
    if !taken(&first) {
        return first;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem, format!(".{ext}")),
        _ => (name, String::new()),
    };
    (2..)
        .map(|n| join(dir, &format!("{stem} ({n}){ext}")))
        .find(|p| !taken(p))
        .unwrap_or(first)
}

/// Copies a file or a folder with everything in it.
pub(crate) fn copy(storage: &dyn Storage, from: &str, to: &str) -> Result<(), StorageError> {
    if is_dir(storage, from) {
        if to.starts_with(&format!("{from}/")) {
            // Into itself: it would never end.
            return Err(StorageError::InvalidPath);
        }
        storage.create_dir(to)?;
        for entry in list(storage, from, true)? {
            copy(storage, &join(from, &entry.name), &join(to, &entry.name))?;
        }
        return Ok(());
    }
    storage.write(to, &[])?;
    let mut chunk = vec![0u8; 16 * 1024];
    let mut at = 0u64;
    loop {
        let n = storage.read_at(from, at, &mut chunk)?;
        if n == 0 {
            return Ok(());
        }
        storage.append(to, &chunk[..n])?;
        at += n as u64;
    }
}

/// Moves a file or folder; between storages that cannot rename across
/// (e.g. a memory card and the device), by copying and deleting.
pub(crate) fn move_to(storage: &dyn Storage, from: &str, to: &str) -> Result<(), StorageError> {
    if to.starts_with(&format!("{from}/")) {
        return Err(StorageError::InvalidPath);
    }
    match storage.rename(from, to) {
        Err(StorageError::Unsupported) => {
            copy(storage, from, to)?;
            delete(storage, from)
        }
        result => result,
    }
}

/// Deletes a file or a folder with everything in it.
pub(crate) fn delete(storage: &dyn Storage, path: &str) -> Result<(), StorageError> {
    if is_dir(storage, path) {
        storage.remove_dir_all(path)
    } else {
        storage.remove(path)
    }
}

/// A size as people read it: `820 B`, `14 KB`, `2,3 MB` (with the
/// language's decimal mark).
pub(crate) fn size(bytes: u64, decimal: &str) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 || value >= 10.0 {
        format!("{} {}", (value + 0.5) as u64, UNITS[unit])
    } else {
        let tenths = (value * 10.0 + 0.5) as u64;
        format!("{}{decimal}{} {}", tenths / 10, tenths % 10, UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quireink_sdk::MemoryStorage;

    fn storage() -> MemoryStorage {
        MemoryStorage::new()
            .with("books/b.epub", b"bb".to_vec())
            .with("books/A.epub", b"aaa".to_vec())
            .with("books/sub/c.txt", b"c".to_vec())
            .with("books/.hidden", b"h".to_vec())
    }

    #[test]
    fn lists_folders_first() {
        let s = storage();
        let names: Vec<String> = list(&s, "books", false)
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, ["sub", "A.epub", "b.epub"]);
        assert_eq!(list(&s, "books", true).unwrap().len(), 4);
        assert!(is_dir(&s, "books/sub"));
        assert!(!is_dir(&s, "books/A.epub"));
        assert_eq!(
            (parent("books/sub/c.txt"), name("books/sub/c.txt")),
            ("books/sub", "c.txt")
        );
        assert_eq!(parent("top"), "");
    }

    #[test]
    fn copies_moves_and_deletes() {
        let s = storage();
        copy(&s, "books", "copy").unwrap();
        assert_eq!(s.get("copy/sub/c.txt").as_deref(), Some(&b"c"[..]));
        assert_eq!(
            copy(&s, "books", "books/sub/x"),
            Err(StorageError::InvalidPath)
        );
        move_to(&s, "copy/A.epub", "A.epub").unwrap();
        assert!(s.get("copy/A.epub").is_none() && s.get("A.epub").is_some());
        assert_eq!(free_path(&s, "", "A.epub"), "A (2).epub");
        assert_eq!(free_path(&s, "", "new"), "new");
        delete(&s, "copy").unwrap();
        assert!(s.get("copy/sub/c.txt").is_none());
        assert_eq!(size(820, ","), "820 B");
        assert_eq!(size(2_400_000, ","), "2,3 MB");
        assert_eq!(size(14_000, "."), "14 KB");
    }
}
