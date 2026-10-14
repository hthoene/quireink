//! Any bytes are a plain text file of some sort: random data in various
//! shapes must be read block by block without panics or endless loops.

use quireink_plugin_format_txt::TxtDocument;
use quireink_sdk::MemoryStorage;
use quireink_sdk::document::{Document, Locator};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

const PIECES: [&[u8]; 10] = [
    b"\r\n",
    b"\n\n",
    b"\xef\xbb\xbf",
    b"\xff\xfe",
    "Größe ".as_bytes(),
    b"\xe4\xf6\xfc ",
    b"\t",
    b"CHAPTER I\n\n",
    b"\x00",
    b"\xc3",
];

#[test]
fn any_bytes_read_cleanly() {
    for seed in 1..=300u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15));
        let mut data = Vec::new();
        for _ in 0..rng.below(3000) {
            match rng.below(3) {
                0 => data.push(rng.next() as u8),
                1 => data.extend(std::iter::repeat_n(b'w', rng.below(9000))),
                _ => data.extend_from_slice(PIECES[rng.below(PIECES.len())]),
            }
        }
        let storage = MemoryStorage::new().with("book.txt", data);
        let Ok(doc) = TxtDocument::open(&storage, "book.txt") else {
            continue;
        };
        let mut at = Locator::new(0, 0);
        let mut blocks = 0;
        while let Ok(Some(block)) = doc.block_at(&storage, at) {
            assert!(block.end > at.offset, "seed {seed}: no progress at {at:?}");
            let _ = (block.source_offset(block.text.len() / 2), doc.progress(at));
            at = Locator::new(0, block.end);
            blocks += 1;
            assert!(blocks < 100_000, "seed {seed}: too many blocks");
        }
    }
}
