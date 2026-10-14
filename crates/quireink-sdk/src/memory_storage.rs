use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;

use quireink_platform::storage::is_valid_path;
use quireink_platform::{DirEntry, Storage, StorageError};

/// A storage kept in memory — for tests of plugins and for devices or
/// demos without a file system.
#[derive(Debug, Default)]
pub struct MemoryStorage {
    files: RefCell<BTreeMap<String, Vec<u8>>>,
}

impl MemoryStorage {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a file (builder style).
    pub fn with(self, path: &str, data: impl Into<Vec<u8>>) -> Self {
        self.files
            .borrow_mut()
            .insert(String::from(path), data.into());
        self
    }

    /// The content of a file, for assertions.
    pub fn get(&self, path: &str) -> Option<Vec<u8>> {
        self.files.borrow().get(path).cloned()
    }

    fn check(path: &str) -> Result<(), StorageError> {
        if is_valid_path(path) {
            Ok(())
        } else {
            Err(StorageError::InvalidPath)
        }
    }
}

impl Storage for MemoryStorage {
    fn read_at(&self, path: &str, offset: u64, buf: &mut [u8]) -> Result<usize, StorageError> {
        Self::check(path)?;
        let files = self.files.borrow();
        let data = files.get(path).ok_or(StorageError::NotFound)?;
        let start = usize::try_from(offset)
            .unwrap_or(usize::MAX)
            .min(data.len());
        let n = buf.len().min(data.len() - start);
        buf[..n].copy_from_slice(&data[start..start + n]);
        Ok(n)
    }

    fn size(&self, path: &str) -> Result<u64, StorageError> {
        Self::check(path)?;
        self.files
            .borrow()
            .get(path)
            .map(|data| data.len() as u64)
            .ok_or(StorageError::NotFound)
    }

    fn write(&self, path: &str, data: &[u8]) -> Result<(), StorageError> {
        Self::check(path)?;
        self.files
            .borrow_mut()
            .insert(String::from(path), data.to_vec());
        Ok(())
    }

    fn append(&self, path: &str, data: &[u8]) -> Result<(), StorageError> {
        Self::check(path)?;
        self.files
            .borrow_mut()
            .entry(String::from(path))
            .or_default()
            .extend_from_slice(data);
        Ok(())
    }

    fn list(&self, dir: &str, visit: &mut dyn FnMut(DirEntry<'_>)) -> Result<(), StorageError> {
        if !dir.is_empty() {
            Self::check(dir)?;
        }
        let prefix = if dir.is_empty() {
            String::new()
        } else {
            alloc::format!("{dir}/")
        };
        let files = self.files.borrow();
        let mut dirs = BTreeSet::new();
        let mut found = dir.is_empty();
        for (path, data) in files.iter() {
            let Some(rest) = path.strip_prefix(prefix.as_str()) else {
                continue;
            };
            found = true;
            match rest.split_once('/') {
                Some((sub, _)) => {
                    dirs.insert(String::from(sub));
                }
                None => visit(DirEntry {
                    name: rest,
                    is_dir: false,
                    size: data.len() as u64,
                }),
            }
        }
        for name in &dirs {
            visit(DirEntry {
                name,
                is_dir: true,
                size: 0,
            });
        }
        if found {
            Ok(())
        } else {
            Err(StorageError::NotFound)
        }
    }

    fn remove(&self, path: &str) -> Result<(), StorageError> {
        Self::check(path)?;
        self.files.borrow_mut().remove(path);
        Ok(())
    }
}

/// Reads a whole (small) file, refusing files larger than `limit` bytes.
pub fn read_file(storage: &dyn Storage, path: &str, limit: usize) -> Result<Vec<u8>, StorageError> {
    let size = storage.size(path)?;
    let size = usize::try_from(size).unwrap_or(usize::MAX);
    if size > limit {
        return Err(StorageError::Unsupported);
    }
    let mut data = alloc::vec![0; size];
    let mut filled = 0;
    while filled < size {
        let n = storage.read_at(path, filled as u64, &mut data[filled..])?;
        if n == 0 {
            break;
        }
        filled += n;
    }
    data.truncate(filled);
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn reads_writes_and_lists() {
        let storage = MemoryStorage::new()
            .with("books/a.txt", "hello")
            .with("books/sub/b.txt", "x");
        let mut buf = [0u8; 3];
        assert_eq!(storage.read_at("books/a.txt", 2, &mut buf), Ok(3));
        assert_eq!(&buf, b"llo");
        assert_eq!(storage.read_at("books/a.txt", 10, &mut buf), Ok(0));
        storage.append("notes/n.txt", b"ab").unwrap();
        storage.append("notes/n.txt", b"c").unwrap();
        assert_eq!(read_file(&storage, "notes/n.txt", 10).unwrap(), b"abc");
        assert_eq!(
            read_file(&storage, "notes/n.txt", 2),
            Err(StorageError::Unsupported)
        );

        let mut entries = vec![];
        storage
            .list("books", &mut |e| {
                entries.push((String::from(e.name), e.is_dir))
            })
            .unwrap();
        entries.sort();
        assert_eq!(
            entries,
            vec![(String::from("a.txt"), false), (String::from("sub"), true)]
        );
        assert_eq!(
            storage.list("nope", &mut |_| {}),
            Err(StorageError::NotFound)
        );
        assert_eq!(storage.size("../x"), Err(StorageError::InvalidPath));
    }
}
