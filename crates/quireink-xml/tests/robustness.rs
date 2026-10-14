//! The tokenizer must accept any input: random bytes and tag soup, in any
//! chunking, without panicking and with a bounded buffer.

// Tests panic on purpose when something is wrong.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use quireink_xml::{Token, Tokenizer, decode};

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

const PIECES: [&str; 16] = [
    "<",
    ">",
    "</",
    "/>",
    "<p class=\"a\">",
    "</p>",
    "<!--",
    "-->",
    "<![CDATA[",
    "]]>",
    "<?xml ",
    "?>",
    "&amp;",
    "&#x1F4D6;",
    "&#;",
    "\"",
];

fn input(rng: &mut Rng) -> Vec<u8> {
    let mut data = Vec::new();
    for _ in 0..rng.below(400) {
        match rng.below(4) {
            0 => data.push(rng.next() as u8),
            1 => data.extend_from_slice("Wörter ß ".as_bytes()),
            2 => data.extend(std::iter::repeat_n(b'x', rng.below(3000))),
            _ => data.extend_from_slice(PIECES[rng.below(PIECES.len())].as_bytes()),
        }
    }
    data
}

#[test]
fn any_input_is_tokenized() {
    for seed in 1..=500u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15));
        let data = input(&mut rng);
        let chunk = 1 + rng.below(2048);
        let mut tokenizer = Tokenizer::new();
        let mut emit = |token: Token<'_>| match token {
            Token::Text(text) => {
                let _ = decode(&text);
            }
            Token::Start { name, attrs, .. } => {
                let _ = name.len();
                for (key, value) in attrs.iter() {
                    let _ = (key, decode(value));
                }
                let _ = attrs.get("class");
            }
            _ => {}
        };
        for piece in data.chunks(chunk) {
            tokenizer.feed(piece, &mut emit);
        }
        tokenizer.finish(&mut emit);
    }
}
