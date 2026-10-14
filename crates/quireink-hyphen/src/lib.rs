//! Hyphenation with Liang's algorithm (as in TeX), on pattern tables that
//! are read in place.
//!
//! Patterns are compiled ahead of time ([`compile`], e.g. in a build
//! script) into a sorted table with a sparse index, so a device can keep
//! them in flash and look them up by binary search without loading them:
//! hyphenating needs a few hundred bytes of RAM, whatever the size of the
//! pattern set.
//!
//! Table format (little endian):
//!
//! | Field | Size |
//! |-------|------|
//! | magic `QIHP`, version 1 | 5 |
//! | minimum characters before / after a hyphen | 2 |
//! | longest pattern in characters | 1 |
//! | number of patterns, number of index blocks | 4 + 4 |
//! | offset and number of exceptions | 4 + 4 |
//! | index: offset of every [`BLOCK`]th pattern | 4 per block |
//! | patterns: key length, key (UTF-8), points (4 bits each) | |
//! | exceptions: length, word with `-` at the allowed breaks | |

#![no_std]

extern crate alloc;

mod compile;

use alloc::vec::Vec;
use core::cmp::Ordering;
use core::fmt;

pub use compile::compile;

/// Patterns per index entry.
pub const BLOCK: usize = 16;
const MAGIC: &[u8; 5] = b"QIHP\x01";
const HEADER: usize = 24;
/// Longest word that is hyphenated, in characters.
const MAX_WORD: usize = 48;

/// Why a pattern table cannot be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidPatterns;

impl fmt::Display for InvalidPatterns {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("invalid hyphenation patterns")
    }
}

/// A compiled pattern table.
#[derive(Clone, Copy)]
pub struct Patterns<'a> {
    data: &'a [u8],
    left_min: usize,
    right_min: usize,
    max_len: usize,
    blocks: usize,
    exceptions: (usize, usize),
}

impl fmt::Debug for Patterns<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Patterns")
            .field("bytes", &self.data.len())
            .field("left_min", &self.left_min)
            .field("right_min", &self.right_min)
            .finish_non_exhaustive()
    }
}

fn u32_at(data: &[u8], at: usize) -> Option<usize> {
    let bytes = data.get(at..at + 4)?;
    Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize)
}

impl<'a> Patterns<'a> {
    pub fn new(data: &'a [u8]) -> Result<Self, InvalidPatterns> {
        if data.len() < HEADER || &data[..5] != MAGIC {
            return Err(InvalidPatterns);
        }
        let blocks = u32_at(data, 12).ok_or(InvalidPatterns)?;
        let exceptions = (
            u32_at(data, 16).ok_or(InvalidPatterns)?,
            u32_at(data, 20).ok_or(InvalidPatterns)?,
        );
        if HEADER + blocks * 4 > data.len() || exceptions.0 > data.len() {
            return Err(InvalidPatterns);
        }
        Ok(Self {
            data,
            left_min: usize::from(data[5]).max(1),
            right_min: usize::from(data[6]).max(1),
            max_len: usize::from(data[7]),
            blocks,
            exceptions,
        })
    }

    /// Minimum characters before and after a hyphen.
    pub fn minimums(&self) -> (usize, usize) {
        (self.left_min, self.right_min)
    }

    fn entries(&self) -> usize {
        HEADER + self.blocks * 4
    }

    /// The pattern at `at` (relative to the entries): key, points and the
    /// offset of the next pattern.
    fn entry(&self, at: usize) -> Option<(&'a [u8], &'a [u8], usize)> {
        let start = self.entries() + at;
        let len = usize::from(*self.data.get(start)?);
        let key = self.data.get(start + 1..start + 1 + len)?;
        let chars = key.iter().filter(|&&b| b & 0xc0 != 0x80).count();
        let points_len = (chars + 2) / 2;
        let points = self
            .data
            .get(start + 1 + len..start + 1 + len + points_len)?;
        Some((key, points, at + 1 + len + points_len))
    }

