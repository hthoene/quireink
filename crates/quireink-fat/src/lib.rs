//! QuireInk's storage on a FAT file system (FAT12, FAT16, FAT32) with long
//! file names, e.g. the X4's SD card: [`FatStorage`] over any disk that
//! reads, writes and seeks in bytes (see [`fatfs::ReadWriteSeek`]), such
//! as a [`BlockDisk`] over an SD card's blocks.
//!
//! The [`OPEN_FILES`] files read last stay open, so reading a book a piece
//! at a time, often from two files by turns (an index and its text), does
//! not look them up again for every piece. Files appended to stay open
//! the same way, written back only when they are wanted otherwise (listed,
//! renamed, removed, replaced), when they make room for another file, or on
//! [`Storage::sync`]: a picture decoded into a file in hundreds of pieces
//! is not looked up and written back hundreds of times. Files are replaced
//! through a temporary file beside them, so a write cut short by an empty
//! battery leaves the old content.

#![no_std]

extern crate alloc;

mod disk;

pub use disk::{BLOCK, BlockDisk, Blocks, DiskError, VolumeError};

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

/// Files kept open (each takes about a hundred bytes).
pub const OPEN_FILES: usize = 8;
type FatDir<IO> = Dir<'static, IO, DefaultTimeProvider, LossyOemCpConverter>;

/// A file kept open.
struct Open<IO: ReadWriteSeek + 'static> {
    path: String,
    file: FatFile<IO>,
    /// Appended to since it was last written back.
    dirty: bool,
}

impl<IO: ReadWriteSeek + 'static> Open<IO> {
    /// Writes back what was appended (its size in its folder, the disk's
    /// blocks).
    fn flush(&mut self) -> Result<(), StorageError> {
        if self.dirty {
            self.dirty = false;
            self.file.flush().map_err(error)?;
        }
        Ok(())
    }
}

/// QuireInk's storage on a FAT file system that lives as long as the
/// device runs (`Box::leak` it).
pub struct FatStorage<IO: ReadWriteSeek + 'static> {
    fs: &'static FileSystem<IO>,
    /// The files used last, the latest last.
    open: RefCell<alloc::vec::Vec<Open<IO>>>,
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
            open: RefCell::new(alloc::vec::Vec::new()),
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

    /// Runs `f` with `path` open (created first with its folders if
    /// `create`), keeping it open for the next call.
    fn with_open<T>(
        &self,
        path: &str,
        create: bool,
        f: impl FnOnce(&mut Open<IO>) -> Result<T, StorageError>,
    ) -> Result<T, StorageError> {
        let mut open = self.open.borrow_mut();
        match open.iter().position(|o| o.path == path) {
            // The latest last.
            Some(i) => {
                let file = open.remove(i);
                open.push(file);
            }
            None => {
                // Checked once, when opened: a book is read 4 bytes at a
                // time, many times a page.
                check(path)?;
                if open.len() == OPEN_FILES {
                    open.remove(0).flush()?;
                }
                let file = if create {
                    self.create_dir(parent(path))?;
                    self.root().create_file(path)
                } else {
                    self.root().open_file(path)
                }
                .map_err(error)?;
                open.push(Open {
                    path: path.to_string(),
                    file,
                    dirty: false,
                });
            }
        }
        match open.last_mut() {
            Some(file) => f(file),
            None => Err(StorageError::Io),
        }
    }

    /// Closes the open files at `path` (or below it), writing back what
    /// was appended to them.
    fn close(&self, path: &str) -> Result<(), StorageError> {
        let below = format!("{path}/");
        let mut result = Ok(());
        self.open.borrow_mut().retain_mut(|o| {
            let keep = o.path != path && !o.path.starts_with(&below);
            if !keep && let Err(e) = o.flush() {
                result = Err(e);
            }
            keep
        });
        result
    }

    fn write_all(file: &mut FatFile<IO>, data: &[u8]) -> Result<(), StorageError> {
        file.write_all(data).map_err(error)?;
        file.flush().map_err(error)
    }
}

