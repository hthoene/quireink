//! QR codes, e.g. to show an address a phone can open: text in byte mode
//! with error correction level M, versions 1 to 10 (up to 213 bytes).
//!
//! Follows ISO/IEC 18004: finder, timing and alignment patterns, format
//! and version information, Reed–Solomon error correction over GF(256),
//! interleaved blocks and the mask with the lowest penalty.

use alloc::vec;
use alloc::vec::Vec;

/// Error correction codewords per block and blocks, for level M, by
/// version (index 0 unused).
const ECC_PER_BLOCK: [usize; 11] = [0, 10, 16, 26, 18, 24, 16, 18, 22, 22, 26];
const BLOCKS: [usize; 11] = [0, 1, 1, 1, 2, 2, 4, 4, 4, 5, 5];
const MAX_VERSION: u8 = 10;

/// A QR code: a square of dark and light modules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QrCode {
    size: usize,
    modules: Vec<bool>,
}

impl QrCode {
    /// The code for `data`, in the smallest version it fits; `None` if
    /// it is longer than version 10 holds.
    pub fn new(data: &[u8]) -> Option<QrCode> {
        let version = (1..=MAX_VERSION).find(|&v| data.len() <= capacity(v))?;
        Some(Self::encode(data, version, None))
    }

    /// The code in `version` with mask `mask` (or the best one).
    fn encode(data: &[u8], version: u8, mask: Option<u8>) -> QrCode {
        let size = usize::from(version) * 4 + 17;
        let mut code = Builder {
            size,
            modules: vec![false; size * size],
            function: vec![false; size * size],
        };
        code.function_patterns(version);
        let codewords = codewords(data, version);
        code.place(&codewords);
        let mask = mask.unwrap_or_else(|| {
            (0..8)
                .min_by_key(|&m| {
                    let mut trial = code.clone();
                    trial.mask(m);
                    trial.format(m);
                    trial.penalty()
                })
                .unwrap_or(0)
        });
        code.mask(mask);
        code.format(mask);
        QrCode {
            size,
            modules: code.modules,
        }
    }

    /// Modules per side.
    pub fn size(&self) -> usize {
        self.size
    }

    /// Whether the module at column `x`, row `y` is dark.
    pub fn dark(&self, x: usize, y: usize) -> bool {
        x < self.size && y < self.size && self.modules[y * self.size + x]
    }
}

/// Bytes a version holds in byte mode at level M.
fn capacity(version: u8) -> usize {
    let bits = data_codewords(version) * 8;
    let count_bits = if version < 10 { 8 } else { 16 };
    (bits - 4 - count_bits) / 8
}

/// Codewords of a version: all of them, then those for data.
fn raw_codewords(version: u8) -> usize {
    let v = usize::from(version);
    let mut bits = (16 * v + 128) * v + 64;
    if v >= 2 {
        let align = v / 7 + 2;
        bits -= (25 * align - 10) * align - 55;
        if v >= 7 {
            bits -= 36;
        }
    }
    bits / 8
}

fn data_codewords(version: u8) -> usize {
    let v = usize::from(version);
    raw_codewords(version) - ECC_PER_BLOCK[v] * BLOCKS[v]
}

