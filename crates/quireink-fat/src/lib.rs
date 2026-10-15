//! QuireInk's storage on a FAT file system (FAT12, FAT16, FAT32) with long
//! file names, e.g. the X4's SD card: [`FatStorage`] over any disk that
//! reads, writes and seeks in bytes (see [`fatfs::ReadWriteSeek`]).
//!
//! The file read last stays open, so reading a book a piece at a time
//! does not look it up again for every piece. Files are replaced through
//! a temporary file beside them, so a write cut short by an empty battery
//! leaves the old content.

#![no_std]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use core::cell::RefCell;

use fatfs::{
    DefaultTimeProvider, Dir, File, FileSystem, LossyOemCpConverter, Read, ReadWriteSeek, Seek,
    SeekFrom, Write,
};
use quireink_platform::storage::is_valid_path;
use quireink_platform::{DirEntry, Storage, StorageError};

pub use fatfs;

type FatFile<IO> = File<'static, IO, DefaultTimeProvider, LossyOemCpConverter>;
type FatDir<IO> = Dir<'static, IO, DefaultTimeProvider, LossyOemCpConverter>;

/// QuireInk's storage on a FAT file system that lives as long as the
/// device runs (`Box::leak` it).
pub struct FatStorage<IO: ReadWriteSeek + 'static> {
    fs: &'static FileSystem<IO>,
    /// The file read last, by its path.
    open: RefCell<Option<(String, FatFile<IO>)>>,
}

impl<IO: ReadWriteSeek + 'static> core::fmt::Debug for FatStorage<IO> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FatStorage").finish_non_exhaustive()
    }
}

fn error<E>(e: fatfs::Error<E>) -> StorageError {
    match e {
        fatfs::Error::NotFound => StorageError::NotFound,
        fatfs::Error::NotEnoughSpace => StorageError::Full,
        fatfs::Error::InvalidInput
        | fatfs::Error::InvalidFileNameLength
        | fatfs::Error::UnsupportedFileNameCharacter => StorageError::InvalidPath,
        _ => StorageError::Io,
    }
}

fn check(path: &str) -> Result<(), StorageError> {
    if is_valid_path(path) {
        Ok(())
    } else {
        Err(StorageError::InvalidPath)
    }
}

/// The folder of `path` (`""` for the root).
fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

impl<IO: ReadWriteSeek + 'static> FatStorage<IO> {
    pub fn new(fs: &'static FileSystem<IO>) -> Self {
        Self {
            fs,
            open: RefCell::new(None),
        }
    }

    fn root(&self) -> FatDir<IO> {
        self.fs.root_dir()
    }

    fn dir(&self, path: &str) -> Result<FatDir<IO>, StorageError> {
        let path = path.trim_matches('/');
        if path.is_empty() {
            Ok(self.root())
        } else {
            self.root().open_dir(path).map_err(error)
        }
    }

    /// Runs `f` with `path` open, keeping it open for the next call.
    fn with_open<T>(
        &self,
        path: &str,
        f: impl FnOnce(&mut FatFile<IO>) -> Result<T, StorageError>,
    ) -> Result<T, StorageError> {
        check(path)?;
        let mut open = self.open.borrow_mut();
        if open.as_ref().is_none_or(|(p, _)| p != path) {
            *open = None;
            let file = self.root().open_file(path).map_err(error)?;
            *open = Some((path.to_string(), file));
        }
        match open.as_mut() {
            Some((_, file)) => f(file),
            None => Err(StorageError::Io),
        }
    }

    /// Forgets the open file if it is `path` (or below it).
    fn close(&self, path: &str) {
        let mut open = self.open.borrow_mut();
        if open
            .as_ref()
            .is_some_and(|(p, _)| p == path || p.starts_with(&format!("{path}/")))
        {
            *open = None;
        }
    }

    fn write_all(file: &mut FatFile<IO>, data: &[u8]) -> Result<(), StorageError> {
        file.write_all(data).map_err(error)?;
        file.flush().map_err(error)
    }
}

