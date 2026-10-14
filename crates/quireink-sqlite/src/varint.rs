//! SQLite's variable-length integers: 1 to 9 bytes, big-endian, seven
//! bits per byte with the high bit set on all but the last; a ninth byte
//! carries eight bits.

/// The value at the start of `data` and its length in bytes; `None` if
/// `data` ends first.
pub(crate) fn read(data: &[u8]) -> Option<(u64, usize)> {
    let mut value: u64 = 0;
    for i in 0..9 {
        let byte = *data.get(i)?;
        if i == 8 {
            return Some(((value << 8) | u64::from(byte), 9));
        }
        value = (value << 7) | u64::from(byte & 0x7f);
        if byte & 0x80 == 0 {
            return Some((value, i + 1));
        }
    }
    None
}

/// Appends `value` to `out`.
pub(crate) fn write(out: &mut alloc::vec::Vec<u8>, value: u64) {
    if value > 0x00ff_ffff_ffff_ffff {
        // Nine bytes: eight with seven bits, the last with eight.
        let mut bytes = [0u8; 9];
        bytes[8] = value as u8;
        let mut v = value >> 8;
        for i in (0..8).rev() {
            bytes[i] = (v as u8 & 0x7f) | 0x80;
            v >>= 7;
        }
        out.extend_from_slice(&bytes);
        return;
    }
    let mut bytes = [0u8; 9];
    let mut n = 0;
    let mut v = value;
    loop {
        bytes[n] = (v & 0x7f) as u8;
        n += 1;
        v >>= 7;
        if v == 0 {
            break;
        }
    }
    for i in (0..n).rev() {
        let more = if i > 0 { 0x80 } else { 0 };
        out.push(bytes[i] | more);
    }
}

/// The length of `value` as a varint.
pub(crate) fn len(value: u64) -> usize {
    let mut out = alloc::vec::Vec::new();
    write(&mut out, value);
    out.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    #[test]
    fn round_trips() {
        for value in [
            0u64,
            1,
            127,
            128,
            240,
            16_383,
            16_384,
            0xffff_ffff,
            0x00ff_ffff_ffff_ffff,
            0x0100_0000_0000_0000,
            u64::MAX,
        ] {
            let mut out = Vec::new();
            write(&mut out, value);
            assert_eq!(read(&out), Some((value, out.len())), "{value}");
            assert_eq!(len(value), out.len());
        }
        assert_eq!(read(&[0x81]), None);
        let mut out = Vec::new();
        write(&mut out, 300);
        assert_eq!(out, [0x82, 0x2c]);
    }
}