/// The data bits and padding, split into blocks with their error
/// correction, interleaved.
fn codewords(data: &[u8], version: u8) -> Vec<u8> {
    let mut bits: Vec<bool> = Vec::new();
    fn push(bits: &mut Vec<bool>, value: u32, count: u32) {
        for i in (0..count).rev() {
            bits.push((value >> i) & 1 == 1);
        }
    }
    // Byte mode, the length, the bytes.
    push(&mut bits, 0b0100, 4);
    push(
        &mut bits,
        data.len() as u32,
        if version < 10 { 8 } else { 16 },
    );
    for &b in data {
        push(&mut bits, u32::from(b), 8);
    }
    let capacity = data_codewords(version) * 8;
    // Terminator, then to a byte, then alternate pad bytes.
    let terminator = (capacity - bits.len()).min(4) as u32;
    push(&mut bits, 0, terminator);
    let to_byte = ((8 - bits.len() % 8) % 8) as u32;
    push(&mut bits, 0, to_byte);
    let mut pad = [0xECu32, 0x11].into_iter().cycle();
    while bits.len() < capacity {
        push(&mut bits, pad.next().unwrap_or(0xEC), 8);
    }
    let bytes: Vec<u8> = bits
        .chunks(8)
        .map(|c| c.iter().fold(0u8, |acc, &b| (acc << 1) | u8::from(b)))
        .collect();

    let v = usize::from(version);
    let blocks = BLOCKS[v];
    let ecc_len = ECC_PER_BLOCK[v];
    let raw = raw_codewords(version);
    let short_blocks = blocks - raw % blocks;
    let short_len = raw / blocks - ecc_len;
    let divisor = rs_divisor(ecc_len);
    let mut data_blocks: Vec<&[u8]> = Vec::new();
    let mut ecc_blocks: Vec<Vec<u8>> = Vec::new();
    let mut at = 0;
    for i in 0..blocks {
        let len = short_len + usize::from(i >= short_blocks);
        let block = &bytes[at..at + len];
        at += len;
        ecc_blocks.push(rs_remainder(block, &divisor));
        data_blocks.push(block);
    }
    let mut out = Vec::with_capacity(raw);
    for i in 0..=short_len {
        for block in &data_blocks {
            if let Some(&b) = block.get(i) {
                out.push(b);
            }
        }
    }
    for i in 0..ecc_len {
        for ecc in &ecc_blocks {
            out.push(ecc[i]);
        }
    }
    out
}

/// Multiplies in GF(256) modulo x⁸ + x⁴ + x³ + x² + 1.
fn gf_mul(x: u8, y: u8) -> u8 {
    let mut z: u32 = 0;
    for i in (0..8).rev() {
        z = (z << 1) ^ ((z >> 7) * 0x11D);
        z ^= u32::from((y >> i) & 1) * u32::from(x);
    }
    z as u8
}

/// The Reed–Solomon generator polynomial of `degree` (highest term
/// first, the leading 1 left out).
fn rs_divisor(degree: usize) -> Vec<u8> {
    let mut result = vec![0u8; degree];
    result[degree - 1] = 1;
    let mut root = 1u8;
    for _ in 0..degree {
        for j in 0..degree {
            result[j] = gf_mul(result[j], root);
            if j + 1 < degree {
                result[j] ^= result[j + 1];
            }
        }
        root = gf_mul(root, 0x02);
    }
    result
}

fn rs_remainder(data: &[u8], divisor: &[u8]) -> Vec<u8> {
    let mut result = vec![0u8; divisor.len()];
    for &b in data {
        let factor = b ^ result[0];
        result.remove(0);
        result.push(0);
        for (r, &d) in result.iter_mut().zip(divisor) {
            *r ^= gf_mul(d, factor);
        }
    }
    result
}

#[derive(Clone)]
struct Builder {
    size: usize,
    modules: Vec<bool>,
    /// Modules of the patterns, which data and masks leave alone.
    function: Vec<bool>,
}

impl Builder {
    fn set(&mut self, x: usize, y: usize, dark: bool) {
        let i = y * self.size + x;
        self.modules[i] = dark;
        self.function[i] = true;
    }