impl<IO: ReadWriteSeek + 'static> Storage for FatStorage<IO> {
    fn read_at(&self, path: &str, offset: u64, buf: &mut [u8]) -> Result<usize, StorageError> {
        self.with_open(path, |file| {
            file.seek(SeekFrom::Start(offset)).map_err(error)?;
            let mut read = 0;
            while read < buf.len() {
                match file.read(&mut buf[read..]).map_err(error)? {
                    0 => break,
                    n => read += n,
                }
            }
            Ok(read)
        })
    }

    fn size(&self, path: &str) -> Result<u64, StorageError> {
        self.with_open(path, |file| file.seek(SeekFrom::End(0)).map_err(error))
    }

    fn write(&self, path: &str, data: &[u8]) -> Result<(), StorageError> {
        check(path)?;
        self.close(path);
        self.create_dir(parent(path))?;
        let temp = format!("{path}.tmp");
        let root = self.root();
        {
            let mut file = root.create_file(&temp).map_err(error)?;
            file.truncate().map_err(error)?;
            Self::write_all(&mut file, data)?;
        }
        match root.remove(path) {
            Ok(()) | Err(fatfs::Error::NotFound) => {}
            Err(e) => return Err(error(e)),
        }
        root.rename(&temp, &root, path).map_err(error)
    }

    fn append(&self, path: &str, data: &[u8]) -> Result<(), StorageError> {
        check(path)?;
        self.close(path);
        self.create_dir(parent(path))?;
        let mut file = self.root().create_file(path).map_err(error)?;
        file.seek(SeekFrom::End(0)).map_err(error)?;
        Self::write_all(&mut file, data)
    }

    fn list(&self, dir: &str, visit: &mut dyn FnMut(DirEntry<'_>)) -> Result<(), StorageError> {
        if !dir.is_empty() {
            check(dir)?;
        }
        for entry in self.dir(dir)?.iter() {
            let entry = entry.map_err(error)?;
            let name = entry.file_name();
            if name == "." || name == ".." {
                continue;
            }
            let is_dir = entry.is_dir();
            visit(DirEntry {
                name: &name,
                is_dir,
                size: if is_dir { 0 } else { entry.len() },
            });
        }
        Ok(())
    }

    fn remove(&self, path: &str) -> Result<(), StorageError> {
        check(path)?;
        self.close(path);
        match self.root().remove(path) {
            Ok(()) | Err(fatfs::Error::NotFound) => Ok(()),
            Err(e) => Err(error(e)),
        }
    }

    fn rename(&self, from: &str, to: &str) -> Result<(), StorageError> {
        check(from)?;
        check(to)?;
        if from == to {
            return Ok(());
        }
        self.close(from);
        self.close(to);
        self.create_dir(parent(to))?;
        let root = self.root();
        // A file there is replaced.
        if self.size(to).is_ok() {
            self.close(to);
            root.remove(to).map_err(error)?;
        }
        root.rename(from, &root, to).map_err(error)
    }

    fn create_dir(&self, path: &str) -> Result<(), StorageError> {
        let path = path.trim_matches('/');
        if path.is_empty() {
            return Ok(());
        }
        check(path)?;
        let mut dir = self.root();
        for part in path.split('/').filter(|p| !p.is_empty()) {
            dir = dir.create_dir(part).map_err(error)?;
        }
        Ok(())
    }

    fn remove_dir_all(&self, path: &str) -> Result<(), StorageError> {
        check(path)?;
        self.close(path);
        let dir = match self.dir(path) {
            Ok(dir) => dir,
            Err(StorageError::NotFound) => return Ok(()),
            Err(e) => return Err(e),
        };
        let mut names: alloc::vec::Vec<(String, bool)> = alloc::vec::Vec::new();
        for entry in dir.iter() {
            let entry = entry.map_err(error)?;
            let name = entry.file_name();
            if name != "." && name != ".." {
                names.push((name, entry.is_dir()));
            }
        }
        for (name, is_dir) in names {
            let child = format!("{path}/{name}");
            if is_dir {
                self.remove_dir_all(&child)?;
            } else {
                self.remove(&child)?;
            }
        }
        self.root().remove(path).map_err(error)
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use alloc::boxed::Box;
    use alloc::vec;
    use alloc::vec::Vec;
    use fatfs::{FormatVolumeOptions, FsOptions, StdIoWrapper};
    use std::io::Cursor;

    type Disk = StdIoWrapper<Cursor<Vec<u8>>>;

    /// A fresh FAT disk of 4 MB, formatted.
    fn storage() -> FatStorage<Disk> {
        let mut disk = StdIoWrapper::from(Cursor::new(vec![0u8; 4 << 20]));
        fatfs::format_volume(&mut disk, FormatVolumeOptions::new()).unwrap();
        disk.seek(SeekFrom::Start(0)).unwrap();
        let fs = FileSystem::new(disk, FsOptions::new()).unwrap();
        FatStorage::new(Box::leak(Box::new(fs)))
    }

    fn names(storage: &FatStorage<Disk>, dir: &str) -> Vec<(String, bool, u64)> {
        let mut out = Vec::new();
        storage
            .list(dir, &mut |e| {
                out.push((e.name.to_string(), e.is_dir, e.size))
            })
            .unwrap();
        out.sort();
        out
    }

    #[test]
    fn long_names_written_read_and_listed() {
        let s = storage();
        let path = "Books/Lewis Carroll/Alice's Adventures in Wonderland.epub";
        s.write(path, b"the whole book").unwrap();
        assert_eq!(s.size(path), Ok(14));
        let mut buf = [0u8; 5];
        assert_eq!(s.read_at(path, 4, &mut buf), Ok(5));
        assert_eq!(&buf, b"whole");
        assert_eq!(s.read_at(path, 12, &mut buf), Ok(2));
        assert_eq!(&buf[..2], b"ok");
        assert_eq!(
            names(&s, "Books/Lewis Carroll"),
            [(
                String::from("Alice's Adventures in Wonderland.epub"),
                false,
                14
            )]
        );
        assert_eq!(names(&s, ""), [(String::from("Books"), true, 0)]);
        // Hidden folders with dots, as QuireInk keeps its data.
        s.write(".quireink/plugins/reader/annotations.txt", b"a")
            .unwrap();
        assert!(s.exists(".quireink/plugins/reader/annotations.txt"));
        assert!(!s.exists("missing.txt"));
    }

    #[test]
    fn written_anew_appended_moved_and_removed() {
        let s = storage();
        s.write("a.txt", b"first, longer content").unwrap();
        // The file read last is open: a new content is seen anyway.
        let mut buf = [0u8; 32];
        assert_eq!(s.read_at("a.txt", 0, &mut buf), Ok(21));
        s.write("a.txt", b"second").unwrap();
        assert_eq!(s.read_at("a.txt", 0, &mut buf), Ok(6));
        assert_eq!(&buf[..6], b"second");
        s.append("a.txt", b" and more").unwrap();
        assert_eq!(s.size("a.txt"), Ok(15));
        assert_eq!(
            names(&s, ""),
            [(String::from("a.txt"), false, 15)],
            "no temporary left"
        );
        // Moved over another file, into a new folder.
        s.write("b/c.txt", b"old").unwrap();
        s.rename("a.txt", "b/c.txt").unwrap();
        assert!(!s.exists("a.txt"));
        assert_eq!(s.size("b/c.txt"), Ok(15));
        s.remove("b/c.txt").unwrap();
        s.remove("b/c.txt").unwrap();
        assert!(names(&s, "b").is_empty());
        // Folders with all in them.
        s.write("d/e/f.txt", b"x").unwrap();
        s.create_dir("d/g").unwrap();
        s.create_dir("d/g").unwrap();
        s.remove_dir_all("d").unwrap();
        s.remove_dir_all("d").unwrap();
        assert_eq!(names(&s, ""), [(String::from("b"), true, 0)]);
        assert_eq!(
            s.read_at("../x", 0, &mut buf),
            Err(StorageError::InvalidPath)
        );
    }
}
