//! MD5, for what KOReader asks for: the key derived from a password
//! (progress sync) and the fingerprint of a book file, by which KOReader
//! and QuireInk recognise a book on every device (not for security).

use alloc::string::String;

use quireink_platform::Storage;

const SHIFTS: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9,
    14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15,
    21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
];

const CONSTANTS: [u32; 64] = [
    0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501,
    0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821,
    0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8,
    0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed, 0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a,
    0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70,
    0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665,
    0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
    0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391,
];

/// An MD5 digest computed piece by piece.
#[derive(Debug, Clone)]
pub struct Md5 {
    state: [u32; 4],
    block: [u8; 64],
    used: usize,
    length: u64,
}

impl Default for Md5 {
    fn default() -> Self {
        Self::new()
    }
}

impl Md5 {
    pub fn new() -> Self {
        Self {
            state: [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476],
            block: [0; 64],
            used: 0,
            length: 0,
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.length += data.len() as u64;
        while !data.is_empty() {
            let take = (64 - self.used).min(data.len());
            self.block[self.used..self.used + take].copy_from_slice(&data[..take]);
            self.used += take;
            data = &data[take..];
            if self.used == 64 {
                let block = self.block;
                self.compress(&block);
                self.used = 0;
            }
        }
    }

    /// The digest as 32 lower-case hex digits.
    pub fn hex(mut self) -> String {
        let bits = self.length.wrapping_mul(8);
        self.update(&[0x80]);
        while self.used != 56 {
            self.update(&[0]);
        }
        self.update(&bits.to_le_bytes());
        let mut out = String::new();
        for word in self.state {
            for byte in word.to_le_bytes() {
                out.push_str(&alloc::format!("{byte:02x}"));
            }
        }
        out
    }

    fn compress(&mut self, block: &[u8; 64]) {
        let mut m = [0u32; 16];
        for (i, word) in block.chunks(4).enumerate() {
            m[i] = u32::from_le_bytes([word[0], word[1], word[2], word[3]]);
        }
        let [mut a, mut b, mut c, mut d] = self.state;
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let rotated = a
                .wrapping_add(f)
                .wrapping_add(CONSTANTS[i])
                .wrapping_add(m[g])
                .rotate_left(SHIFTS[i]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(rotated);
        }
        for (state, value) in self.state.iter_mut().zip([a, b, c, d]) {
            *state = state.wrapping_add(value);
        }
    }
}

/// The MD5 of `text` as hex (KOReader's password key).
pub fn of(text: &str) -> String {
    let mut md5 = Md5::new();
    md5.update(text.as_bytes());
    md5.hex()
}

/// KOReader's fingerprint of a book file ("partial MD5"): 1 KiB at
/// offsets 0, 1 KiB, 4 KiB, 16 KiB, … (×4) up to 1 GiB, as far as the file
/// reaches. The same file gets the same id on every device.
pub fn partial(storage: &dyn Storage, path: &str) -> Option<String> {
    let mut md5 = Md5::new();
    let mut buffer = [0u8; 1024];
    for i in -1i32..=10 {
        let offset: u64 = if i < 0 { 0 } else { 1024 << (2 * i) };
        let n = storage.read_at(path, offset, &mut buffer).ok()?;
        if n == 0 {
            break;
        }
        md5.update(&buffer[..n]);
    }
    Some(md5.hex())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemoryStorage;

    #[test]
    fn known_digests() {
        assert_eq!(of(""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(of("abc"), "900150983cd24fb0d6963f7d28e17f72");
        let long: String = core::iter::repeat_n('a', 1000).collect();
        assert_eq!(of(&long), "cabe45dcc9ae5b66ba86600cca6b8ba8");
    }

    #[test]
    fn partial_digest_samples_the_file() {
        // 5000 bytes: samples at 0, 1024 and 4096 (the last one short).
        let data: alloc::vec::Vec<u8> = (0..5000u32).map(|i| (i % 251) as u8).collect();
        let storage = MemoryStorage::new().with("b.epub", data.clone());
        let mut expected = Md5::new();
        expected.update(&data[0..1024]);
        expected.update(&data[1024..2048]);
        expected.update(&data[4096..5000]);
        assert_eq!(partial(&storage, "b.epub"), Some(expected.hex()));
    }
}
