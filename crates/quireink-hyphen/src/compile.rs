//! Compiling TeX hyphenation patterns into a table.

use alloc::string::String;
use alloc::vec::Vec;

use crate::{BLOCK, HEADER, MAGIC};

/// Compiles patterns (whitespace-separated, TeX syntax such as `hy3ph`) and
/// exceptions (words such as `ta-ble`) into a table for
/// [`Patterns`](crate::Patterns). `left_min` and `right_min` are the fewest
/// characters to keep before and after a hyphen. Lines starting with `%`
/// are comments.
pub fn compile(patterns: &str, exceptions: &str, left_min: u8, right_min: u8) -> Vec<u8> {
    let words = |text: &str| -> Vec<String> {
        text.lines()
            .filter(|line| !line.trim_start().starts_with('%'))
            .flat_map(str::split_whitespace)
            .map(String::from)
            .collect()
    };

    // (key, points) with one point per gap, including both ends.
    let mut entries: Vec<(String, Vec<u8>)> = words(patterns)
        .iter()
        .map(|pattern| {
            let mut key = String::new();
            let mut points = alloc::vec![0u8];
            for c in pattern.chars() {
                match c.to_digit(10) {
                    Some(d) => *points.last_mut().unwrap_or(&mut 0) = d as u8,
                    None => {
                        key.extend(c.to_lowercase());
                        points.push(0);
                    }
                }
            }
            (key, points)
        })
        .filter(|(key, _)| !key.is_empty() && key.len() < 256)
        .collect();
    entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    entries.dedup_by(|later, earlier| later.0 == earlier.0);
    let max_len = entries
        .iter()
        .map(|(key, _)| key.chars().count())
        .max()
        .unwrap_or(0)
        .min(255) as u8;

    let blocks = entries.len().div_ceil(BLOCK);
    let mut index = Vec::with_capacity(blocks * 4);
    let mut body = Vec::new();
    for (i, (key, points)) in entries.iter().enumerate() {
        if i % BLOCK == 0 {
            index.extend_from_slice(&(body.len() as u32).to_le_bytes());
        }
        body.push(key.len() as u8);
        body.extend_from_slice(key.as_bytes());
        for pair in points.chunks(2) {
            body.push(pair[0] << 4 | pair.get(1).copied().unwrap_or(0));
        }
    }

    let exceptions: Vec<String> = words(exceptions)
        .into_iter()
        .map(|w| w.to_lowercase())
        .filter(|w| w.len() < 256)
        .collect();
    let exceptions_at = HEADER + index.len() + body.len();

    let mut out = Vec::with_capacity(exceptions_at + 64);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&[left_min, right_min, max_len]);
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    out.extend_from_slice(&(blocks as u32).to_le_bytes());
    out.extend_from_slice(&(exceptions_at as u32).to_le_bytes());
    out.extend_from_slice(&(exceptions.len() as u32).to_le_bytes());
    out.extend_from_slice(&index);
    out.extend_from_slice(&body);
    for word in &exceptions {
        out.push(word.len() as u8);
        out.extend_from_slice(word.as_bytes());
    }
    out
}