    /// The points of the pattern `key`, if there is one.
    fn lookup(&self, key: &[u8]) -> Option<&'a [u8]> {
        let block_key = |b: usize| -> Option<&'a [u8]> {
            let at = u32_at(self.data, HEADER + b * 4)?;
            self.entry(at).map(|(k, ..)| k)
        };
        // The last block whose first key is not greater than `key`.
        let (mut low, mut high) = (0, self.blocks);
        while low < high {
            let mid = (low + high) / 2;
            if block_key(mid)? <= key {
                low = mid + 1;
            } else {
                high = mid;
            }
        }
        let block = low.checked_sub(1)?;
        let mut at = u32_at(self.data, HEADER + block * 4)?;
        for _ in 0..BLOCK {
            let (k, points, next) = self.entry(at)?;
            match k.cmp(key) {
                Ordering::Equal => return Some(points),
                Ordering::Greater => return None,
                Ordering::Less => at = next,
            }
            if self.entries() + at >= self.exceptions.0 {
                return None;
            }
        }
        None
    }

    /// Calls `out` with the byte offset of every position in `word` where
    /// it may be hyphenated, in ascending order. `word` should consist of
    /// letters only; it is matched without regard to case.
    pub fn hyphenate(&self, word: &str, out: &mut dyn FnMut(usize)) {
        let mut chars: Vec<char> = Vec::with_capacity(word.len() + 2);
        chars.push('.');
        for c in word.chars() {
            let mut lower = c.to_lowercase();
            match (lower.next(), lower.next()) {
                (Some(l), None) => chars.push(l),
                // Characters that change length when lowercased are rare;
                // such words are left alone.
                _ => return,
            }
        }
        chars.push('.');
        let n = chars.len() - 2;
        if n < self.left_min + self.right_min || n > MAX_WORD {
            return;
        }
        let offsets: Vec<usize> = word.char_indices().map(|(i, _)| i).collect();
        if let Some(found) = self.exception(&chars[1..=n]) {
            for p in found {
                out(offsets[p]);
            }
            return;
        }

        // values[k]: the highest point between chars[k - 1] and chars[k].
        let mut values = alloc::vec![0u8; chars.len() + 1];
        let mut key = [0u8; 4 * 16];
        for start in 0..chars.len() {
            let mut len = 0;
            for (count, &c) in chars[start..].iter().enumerate().take(self.max_len) {
                if len + c.len_utf8() > key.len() {
                    break;
                }
                len += c.encode_utf8(&mut key[len..]).len();
                let Some(points) = self.lookup(&key[..len]) else {
                    continue;
                };
                for k in 0..=count + 1 {
                    let byte = points.get(k / 2).copied().unwrap_or(0);
                    let point = if k % 2 == 0 { byte >> 4 } else { byte & 0x0f };
                    let slot = &mut values[start + k];
                    *slot = (*slot).max(point);
                }
            }
        }
        // A hyphen before letter p (0-based) sits at values[p + 1].
        for p in self.left_min..=n - self.right_min {
            if values[p + 1] % 2 == 1 {
                out(offsets[p]);
            }
        }
    }

    /// Positions (letter indices) from the exception list, if `word` is in it.
    fn exception(&self, word: &[char]) -> Option<Vec<usize>> {
        let (mut at, count) = self.exceptions;
        for _ in 0..count {
            let len = usize::from(*self.data.get(at)?);
            let entry = core::str::from_utf8(self.data.get(at + 1..at + 1 + len)?).ok()?;
            at += 1 + len;
            let mut letters = entry.chars().filter(|&c| c != '-');
            if entry.chars().filter(|&c| c != '-').count() != word.len()
                || !word.iter().all(|&c| letters.next() == Some(c))
            {
                continue;
            }
            let mut positions = Vec::new();
            let mut index = 0;
            for c in entry.chars() {
                if c == '-' {
                    positions.push(index);
                } else {
                    index += 1;
                }
            }
            return Some(positions);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::String;

    fn hyphenated(patterns: &Patterns<'_>, word: &str) -> String {
        let mut breaks = Vec::new();
        patterns.hyphenate(word, &mut |at| breaks.push(at));
        let mut out = String::new();
        for (i, c) in word.char_indices() {
            if breaks.contains(&i) {
                out.push('-');
            }
            out.push(c);
        }
        out
    }

    #[test]
    fn applies_patterns_like_tex() {
        // Liang's example patterns for "hyphenation".
        let table = compile("hy3ph he2n hena4 hen5at 1na n2at 1tio 2io o2n", "", 2, 3);
        let patterns = Patterns::new(&table).unwrap();
        assert_eq!(hyphenated(&patterns, "hyphenation"), "hy-phen-ation");
        assert_eq!(hyphenated(&patterns, "Hyphenation"), "Hy-phen-ation");
        // Too short for the minimums.
        assert_eq!(hyphenated(&patterns, "hyph"), "hyph");
    }

    #[test]
    fn exceptions_win() {
        let table = compile("1na", "ta-ble", 2, 2);
        let patterns = Patterns::new(&table).unwrap();
        assert_eq!(hyphenated(&patterns, "table"), "ta-ble");
        assert_eq!(hyphenated(&patterns, "banana"), "ba-na-na");
    }

    #[test]
    fn handles_non_ascii_letters() {
        let table = compile("1ü ä1", "", 1, 1);
        let patterns = Patterns::new(&table).unwrap();
        assert_eq!(hyphenated(&patterns, "Mädchenbüro"), "Mä-dchenb-üro");
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(Patterns::new(b"nope").unwrap_err(), InvalidPatterns);
    }
}
