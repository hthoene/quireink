//! Storage backed by a directory of the host file system: the emulated SD
//! card of the desktop emulator, or an app's folder on Android.

use std::cell::RefCell;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use quireink_platform::storage::is_valid_path;
use quireink_platform::{DirEntry, Storage, StorageError};

/// Maps storage paths below a root directory.
#[derive(Debug)]
pub struct DirStorage {
    root: PathBuf,
    /// The file read last, kept open for sequential reads.
    open: RefCell<Option<(String, File)>>,
}

impl DirStorage {
    /// The directory the storage is rooted at.
    pub fn root(&self) -> &std::path::Path {
        &self.root
    }

    pub fn new(root: impl Into<PathBuf>) -> io::Result<Self> {
        let root = root.into();
        fs::create_dir_all(&root)?;
        Ok(Self {
            root,
            open: RefCell::new(None),
        })
    }

    fn resolve(&self, path: &str) -> Result<PathBuf, StorageError> {
        if path.is_empty() {
            return Ok(self.root.clone());
        }
        if !is_valid_path(path) {
            return Err(StorageError::InvalidPath);
        }
        Ok(path
            .split('/')
            .fold(self.root.clone(), |p, part| p.join(part)))
    }

    /// Closes the cached handle if it refers to `path` (before writes).
    fn forget(&self, path: &str) {
        let mut open = self.open.borrow_mut();
        if open.as_ref().is_some_and(|(p, _)| p == path) {
            *open = None;
        }
    }
}

fn error(e: io::Error) -> StorageError {
    match e.kind() {
        io::ErrorKind::NotFound => StorageError::NotFound,
        io::ErrorKind::StorageFull => StorageError::Full,
        io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem => {
            StorageError::Unsupported
        }
        _ => StorageError::Io,
    }
}

fn create_parent(path: &Path) -> Result<(), StorageError> {
    match path.parent() {
        Some(parent) => fs::create_dir_all(parent).map_err(error),
        None => Ok(()),
    }
}

impl Storage for DirStorage {
    fn read_at(&self, path: &str, offset: u64, buf: &mut [u8]) -> Result<usize, StorageError> {
        let mut open = self.open.borrow_mut();
        if open.as_ref().is_none_or(|(p, _)| p != path) {
            let file = File::open(self.resolve(path)?).map_err(error)?;
            *open = Some((path.to_string(), file));
        }
        let Some((_, file)) = open.as_mut() else {
            return Err(StorageError::Io);
        };
        file.seek(SeekFrom::Start(offset)).map_err(error)?;
        let mut filled = 0;
        while filled < buf.len() {
            match file.read(&mut buf[filled..]) {
                Ok(0) => break,
                Ok(n) => filled += n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(error(e)),
            }
        }
        Ok(filled)
    }

    fn size(&self, path: &str) -> Result<u64, StorageError> {
        let metadata = fs::metadata(self.resolve(path)?).map_err(error)?;
        if metadata.is_file() {
            Ok(metadata.len())
        } else {
            Err(StorageError::NotFound)
        }
    }

    fn write(&self, path: &str, data: &[u8]) -> Result<(), StorageError> {
        let target = self.resolve(path)?;
        self.forget(path);
        create_parent(&target)?;
        // Write a temporary file and rename it: never a half-written file.
        let temp = target.with_extension("quireink-tmp");
        fs::write(&temp, data).map_err(error)?;
        fs::rename(&temp, &target).map_err(error)
    }

    fn append(&self, path: &str, data: &[u8]) -> Result<(), StorageError> {
        let target = self.resolve(path)?;
        self.forget(path);
        create_parent(&target)?;
        // To the end by seeking, not with `O_APPEND`: Android's shared
        // storage (FUSE) does not always honour it and writes at the
        // start instead, which broke uploads of more than one part.
        OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&target)
            .and_then(|mut file| {
                file.seek(SeekFrom::End(0))?;
                file.write_all(data)
            })
            .map_err(error)
    }

    fn list(&self, dir: &str, visit: &mut dyn FnMut(DirEntry<'_>)) -> Result<(), StorageError> {
        for entry in fs::read_dir(self.resolve(dir)?).map_err(error)? {
            let entry = entry.map_err(error)?;
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            visit(DirEntry {
                name: &name,
                is_dir: metadata.is_dir(),
                size: if metadata.is_file() {
                    metadata.len()
                } else {
                    0
                },
            });
        }
        Ok(())
    }

    fn remove(&self, path: &str) -> Result<(), StorageError> {
        let target = self.resolve(path)?;
        self.forget(path);
        match fs::remove_file(target) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(error(e)),
            _ => Ok(()),
        }
    }

    fn rename(&self, from: &str, to: &str) -> Result<(), StorageError> {
        let (source, target) = (self.resolve(from)?, self.resolve(to)?);
        // Files kept open must not block the move (Windows).
        *self.open.borrow_mut() = None;
        create_parent(&target)?;
        fs::rename(source, target).map_err(error)
    }

    fn create_dir(&self, path: &str) -> Result<(), StorageError> {
        fs::create_dir_all(self.resolve(path)?).map_err(error)
    }

    fn remove_dir_all(&self, path: &str) -> Result<(), StorageError> {
        let target = self.resolve(path)?;
        *self.open.borrow_mut() = None;
        match fs::remove_dir_all(target) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(error(e)),
            _ => Ok(()),
        }
    }

    fn local_path(&self, path: &str, with: &mut dyn FnMut(&str)) {
        if let Some(real) = self.resolve(path).ok().as_deref().and_then(Path::to_str) {
            with(real);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_files_below_the_root() {
        let root = std::env::temp_dir().join(format!("quireink-storage-{}", std::process::id()));
        let storage = DirStorage::new(&root).unwrap();
        storage.write("a/b.txt", b"hello").unwrap();
        storage.append("a/b.txt", b" world").unwrap();
        let mut buf = [0u8; 5];
        assert_eq!(storage.read_at("a/b.txt", 6, &mut buf), Ok(5));
        assert_eq!(&buf, b"world");
        assert_eq!(storage.size("a/b.txt"), Ok(11));
        let mut names = Vec::new();
        storage
            .list("a", &mut |e| names.push(e.name.to_string()))
            .unwrap();
        assert_eq!(names, ["b.txt"]);
        assert_eq!(storage.size("../outside"), Err(StorageError::InvalidPath));
        storage.remove("a/b.txt").unwrap();
        assert_eq!(storage.size("a/b.txt"), Err(StorageError::NotFound));
        fs::remove_dir_all(root).unwrap();
    }
}
