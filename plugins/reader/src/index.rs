//! A page index stored on the device: the start locator of every page for
//! one book and one layout. It is built in the background and read in place,
//! so even books with thousands of pages need no RAM for it.

use alloc::string::String;

use quireink_sdk::Storage;
use quireink_sdk::document::Locator;

const ENTRY: u64 = 8;
const SENTINEL: Locator = Locator::new(u32::MAX, u32::MAX);

#[derive(Debug, Clone)]
pub(crate) struct PageIndex {
    path: String,
    /// Number of page entries (without the completion sentinel).
    pages: u32,
    complete: bool,
}

impl PageIndex {
    pub(crate) fn open(storage: &dyn Storage, path: String) -> Self {
        let entries = storage.size(&path).map_or(0, |size| (size / ENTRY) as u32);
        let mut index = Self {
            path,
            pages: entries,
            complete: false,
        };
        if entries > 0 && index.entry(storage, entries - 1) == Some(SENTINEL) {
            index.pages = entries - 1;
            index.complete = true;
        }
        index
    }

    pub(crate) fn pages(&self) -> u32 {
        self.pages
    }

    pub(crate) fn is_complete(&self) -> bool {
        self.complete
    }

    pub(crate) fn entry(&self, storage: &dyn Storage, page: u32) -> Option<Locator> {
        let mut buf = [0u8; ENTRY as usize];
        let n = storage
            .read_at(&self.path, u64::from(page) * ENTRY, &mut buf)
            .ok()?;
        if n != buf.len() {
            return None;
        }
        let word = |i: usize| u32::from_le_bytes([buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]);
        Some(Locator::new(word(0), word(4)))
    }

    /// The last indexed page start, from which indexing continues.
    pub(crate) fn last(&self, storage: &dyn Storage) -> Option<Locator> {
        self.pages
            .checked_sub(1)
            .and_then(|page| self.entry(storage, page))
    }

    /// Appends page starts; returns `false` if the storage refused.
    pub(crate) fn append(&mut self, storage: &dyn Storage, starts: &[Locator]) -> bool {
        let mut data = alloc::vec::Vec::with_capacity(starts.len() * ENTRY as usize);
        for start in starts {
            data.extend_from_slice(&start.section.to_le_bytes());
            data.extend_from_slice(&start.offset.to_le_bytes());
        }
        let ok = storage.append(&self.path, &data).is_ok();
        if ok {
            self.pages += starts.len() as u32;
        }
        ok
    }

    pub(crate) fn finish(&mut self, storage: &dyn Storage) {
        if self.append(storage, &[SENTINEL]) {
            self.pages -= 1;
            self.complete = true;
        }
    }

    /// The page containing `at`: the 0-based index of the last page start
    /// at or before it, and whether `at` is exactly that start.
    pub(crate) fn locate(&self, storage: &dyn Storage, at: Locator) -> Option<(u32, bool)> {
        let (mut low, mut high) = (0u32, self.pages);
        while low < high {
            let mid = low + (high - low) / 2;
            if self.entry(storage, mid)? <= at {
                low = mid + 1;
            } else {
                high = mid;
            }
        }
        let page = low.checked_sub(1)?;
        Some((page, self.entry(storage, page)? == at))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quireink_sdk::MemoryStorage;

    #[test]
    fn appends_locates_and_completes() {
        let storage = MemoryStorage::new();
        let path = String::from("index.bin");
        let mut index = PageIndex::open(&storage, path.clone());
        assert_eq!(index.pages(), 0);
        assert_eq!(index.locate(&storage, Locator::new(0, 5)), None);

        let starts = [
            Locator::new(0, 0),
            Locator::new(0, 100),
            Locator::new(0, 250),
        ];
        assert!(index.append(&storage, &starts));
        assert_eq!(
            index.locate(&storage, Locator::new(0, 100)),
            Some((1, true))
        );
        assert_eq!(
            index.locate(&storage, Locator::new(0, 120)),
            Some((1, false))
        );
        assert_eq!(
            index.locate(&storage, Locator::new(0, 900)),
            Some((2, false))
        );
        assert_eq!(index.last(&storage), Some(Locator::new(0, 250)));

        index.finish(&storage);
        let reopened = PageIndex::open(&storage, path);
        assert!(reopened.is_complete());
        assert_eq!(reopened.pages(), 3);
    }
}
