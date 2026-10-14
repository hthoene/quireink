//! Character sets for bitmap font packs.

use core::ops::RangeInclusive;

/// Latin script for Western, Central and Northern European languages,
/// plus common typographic punctuation.
pub const LATIN: &[RangeInclusive<char>] = &[
    ' '..='~',
    '\u{a0}'..='\u{17f}',    // Latin-1 Supplement, Latin Extended-A
    '\u{2010}'..='\u{2027}', // dashes, quotes, bullets, ellipsis
    '\u{2030}'..='\u{203a}', // per mille, primes, guillemets
    '\u{20ac}'..='\u{20ac}', // euro sign
    '\u{2122}'..='\u{2122}', // trade mark
    '\u{fffd}'..='\u{fffd}', // replacement character
];

/// All characters of `ranges`.
pub fn chars(ranges: &[RangeInclusive<char>]) -> impl Iterator<Item = char> + Clone + '_ {
    ranges.iter().flat_map(|range| range.clone())
}