    fn function_patterns(&mut self, version: u8) {
        let size = self.size;
        for i in 0..size {
            self.set(6, i, i % 2 == 0);
            self.set(i, 6, i % 2 == 0);
        }
        for (x, y) in [(3, 3), (size - 4, 3), (3, size - 4)] {
            for dy in -4i32..=4 {
                for dx in -4i32..=4 {
                    let (xx, yy) = (x as i32 + dx, y as i32 + dy);
                    if (0..size as i32).contains(&xx) && (0..size as i32).contains(&yy) {
                        let distance = dx.abs().max(dy.abs());
                        self.set(xx as usize, yy as usize, distance != 2 && distance != 4);
                    }
                }
            }
        }
        let positions = alignment_positions(version, size);
        let last = positions.len().saturating_sub(1);
        for (i, &x) in positions.iter().enumerate() {
            for (j, &y) in positions.iter().enumerate() {
                // Not over the finder patterns.
                if (i == 0 && j == 0) || (i == 0 && j == last) || (i == last && j == 0) {
                    continue;
                }
                for dy in -2i32..=2 {
                    for dx in -2i32..=2 {
                        let distance = dx.abs().max(dy.abs());
                        self.set(
                            (x as i32 + dx) as usize,
                            (y as i32 + dy) as usize,
                            distance != 1,
                        );
                    }
                }
            }
        }
        // Reserved for the format bits until the mask is known.
        self.format(0);
        if version >= 7 {
            let mut rem = u32::from(version);
            for _ in 0..12 {
                rem = (rem << 1) ^ ((rem >> 11) * 0x1F25);
            }
            let bits = (u32::from(version) << 12) | rem;
            for i in 0..18 {
                let dark = (bits >> i) & 1 == 1;
                let (a, b) = (size - 11 + i % 3, i / 3);
                self.set(a, b, dark);
                self.set(b, a, dark);
            }
        }
    }

    /// The format bits: level M (00) and the mask, twice.
    fn format(&mut self, mask: u8) {
        let data = u32::from(mask);
        let mut rem = data;
        for _ in 0..10 {
            rem = (rem << 1) ^ ((rem >> 9) * 0x537);
        }
        let bits = ((data << 10) | rem) ^ 0x5412;
        let bit = |i: u32| (bits >> i) & 1 == 1;
        let size = self.size;
        for i in 0..=5 {
            self.set(8, i, bit(i as u32));
        }
        self.set(8, 7, bit(6));
        self.set(8, 8, bit(7));
        self.set(7, 8, bit(8));
        for i in 9..15 {
            self.set(14 - i, 8, bit(i as u32));
        }
        for i in 0..8 {
            self.set(size - 1 - i, 8, bit(i as u32));
        }
        for i in 8..15 {
            self.set(8, size - 15 + i, bit(i as u32));
        }
        // Always dark.
        self.set(8, size - 8, true);
    }

    /// The codewords' bits in the zigzag order, upwards and downwards in
    /// columns of two from the right.
    fn place(&mut self, codewords: &[u8]) {
        let size = self.size;
        let total = codewords.len() * 8;
        let mut i = 0;
        let mut right = size as i32 - 1;
        while right >= 1 {
            if right == 6 {
                right = 5;
            }
            for vert in 0..size {
                for j in 0..2 {
                    let x = (right - j) as usize;
                    let upward = (right + 1) & 2 == 0;
                    let y = if upward { size - 1 - vert } else { vert };
                    if !self.function[y * size + x] && i < total {
                        let dark = (codewords[i >> 3] >> (7 - (i & 7))) & 1 == 1;
                        self.modules[y * size + x] = dark;
                        i += 1;
                    }
                }
            }
            right -= 2;
        }
    }

    fn mask(&mut self, mask: u8) {
        let size = self.size;
        for y in 0..size {
            for x in 0..size {
                let invert = match mask {
                    0 => (x + y) % 2 == 0,
                    1 => y % 2 == 0,
                    2 => x % 3 == 0,
                    3 => (x + y) % 3 == 0,
                    4 => (x / 3 + y / 2) % 2 == 0,
                    5 => x * y % 2 + x * y % 3 == 0,
                    6 => (x * y % 2 + x * y % 3) % 2 == 0,
                    _ => ((x + y) % 2 + x * y % 3) % 2 == 0,
                };
                let i = y * size + x;
                if invert && !self.function[i] {
                    self.modules[i] = !self.modules[i];
                }
            }
        }
    }

