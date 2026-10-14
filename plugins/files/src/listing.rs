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
    /// Place in the sort order before names ([`UNRANKED`] for none).
    pub rank: u32,
}

/// The rank of entries ordered by name only.
pub(crate) const UNRANKED: u32 = u32::MAX;

impl Entry {
    pub fn folder(name: &str) -> Self {
        Self {
            name: name.to_string(),
            is_dir: true,
            rank: UNRANKED,
        }
    }

    fn cmp_to(&self, is_dir: bool, rank: u32, name: &str) -> Ordering {
        order((self.is_dir, self.rank, &self.name), (is_dir, rank, name))
    }
}

/// Folders first, then ranked entries by rank, then names regardless of
/// case, then exact names (so the order is total).
fn order(a: (bool, u32, &str), b: (bool, u32, &str)) -> Ordering {
    b.0.cmp(&a.0)
        .then_with(|| a.1.cmp(&b.1))
        .then_with(|| {
            a.2.chars()
                .flat_map(char::to_lowercase)
                .cmp(b.2.chars().flat_map(char::to_lowercase))
        })
        .then_with(|| a.2.cmp(b.2))
}

/// Folders other readers keep next to a book, e.g. KOReader's
/// `Book.sdr` with its reading position: not part of the library.
fn is_sidecar(name: &str) -> bool {
    name.len() > 4 && name[name.len() - 4..].eq_ignore_ascii_case(".sdr")
}

/// The visible entries of a directory: folders and files a format plugin
/// can open (or only folders); hidden names (starting with `.`) and
/// sidecar folders of other readers are skipped.
pub(crate) struct Listing<'a> {
    pub storage: &'a dyn Storage,
    pub formats: &'a Formats,
    pub dir: &'a str,
    pub folders_only: bool,
    /// Ranks files before the name order, e.g. by when they were read.
    pub rank: Option<&'a dyn Fn(&str) -> u32>,
}

impl Listing<'_> {
    fn each(&self, f: &mut dyn FnMut(bool, u32, &str)) {
        let _ = self.storage.list(self.dir, &mut |entry| {
            let shown = if entry.is_dir {
                !is_sidecar(entry.name)
            } else {
                !self.folders_only && self.formats.supports(entry.name)
            };
            if shown && !entry.name.starts_with('.') {
                let rank = match self.rank {
                    Some(rank) if !entry.is_dir => rank(entry.name),
                    _ => UNRANKED,
                };
                f(entry.is_dir, rank, entry.name);
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
        self.each(&mut |_, _, _| count += 1);
        count
    }

    /// Number of entries sorted before `entry`.
    pub fn rank(&self, entry: &Entry) -> usize {
        let mut rank = 0;
        self.each(&mut |is_dir, r, name| {
            if entry.cmp_to(is_dir, r, name) == Ordering::Greater {
                rank += 1;
            }
        });
        rank
    }

    /// The first `n` entries after `after` (from the start if `None`), in
    /// order.
    pub fn after(&self, after: Option<&Entry>, n: usize) -> Vec<Entry> {
        let mut page: Vec<Entry> = Vec::with_capacity(n);
        self.each(&mut |is_dir, rank, name| {
            if n == 0 || after.is_some_and(|a| a.cmp_to(is_dir, rank, name) != Ordering::Less) {
                return;
            }
            if page.len() == n && page[n - 1].cmp_to(is_dir, rank, name) == Ordering::Less {
                return;
            }
            let at = page.partition_point(|e| e.cmp_to(is_dir, rank, name) == Ordering::Less);
            page.insert(
                at,
                Entry {
                    name: name.to_string(),
                    is_dir,
                    rank,
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
        self.each(&mut |is_dir, rank, name| {
            if n == 0 || before.is_some_and(|b| b.cmp_to(is_dir, rank, name) != Ordering::Greater) {
                return;
            }
            if page.len() == n && page[0].cmp_to(is_dir, rank, name) == Ordering::Greater {
                return;
            }
            let at = page.partition_point(|e| e.cmp_to(is_dir, rank, name) == Ordering::Less);
            page.insert(
                at,
                Entry {
                    name: name.to_string(),
                    is_dir,
                    rank,
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
            .with("books/.hidden.txt", "")
            .with("books/Book 01.SDR/metadata.epub.lua", "");
        expected[2..].sort_by_key(|n| (n.to_lowercase(), n.clone()));
        let formats = formats();
        let listing = Listing {
            storage: &storage,
            formats: &formats,
            dir: "books",
            folders_only: false,
            rank: None,
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

        // Ranked files (e.g. recently read) come first, folders still on top.
        let recent = |name: &str| match name {
            "bbook 05.txt" => 0,
            "Bbook 10.txt" => 1,
            _ => UNRANKED,
        };
        let ranked = Listing {
            rank: Some(&recent),
            folders_only: false,
            ..folders
        };
        assert_eq!(
            names(&ranked.after(None, 5)),
            [
                "Archiv",
                "neu",
                "bbook 05.txt",
                "Bbook 10.txt",
                "Bbook 00.txt"
            ]
        );
        let first = ranked.after(None, 3);
        assert_eq!(
            names(&ranked.after(first.last(), 2)),
            ["Bbook 10.txt", "Bbook 00.txt"]
        );

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
                            is_dir: false,
                            rank: UNRANKED,
                        })
                        .collect::<Vec<_>>()
                )
            );
        }

        // The page containing an entry, e.g. after going up a folder.
        let target = Entry {
            name: expected[13].clone(),
            is_dir: false,
            rank: UNRANKED,
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
