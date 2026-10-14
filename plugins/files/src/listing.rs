//! Sorted, paged directory listings that never hold more than a page.
//!
//! Folders with thousands of books must work on a device with a few
//! hundred kilobytes of RAM, so a listing is never loaded completely.
//! Pages are found by *keyset pagination*: one pass over the directory
//! keeps only the `n` entries that follow (or precede) a known entry in
//! sort order. Each step costs a directory scan but only `n` entries of
//! memory.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cmp::Ordering;

use quireink_sdk::{Formats, Storage};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Entry {
    pub name: String,
    pub is_dir: bool,
}

/// Folders first, then names regardless of case, then exact names (so the
/// order is total).
fn order(a_dir: bool, a: &str, b_dir: bool, b: &str) -> Ordering {
    b_dir
        .cmp(&a_dir)
        .then_with(|| {
            a.chars()
                .flat_map(char::to_lowercase)
                .cmp(b.chars().flat_map(char::to_lowercase))
        })
        .then_with(|| a.cmp(b))
}

impl Entry {
    fn cmp_to(&self, is_dir: bool, name: &str) -> Ordering {
        order(self.is_dir, &self.name, is_dir, name)
    }
}

/// The visible entries of a directory: folders and files a format plugin
/// can open (or only folders); hidden names (starting with `.`) are
/// skipped.
pub(crate) struct Listing<'a> {
    pub storage: &'a dyn Storage,
    pub formats: &'a Formats,
    pub dir: &'a str,
    pub folders_only: bool,
}

