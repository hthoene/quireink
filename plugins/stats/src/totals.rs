//! Sums over the log, as KOReader's statistics compute them.

use alloc::vec;
use alloc::vec::Vec;

use quireink_sdk::Storage;

use crate::store::{self, Book, PageStat};

/// What one book's log adds up to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct BookTotals {
    pub seconds: u64,
    /// Distinct pages read, in the book's current pages.
    pub pages_read: u32,
    /// Reading time per local day, in order.
    pub days: Vec<(i32, u32)>,
}

/// The pages of the book's current `pages` a stay on page `page` of
/// `then` pages stands for (KOReader's `page_stat` view rescales alike,
/// so the counts agree after a change of font size).
pub(crate) fn rescaled(page: u32, then: u32, pages: u32) -> core::ops::RangeInclusive<u32> {
    if then == 0 || pages == 0 {
        return page..=page;
    }
    let (page, then, pages) = (u64::from(page.max(1)), u64::from(then), u64::from(pages));
    let first = (page - 1) * pages / then + 1;
    let last = first.max(page * pages / then);
    first as u32..=last as u32
}

/// The totals of `book`; `day_of` turns Unix seconds into local days.
pub(crate) fn of_book(
    storage: &dyn Storage,
    book: &Book,
    day_of: impl Fn(u32) -> i32,
) -> BookTotals {
    let mut totals = BookTotals::default();
    // Pages read, as bits.
    let mut read: Vec<u8> = Vec::new();
    store::scan(storage, |stat: &PageStat| {
        if stat.book != book.id {
            return;
        }
        totals.seconds += u64::from(stat.duration);
        let pages = if book.pages > 0 {
            book.pages
        } else {
            stat.pages
        };
        for page in rescaled(stat.page, stat.pages, pages) {
            let (byte, bit) = ((page / 8) as usize, page % 8);
            if read.len() <= byte {
                read.resize(byte + 1, 0);
            }
            read[byte] |= 1 << bit;
        }
        let day = day_of(stat.start);
        match totals.days.binary_search_by_key(&day, |(d, _)| *d) {
            Ok(i) => totals.days[i].1 += u32::from(stat.duration),
            Err(i) => totals.days.insert(i, (day, u32::from(stat.duration))),
        }
    });
    totals.pages_read = read.iter().map(|b| b.count_ones()).sum();
    totals
}

/// Reading time per book id, for lists.
pub(crate) fn seconds_per_book(storage: &dyn Storage) -> Vec<u64> {
    let mut seconds = vec![0u64; 0];
    store::scan(storage, |stat| {
        let i = usize::from(stat.book);
        if seconds.len() <= i {
            seconds.resize(i + 1, 0);
        }
        seconds[i] += u64::from(stat.duration);
    });
    seconds
}

/// The days in a row up to `today` (or yesterday, while today has no
/// reading yet) that have reading.
pub(crate) fn streak(days: &[store::Day], today: i32) -> u32 {
    let has = |day: i32| days.binary_search_by_key(&day, |d| d.day).is_ok();
    let mut day = if has(today) { today } else { today - 1 };
    let mut count = 0;
    while has(day) {
        count += 1;
        day -= 1;
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Day;
    use quireink_sdk::MemoryStorage;

    #[test]
    fn pages_rescale_like_koreader() {
        // Read in 100 pages, now 200: page 10 stands for 19 and 20.
        assert_eq!(rescaled(10, 100, 200), 19..=20);
        // Read in 200, now 100: pages 19 and 20 both stand for page 10.
        assert_eq!(rescaled(19, 200, 100), 10..=10);
        assert_eq!(rescaled(20, 200, 100), 10..=10);
        assert_eq!(rescaled(5, 100, 100), 5..=5);
    }

    #[test]
    fn books_add_up() {
        let storage = MemoryStorage::new();
        let stat = |page, start, duration, pages| PageStat {
            book: 1,
            page,
            start,
            duration,
            pages,
        };
        store::append(&storage, stat(1, 100, 30, 100));
        store::append(&storage, stat(2, 140, 40, 100));
        // The same page again, the next day.
        store::append(&storage, stat(2, 1000, 20, 100));
        // Another book.
        store::append(
            &storage,
            PageStat {
                book: 2,
                ..stat(9, 1100, 50, 100)
            },
        );
        let book = Book {
            id: 1,
            pages: 100,
            ..Book::default()
        };
        let totals = of_book(&storage, &book, |unix| (unix / 500) as i32);
        assert_eq!(totals.seconds, 90);
        assert_eq!(totals.pages_read, 2);
        assert_eq!(totals.days, [(0, 70), (2, 20)]);
        assert_eq!(seconds_per_book(&storage), [0, 90, 50]);
    }

    #[test]
    fn streaks_count_days_in_a_row() {
        let days: Vec<Day> = [3, 5, 6, 7]
            .into_iter()
            .map(|day| Day {
                day,
                seconds: 60,
                stays: 1,
            })
            .collect();
        assert_eq!(streak(&days, 7), 3);
        // Not read yet today: the streak up to yesterday still counts.
        assert_eq!(streak(&days, 8), 3);
        assert_eq!(streak(&days, 9), 0);
    }
}