impl<IO: ReadWriteSeek + 'static> Storage for FatStorage<IO> {
    fn read_at(&self, path: &str, offset: u64, buf: &mut [u8]) -> Result<usize, StorageError> {
        self.with_open(path, false, |Open { file, .. }| {
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
        self.with_open(path, false, |o| {
            o.file.seek(SeekFrom::End(0)).map_err(error)
        })
    }

    fn write(&self, path: &str, data: &[u8]) -> Result<(), StorageError> {
        check(path)?;
        self.close(path)?;
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
        self.with_open(path, true, |o| {
            o.file.seek(SeekFrom::End(0)).map_err(error)?;
            o.dirty = true;
            o.file.write_all(data).map_err(error)
        })
    }

    fn sync(&self) -> Result<(), StorageError> {
        let mut result = Ok(());
        for o in self.open.borrow_mut().iter_mut() {
            if let Err(e) = o.flush() {
                result = Err(e);
            }
        }
        result
    }

    fn list(&self, dir: &str, visit: &mut dyn FnMut(DirEntry<'_>)) -> Result<(), StorageError> {
        if !dir.is_empty() {
            check(dir)?;
        }
        // The sizes in the folders, as appended.
        self.sync()?;
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
        self.close(path)?;
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
        self.close(from)?;
        self.close(to)?;
        self.create_dir(parent(to))?;
        let root = self.root();
        // A file there is replaced.
        if self.size(to).is_ok() {
            self.close(to)?;
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
        self.close(path)?;
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

    /// A disk of blocks in memory.
    struct Card {
        data: Vec<u8>,
    }

    impl Blocks for Card {
        fn read_block(&mut self, index: u32, block: &mut [u8; BLOCK]) -> Result<(), DiskError> {
            let at = index as usize * BLOCK;
            block.copy_from_slice(self.data.get(at..at + BLOCK).ok_or(DiskError)?);
            Ok(())
        }
        fn write_block(&mut self, index: u32, block: &[u8; BLOCK]) -> Result<(), DiskError> {
            let at = index as usize * BLOCK;
            self.data
                .get_mut(at..at + BLOCK)
                .ok_or(DiskError)?
                .copy_from_slice(block);
            Ok(())
        }
        fn blocks(&mut self) -> Result<u32, DiskError> {
            Ok((self.data.len() / BLOCK) as u32)
        }
    }

    #[test]
    fn on_a_card_of_blocks() {
        let mut card = BlockDisk::new(Card {
            data: vec![0; 2 << 20],
        })
        .unwrap();
        fatfs::format_volume(&mut card, FormatVolumeOptions::new()).unwrap();
        card.flush().unwrap();
        card.seek(SeekFrom::Start(0)).unwrap();
        let fs = FileSystem::new(card, FsOptions::new()).unwrap();
        let s = FatStorage::new(Box::leak(Box::new(fs)));
        let text: Vec<u8> = (0..3000u32).map(|i| (i % 251) as u8).collect();
        s.write("Books/A long name.txt", &text).unwrap();
        let mut buf = [0u8; 700];
        assert_eq!(s.read_at("Books/A long name.txt", 1000, &mut buf), Ok(700));
        assert_eq!(&buf[..], &text[1000..1700]);
        assert_eq!(s.size("Books/A long name.txt"), Ok(3000));
    }

    /// A card of `blocks` blocks with a FAT volume formatted at block
    /// `first` and `table` written into its first blocks.
    fn card_with(blocks: usize, first: usize, table: &[(usize, &[u8])]) -> Card {
        let mut volume = StdIoWrapper::from(Cursor::new(vec![0u8; (blocks - first) * BLOCK]));
        fatfs::format_volume(&mut volume, FormatVolumeOptions::new()).unwrap();
        let mut data = vec![0u8; blocks * BLOCK];
        data[first * BLOCK..].copy_from_slice(&volume.into_inner().into_inner());
        for (at, bytes) in table {
            data[*at..*at + bytes.len()].copy_from_slice(bytes);
        }
        Card { data }
    }

    /// A partition table entry of an MBR (`kind` e.g. 0x0C, FAT32).
    fn mbr_entry(kind: u8, first: u32, count: u32) -> [u8; 16] {
        let mut entry = [0u8; 16];
        entry[4] = kind;
        entry[8..12].copy_from_slice(&first.to_le_bytes());
        entry[12..16].copy_from_slice(&count.to_le_bytes());
        entry
    }

    fn opens(card: Card) -> Result<(), VolumeError> {
        let disk = BlockDisk::volume(card)?;
        let fs = FileSystem::new(disk, FsOptions::new()).unwrap();
        let s = FatStorage::new(Box::leak(Box::new(fs)));
        s.write("Books/a.txt", b"found").unwrap();
        assert_eq!(s.size("Books/a.txt"), Ok(5));
        Ok(())
    }

    #[test]
    fn volumes_found_in_partitions() {
        let blocks = 8192;
        // The file system from the first block, without a table.
        assert_eq!(opens(card_with(blocks, 0, &[])), Ok(()));
        // An MBR, as on most cards, its partition after an empty one.
        let first = 2048;
        let entry = mbr_entry(0x0C, first as u32, (blocks - first) as u32);
        let table = [(446 + 16, &entry[..]), (510, &[0x55, 0xAA][..])];
        assert_eq!(opens(card_with(blocks, first, &table)), Ok(()));
        // A GPT: a protective MBR, the header in block 1, entries in 2.
        let protective = mbr_entry(0xEE, 1, (blocks - 1) as u32);
        let mut header = [0u8; 92];
        header[..8].copy_from_slice(b"EFI PART");
        header[72..80].copy_from_slice(&2u64.to_le_bytes());
        header[80..84].copy_from_slice(&4u32.to_le_bytes());
        header[84..88].copy_from_slice(&128u32.to_le_bytes());
        let mut entry = [0u8; 48];
        entry[..16].copy_from_slice(&[0xA2; 16]);
        entry[32..40].copy_from_slice(&(first as u64).to_le_bytes());
        entry[40..48].copy_from_slice(&((blocks - 1) as u64).to_le_bytes());
        let table = [
            (446, &protective[..]),
            (510, &[0x55, 0xAA][..]),
            (BLOCK, &header[..]),
            (2 * BLOCK + 128, &entry[..]),
        ];
        assert_eq!(opens(card_with(blocks, first, &table)), Ok(()));
        // exFAT, as cards above 32 GB come.
        let mut card = card_with(
            blocks,
            first,
            &[
                (446, &mbr_entry(0x07, 2048, 6144)[..]),
                (510, &[0x55, 0xAA][..]),
            ],
        );
        card.data[first * BLOCK + 3..first * BLOCK + 11].copy_from_slice(b"EXFAT   ");
        assert_eq!(opens(card).unwrap_err(), VolumeError::ExFat);
        // Nothing at all.
        let card = Card {
            data: vec![0; blocks * BLOCK],
        };
        assert_eq!(opens(card).unwrap_err(), VolumeError::NoFat);
    }

    #[test]
    fn many_blocks_and_files_by_turns() {
        // More blocks than are kept, written and read by turns from more
        // files than stay open, on a card of blocks.
        let mut card = BlockDisk::new(Card {
            data: vec![0; 4 << 20],
        })
        .unwrap();
        fatfs::format_volume(&mut card, FormatVolumeOptions::new()).unwrap();
        card.flush().unwrap();
        card.seek(SeekFrom::Start(0)).unwrap();
        let fs = FileSystem::new(card, FsOptions::new()).unwrap();
        let s = FatStorage::new(Box::leak(Box::new(fs)));
        let content =
            |n: u32| -> Vec<u8> { (0..20_000u32).map(|i| ((i * 7 + n) % 251) as u8).collect() };
        let names: Vec<String> = (0..6).map(|n| alloc::format!("book/{n}.txt")).collect();
        for (n, name) in names.iter().enumerate() {
            s.write(name, &content(n as u32)).unwrap();
        }
        let mut buf = [0u8; 300];
        for round in 0..40u32 {
            let n = (round * 5 % 6) as usize;
            let at = u64::from(round * 487 % 19_000);
            assert_eq!(s.read_at(&names[n], at, &mut buf), Ok(300));
            let at = at as usize;
            assert_eq!(
                &buf[..],
                &content(n as u32)[at..at + 300],
                "{} at {at}",
                names[n]
            );
        }
        // Written again while open: read as written.
        s.write(&names[0], b"short").unwrap();
        assert_eq!(s.read_at(&names[0], 0, &mut buf), Ok(5));
    }

    #[test]
    fn appended_by_turns_and_written_back() {
        let s = storage();
        s.write("book.epub", &[7u8; 5000]).unwrap();
        let files: Vec<String> = (0..OPEN_FILES + 2)
            .map(|n| alloc::format!(".cache/{n}.bin"))
            .collect();
        let mut buf = [0u8; 100];
        // More files than stay open, appended to by turns, with a book
        // read between.
        for round in 0..30u8 {
            for (n, name) in files.iter().enumerate() {
                s.append(name, &[round, n as u8]).unwrap();
            }
            assert_eq!(
                s.read_at("book.epub", u64::from(round) * 100, &mut buf),
                Ok(100)
            );
        }
        for (n, name) in files.iter().enumerate() {
            assert_eq!(s.size(name), Ok(60));
            let mut all = [0u8; 60];
            assert_eq!(s.read_at(name, 0, &mut all), Ok(60));
            let want: Vec<u8> = (0..30u8).flat_map(|r| [r, n as u8]).collect();
            assert_eq!(&all[..], &want[..], "{name}");
        }
        // Listed with their sizes as appended.
        assert!(names(&s, ".cache").iter().all(|(_, _, size)| *size == 60));
        // Written back on sync: opened anew, a file has it all.
        s.append(&files[0], b"end").unwrap();
        s.sync().unwrap();
        let mut fresh = s.root().open_file(&files[0]).unwrap();
        assert_eq!(fresh.seek(SeekFrom::End(0)).unwrap(), 63);
        // And moved with it all.
        s.append(&files[1], b"end").unwrap();
        s.rename(&files[1], "moved.bin").unwrap();
        assert_eq!(s.size("moved.bin"), Ok(63));
        let mut fresh = s.root().open_file("moved.bin").unwrap();
        assert_eq!(fresh.seek(SeekFrom::End(0)).unwrap(), 63);
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
