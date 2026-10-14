//! Font obfuscation: many EPUBs scramble the start of their embedded fonts
//! so they cannot simply be copied out (not DRM: the key is the book's own
//! identifier). `META-INF/encryption.xml` names the fonts and the method:
//!
//! - IDPF: the first 1040 bytes XOR the SHA-1 of the unique identifier
//!   (whitespace removed).
//! - Adobe: the first 1024 bytes XOR the 16 bytes of the identifier's
//!   UUID (`urn:uuid:…`).

use alloc::string::String;
use alloc::vec::Vec;

/// How a font is obfuscated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Method {
    None,
    Idpf,
    Adobe,
}

impl Method {
    /// The method of an `EncryptionMethod` algorithm URI.
    pub fn from_algorithm(uri: &str) -> Method {
        match uri.trim() {
            "http://www.idpf.org/2008/embedding" => Method::Idpf,
            "http://ns.adobe.com/pdf/enc#RC" => Method::Adobe,
            _ => Method::None,
        }
    }

    pub fn code(self) -> u8 {
        match self {
            Method::None => 0,
            Method::Idpf => 1,
            Method::Adobe => 2,
        }
    }

    pub fn from_code(code: u8) -> Method {
        match code {
            1 => Method::Idpf,
            2 => Method::Adobe,
            _ => Method::None,
        }
    }
}

/// Undoes `method` on font `data`, with the book's unique `identifier`.
pub(crate) fn reveal(data: &mut [u8], method: Method, identifier: &str) {
    let (key, length): (Vec<u8>, usize) = match method {
        Method::None => return,
        Method::Idpf => {
            let id: String = identifier.chars().filter(|c| !c.is_whitespace()).collect();
            (sha1(id.as_bytes()).to_vec(), 1040)
        }
        Method::Adobe => match uuid_bytes(identifier) {
            Some(key) => (key.to_vec(), 1024),
            None => return,
        },
    };
    for (i, byte) in data.iter_mut().take(length).enumerate() {
        *byte ^= key[i % key.len()];
    }
}

/// The 16 bytes of a UUID in `identifier` (e.g. `urn:uuid:…`).
fn uuid_bytes(identifier: &str) -> Option<[u8; 16]> {
    let hex: Vec<u8> = identifier
        .trim()
        .trim_start_matches("urn:")
        .trim_start_matches("uuid:")
        .bytes()
        .filter(|b| *b != b'-')
        .collect();
    if hex.len() != 32 {
        return None;
    }
    let digit = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
    let mut out = [0u8; 16];
    for (i, pair) in hex.chunks(2).enumerate() {
        out[i] = digit(pair[0])? << 4 | digit(pair[1])?;
    }
    Some(out)
}

/// SHA-1 (FIPS 180-4), only for deriving the IDPF key.
fn sha1(message: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [
        0x6745_2301,
        0xEFCD_AB89,
        0x98BA_DCFE,
        0x1032_5476,
        0xC3D2_E1F0,
    ];
    let mut data = message.to_vec();
    let bits = (message.len() as u64) * 8;
    data.push(0x80);
    while data.len() % 64 != 56 {
        data.push(0);
    }
    data.extend_from_slice(&bits.to_be_bytes());
    for block in data.chunks(64) {
        let mut w = [0u32; 80];
        for (i, word) in block.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, &word) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let t = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        for (state, value) in h.iter_mut().zip([a, b, c, d, e]) {
            *state = state.wrapping_add(value);
        }
    }
    let mut out = [0u8; 20];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha1_matches_known_digests() {
        let hex = |d: [u8; 20]| -> String { d.iter().map(|b| alloc::format!("{b:02x}")).collect() };
        assert_eq!(
            hex(sha1(b"abc")),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(hex(sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        let long = [b'a'; 1000];
        assert_eq!(hex(sha1(&long)), "291e9a6c66994949b57ba5e650361e98fc36b1ba");
    }

    #[test]
    fn reveal_undoes_both_methods() {
        let original: Vec<u8> = (0..2000).map(|i| (i % 251) as u8).collect();
        for (method, id) in [
            (Method::Idpf, "urn:isbn: 978 0 00 000000 0"),
            (
                Method::Adobe,
                "urn:uuid:12345678-9abc-def0-1234-56789abcdef0",
            ),
        ] {
            let mut data = original.clone();
            reveal(&mut data, method, id);
            assert_ne!(data, original);
            // XOR twice is the original again.
            reveal(&mut data, method, id);
            assert_eq!(data, original);
        }
        let mut data = original.clone();
        reveal(&mut data, Method::Adobe, "not a uuid");
        assert_eq!(data, original);
    }
}