    /// How hard the code is to read (runs, blocks, finder-like patterns
    /// and dark balance); the mask with the least is used.
    fn penalty(&self) -> u32 {
        let size = self.size;
        let at = |x: usize, y: usize| self.modules[y * size + x];
        let mut score = 0u32;
        let line = |get: &dyn Fn(usize) -> bool| -> u32 {
            let mut score = 0;
            let mut run = 1;
            for i in 1..size {
                if get(i) == get(i - 1) {
                    run += 1;
                } else {
                    if run >= 5 {
                        score += run - 2;
                    }
                    run = 1;
                }
            }
            if run >= 5 {
                score += run - 2;
            }
            // 1:1:3:1:1 with four light modules beside it.
            let pattern = [true, false, true, true, true, false, true];
            for i in 0..size.saturating_sub(6) {
                if (0..7).all(|k| get(i + k) == pattern[k]) {
                    let light = |from: usize, to: usize| (from..to).all(|k| k >= size || !get(k));
                    let before = i >= 4 && (i - 4..i).all(|k| !get(k));
                    if before || light(i + 7, i + 11) {
                        score += 40;
                    }
                }
            }
            score
        };
        for y in 0..size {
            score += line(&|x| at(x, y));
        }
        for x in 0..size {
            score += line(&|y| at(x, y));
        }
        for y in 0..size - 1 {
            for x in 0..size - 1 {
                let c = at(x, y);
                if c == at(x + 1, y) && c == at(x, y + 1) && c == at(x + 1, y + 1) {
                    score += 3;
                }
            }
        }
        let dark = self.modules.iter().filter(|&&d| d).count() as i64;
        let total = (size * size) as i64;
        let k = ((dark * 20 - total * 10).abs() + total - 1) / total - 1;
        score + 10 * k.max(0) as u32
    }
}

fn alignment_positions(version: u8, size: usize) -> Vec<usize> {
    if version == 1 {
        return Vec::new();
    }
    let v = usize::from(version);
    let count = v / 7 + 2;
    let step = (v * 4 + count * 2 + 1) / (count * 2 - 2) * 2;
    let mut positions = vec![0usize; count];
    positions[0] = 6;
    let mut position = size - 7;
    for i in (1..count).rev() {
        positions[i] = position;
        position -= step;
    }
    positions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacities_match_the_standard() {
        // Byte mode, level M.
        let expected = [14, 26, 42, 62, 84, 106, 122, 152, 180, 213];
        for (v, &bytes) in (1..=10).zip(expected.iter()) {
            assert_eq!(capacity(v), bytes, "version {v}");
        }
        assert_eq!(raw_codewords(1), 26);
        assert_eq!(raw_codewords(7), 196);
        assert_eq!(alignment_positions(7, 45), [6, 22, 38]);
    }

    #[test]
    fn reed_solomon_matches_a_known_block() {
        // "HELLO WORLD" at version 1-M, from the standard's example.
        let data = [
            0x20, 0x5B, 0x0B, 0x78, 0xD1, 0x72, 0xDC, 0x4D, 0x43, 0x40, 0xEC, 0x11, 0xEC, 0x11,
            0xEC, 0x11,
        ];
        let ecc = rs_remainder(&data, &rs_divisor(10));
        assert_eq!(
            ecc,
            [0xC4, 0x23, 0x27, 0x77, 0xEB, 0xD7, 0xE7, 0xE2, 0x5D, 0x17]
        );
    }

    #[test]
    fn codes_have_their_patterns() {
        let code = QrCode::new(b"http://192.168.178.42:8080/?t=7F3A9C").unwrap();
        // 36 bytes: version 3.
        assert_eq!(code.size(), 29);
        // Finder patterns: dark corners and rings.
        for (x, y) in [(0, 0), (28, 0), (0, 28), (3, 3), (25, 3)] {
            assert!(code.dark(x, y), "({x}, {y})");
        }
        assert!(!code.dark(1, 1) && !code.dark(7, 7));
        // The dark module beside the lower finder.
        assert!(code.dark(8, 29 - 8));
        assert!(QrCode::new(&[b'a'; 214]).is_none());
    }
}