impl Listing<'_> {
    fn each(&self, f: &mut dyn FnMut(bool, &str)) {
        let _ = self.storage.list(self.dir, &mut |entry| {
            let shown = entry.is_dir || (!self.folders_only && self.formats.supports(entry.name));
            if shown && !entry.name.starts_with('.') {
                f(entry.is_dir, entry.name);
            }
        });
    }

    /// `n` entries from index `start` on, holding at most `n` entries at a
    /// time while skipping to `start`.
    pub fn range(&self, start: usize, n: usize) -> Vec<Entry> {
        let mut last: Option<Entry> = None;
        let mut skip = start;
        while skip > 0 {
            let step = skip.min(n.max(1));
            let chunk = self.after(last.as_ref(), step);
            if chunk.len() < step {
                return Vec::new();
            }
            last = chunk.into_iter().last();
            skip -= step;
        }
        self.after(last.as_ref(), n)
    }

    pub fn count(&self) -> usize {
        let mut count = 0;
        self.each(&mut |_, _| count += 1);
        count
    }

    /// Number of entries sorted before `entry`.
    pub fn rank(&self, entry: &Entry) -> usize {
        let mut rank = 0;
        self.each(&mut |is_dir, name| {
            if entry.cmp_to(is_dir, name) == Ordering::Greater {
                rank += 1;
            }
        });
        rank
    }

    /// The first `n` entries after `after` (from the start if `None`), in
    /// order.
    pub fn after(&self, after: Option<&Entry>, n: usize) -> Vec<Entry> {
        let mut page: Vec<Entry> = Vec::with_capacity(n);
        self.each(&mut |is_dir, name| {
            if n == 0 || after.is_some_and(|a| a.cmp_to(is_dir, name) != Ordering::Less) {
                return;
            }
            if page.len() == n && page[n - 1].cmp_to(is_dir, name) == Ordering::Less {
                return;
            }
            let at = page.partition_point(|e| e.cmp_to(is_dir, name) == Ordering::Less);
            page.insert(
                at,
                Entry {
                    name: name.to_string(),
                    is_dir,
                },
            );
            page.truncate(n);
        });
        page
    }

    /// The last `n` entries before `before` (up to the end if `None`), in
    /// order.
    pub fn before(&self, before: Option<&Entry>, n: usize) -> Vec<Entry> {
        let mut page: Vec<Entry> = Vec::with_capacity(n + 1);
        self.each(&mut |is_dir, name| {
            if n == 0 || before.is_some_and(|b| b.cmp_to(is_dir, name) != Ordering::Greater) {
                return;
            }
            if page.len() == n && page[0].cmp_to(is_dir, name) == Ordering::Greater {
                return;
            }
            let at = page.partition_point(|e| e.cmp_to(is_dir, name) == Ordering::Less);
            page.insert(
                at,
                Entry {
                    name: name.to_string(),
                    is_dir,
                },
            );
            if page.len() > n {
                page.remove(0);
            }
        });
        page
    }

    /// Page number `page` of `rows` entries, walking from the nearer end.
    pub fn page(&self, page: usize, rows: usize, len: usize) -> Vec<Entry> {
        let pages = len.div_ceil(rows.max(1));
        if page * 2 < pages {
            let mut entries = self.after(None, rows);
            for _ in 0..page {
                entries = self.after(entries.last(), rows);
            }
            entries
        } else {
            let last = len - (pages - 1) * rows;
            let mut entries = self.before(None, last);
            for _ in page + 1..pages {
                entries = self.before(entries.first(), rows);
            }
            entries
        }
    }

    /// The page of `rows` entries that contains `entry` (or where it would
    /// be), and the index of its first entry.
    pub fn page_of(&self, entry: &Entry, rows: usize) -> (usize, Vec<Entry>) {
        let rank = self.rank(entry);
        let mut entries = self.before(Some(entry), rank % rows);
        let rest = self.after(entries.last(), rows - entries.len());
        entries.extend(rest);
        (rank - rank % rows, entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;
    use quireink_sdk::{FormatSpec, MemoryStorage};

    fn formats() -> Formats {
        let mut formats = Formats::new();
        formats
            .add(FormatSpec {
                id: quireink_sdk::Id::new_static("test.txt"),
                title: alloc::borrow::Cow::Borrowed("TXT"),
                extensions: alloc::vec![alloc::borrow::Cow::Borrowed("txt")],
                open: alloc::boxed::Box::new(|_, _| {
                    Err(quireink_sdk::document::DocumentError::UnsupportedFormat)
                }),
            })
            .unwrap();
        formats
    }

    fn names(entries: &[Entry]) -> Vec<&str> {
        entries.iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn pages_follow_the_full_sort_order() {
        let mut storage = MemoryStorage::new();
        // Folders first, case-insensitive, unsupported and hidden skipped.
        let mut expected = alloc::vec![String::from("Archiv"), String::from("neu")];
        storage = storage
            .with("books/Archiv/x.txt", "")
            .with("books/neu/y.txt", "");
        for i in (0..23).rev() {
            let name = format!("{}book {i:02}.txt", if i % 2 == 0 { "B" } else { "b" });
            storage = storage.with(&format!("books/{name}"), "");
            expected.push(name);
        }
        storage = storage
            .with("books/cover.jpg", "")
            .with("books/.hidden.txt", "");
        expected[2..].sort_by_key(|n| (n.to_lowercase(), n.clone()));
        let formats = formats();
        let listing = Listing {
            storage: &storage,
            formats: &formats,
            dir: "books",
            folders_only: false,
        };
        let len = listing.count();
        assert_eq!(len, expected.len());
        assert_eq!(
            names(&listing.range(5, 3)),
            expected[5..8]
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
        );
        assert_eq!(names(&listing.range(24, 3)), [expected[24].as_str()]);
        let folders = Listing {
            folders_only: true,
            ..listing
        };
        assert_eq!(names(&folders.after(None, 9)), ["Archiv", "neu"]);

        let rows = 4;
        let pages = len.div_ceil(rows);
        // Forward page by page, then backward.
        let mut page = listing.after(None, rows);
        let mut seen: Vec<String> = Vec::new();
        for p in 0..pages {
            assert_eq!(names(&page), names(&listing.page(p, rows, len)), "page {p}");
            seen.extend(page.iter().map(|e| e.name.clone()));
            page = listing.after(page.last(), rows);
        }
        assert!(page.is_empty());
        assert_eq!(seen, expected);
        let mut page = listing.page(pages - 1, rows, len);
        for p in (0..pages - 1).rev() {
            page = listing.before(page.first(), rows);
            assert_eq!(
                names(&page),
                names(
                    &expected[p * rows..(p + 1) * rows]
                        .iter()
                        .map(|n| Entry {
                            name: n.clone(),
                            is_dir: false
                        })
                        .collect::<Vec<_>>()
                )
            );
        }

        // The page containing an entry, e.g. after going up a folder.
        let target = Entry {
            name: expected[13].clone(),
            is_dir: false,
        };
        let (first, page) = listing.page_of(&target, rows);
        assert_eq!(first, 12);
        assert_eq!(
            names(&page),
            expected[12..16]
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
        );
    }
}
