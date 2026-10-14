//! Pattern tables come from plugins: damaged tables and unusual words must
//! not panic.

use quireink_hyphen::{Patterns, compile};

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

const WORDS: [&str; 8] = [
    "Donaudampfschifffahrtsgesellschaftskapitänsmützenabzeichen",
    "ß",
    "İstanbul",
    "a",
    "ÄÖÜäöü",
    "ǅemal",
    "x̧ÿ́",
    "hyphenation",
];

#[test]
fn damaged_tables_and_odd_words() {
    let table = compile(
        "hy3ph he2n hena4 hen5at 1na n2at 1tio 2io o2n 1ß 2ü1",
        "ta-ble",
        2,
        2,
    );
    for seed in 1..=3000u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15));
        let mut data = table.clone();
        for _ in 0..1 + rng.below(4) {
            if data.is_empty() {
                break;
            }
            let at = rng.below(data.len());
            match rng.below(3) {
                0 => data[at] = rng.next() as u8,
                1 => data.truncate(at),
                _ => data[at] ^= 0x80,
            }
        }
        if let Ok(patterns) = Patterns::new(&data) {
            for word in WORDS {
                let mut last = None;
                patterns.hyphenate(word, &mut |at| {
                    assert!(word.is_char_boundary(at) && at > 0 && at < word.len());
                    assert!(last.is_none_or(|l| at > l), "positions ascend");
                    last = Some(at);
                });
            }
        }
    }
}

#[test]
fn compiles_any_text() {
    for seed in 1..=500u64 {
        let mut rng = Rng(seed);
        let text: String = (0..rng.below(500))
            .map(|_| ['a', '1', '9', '.', ' ', '\n', '%', 'ä', '0'][rng.below(9)])
            .collect();
        let table = compile(&text, &text, 1, 1);
        let patterns = Patterns::new(&table).unwrap();
        patterns.hyphenate("aaaaaaa", &mut |_| {});
    }
}
