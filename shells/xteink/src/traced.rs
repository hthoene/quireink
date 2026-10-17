//! The storage, with what is done on it noted while asked to (`trace` over
//! USB): which files a page turn reads and writes, to find what is read
//! again and again.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;

use critical_section::Mutex;
use quireink_platform::{DirEntry, Storage, StorageError};

/// What is noted at most.
const MAX: usize = 400;

static TRACE: Mutex<RefCell<Option<Vec<String>>>> = Mutex::new(RefCell::new(None));

/// Starts noting (`true`), or stops and gives what was noted.
pub fn trace(start: bool) -> Option<Vec<String>> {
    critical_section::with(|cs| {
        let mut trace = TRACE.borrow_ref_mut(cs);
        if start {
            *trace = Some(Vec::new());
            None
        } else {
            trace.take()
        }
    })
}

fn note(what: impl FnOnce() -> String) {
    critical_section::with(|cs| {
        if let Some(trace) = TRACE.borrow_ref_mut(cs).as_mut()
            && trace.len() < MAX
        {
            trace.push(what());
        }
    });
}

pub struct Traced(pub Box<dyn Storage>);

impl Storage for Traced {
    fn read_at(&self, path: &str, offset: u64, buf: &mut [u8]) -> Result<usize, StorageError> {
        note(|| format!("read {path} {offset} {}", buf.len()));
        self.0.read_at(path, offset, buf)
    }

    fn size(&self, path: &str) -> Result<u64, StorageError> {
        note(|| format!("size {path}"));
        self.0.size(path)
    }

    fn write(&self, path: &str, data: &[u8]) -> Result<(), StorageError> {
        note(|| format!("write {path} {}", data.len()));
        self.0.write(path, data)
    }

    fn append(&self, path: &str, data: &[u8]) -> Result<(), StorageError> {
        note(|| format!("append {path} {}", data.len()));
        self.0.append(path, data)
    }

    fn list(&self, dir: &str, visit: &mut dyn FnMut(DirEntry<'_>)) -> Result<(), StorageError> {
        note(|| format!("list {dir}"));
        self.0.list(dir, visit)
    }

    fn remove(&self, path: &str) -> Result<(), StorageError> {
        note(|| format!("remove {path}"));
        self.0.remove(path)
    }

    fn exists(&self, path: &str) -> bool {
        note(|| format!("exists {path}"));
        self.0.exists(path)
    }

    fn rename(&self, from: &str, to: &str) -> Result<(), StorageError> {
        note(|| format!("rename {from} {to}"));
        self.0.rename(from, to)
    }

    fn create_dir(&self, path: &str) -> Result<(), StorageError> {
        note(|| format!("create_dir {path}"));
        self.0.create_dir(path)
    }

    fn remove_dir_all(&self, path: &str) -> Result<(), StorageError> {
        note(|| format!("remove_dir_all {path}"));
        self.0.remove_dir_all(path)
    }

    fn sync(&self) -> Result<(), StorageError> {
        note(|| "sync".into());
        self.0.sync()
    }
}
