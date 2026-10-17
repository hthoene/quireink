//! Baseline JPEG, decoded one row of MCUs at a time. For gray only the
//! first component (luma) is transformed; the others are entropy-decoded
//! and dropped. For a sink that wants colour, the chroma components are
//! transformed too, stretched to luma's resolution and turned into RGB.
//! Progressive JPEG goes on in [`progressive`](crate::progressive).

use alloc::vec::Vec;

use crate::{Downscaler, Fit, ImageError, ImageSink, Reader, block_side, zeroed};

/// Natural order of the zig-zag sequence.
pub(crate) const ZIGZAG: [u8; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

#[derive(Clone)]
pub(crate) struct Huffman {
    /// Largest code of each length (1 to 16), -1 if none.
    max_code: [i32; 18],
    /// Index into `values` of the first code of each length, minus that code.
    offset: [i32; 17],
    values: Vec<u8>,
    /// The codes of up to [`FAST_BITS`] bits by the next bits: their
    /// length times 256 plus their value, 0 where a longer code begins.
    fast: [u16; 1 << FAST_BITS],
}

/// Bits of the codes looked up at once.
const FAST_BITS: u32 = 8;

impl Huffman {
    fn new(counts: &[u8; 16], values: Vec<u8>) -> Self {
        let mut max_code = [-1i32; 18];
        let mut offset = [0i32; 17];
        let mut fast = [0u16; 1 << FAST_BITS];
        let (mut code, mut k) = (0i32, 0i32);
        for len in 1..=16 {
            let count = i32::from(counts[len - 1]);
            offset[len] = k - code;
            if len <= FAST_BITS as usize {
                let spread = FAST_BITS as usize - len;
                for c in 0..count {
                    let Some(&value) = values.get((k + c) as usize) else {
                        break;
                    };
                    let first = ((code + c) as usize) << spread;
                    let entry = (len as u16) << 8 | u16::from(value);
                    for slot in fast.iter_mut().skip(first).take(1 << spread) {
                        *slot = entry;
                    }
                }
            }
            code += count;
            k += count;
            max_code[len] = if count > 0 { code - 1 } else { -1 };
            code <<= 1;
        }
        // Sentinel: longer codes do not exist.
        max_code[17] = i32::MAX;
        Self {
            max_code,
            offset,
            values,
            fast,
        }
    }
}

#[derive(Clone, Copy, Default)]
pub(crate) struct Component {
    pub id: u8,
    pub h: usize,
    pub v: usize,
    pub quant: usize,
    pub dc: usize,
    pub ac: usize,
    pub prediction: i32,
}

/// Entropy-coded data: bits with byte stuffing and markers.
pub(crate) struct Bits<'r, 'a> {
    pub reader: &'r mut Reader<'a>,
    pub bits: u32,
    pub count: u32,
    /// A marker ended the data; zeros are read from here on.
    pub marker: Option<u8>,
}

impl<'r, 'a> Bits<'r, 'a> {
    /// Takes bytes until more than 24 bits are at hand (zeros after a
    /// marker).
    fn fill(&mut self) -> Result<(), ImageError> {
        while self.count <= 24 {
            let byte = match self.marker {
                Some(_) => 0,
                None => {
                    let byte = self.reader.byte()?;
                    if byte == 0xff {
                        let mut next = self.reader.byte()?;
                        while next == 0xff {
                            next = self.reader.byte()?;
                        }
                        if next != 0 {
                            self.marker = Some(next);
                            0
                        } else {
                            0xff
                        }
                    } else {
                        byte
                    }
                }
            };
            self.bits = self.bits << 8 | u32::from(byte);
            self.count += 8;
        }
        Ok(())
    }

    /// The next `n` bits (at most 24), not taken.
    fn peek(&mut self, n: u32) -> Result<u32, ImageError> {
        if self.count < n {
            self.fill()?;
        }
        Ok((self.bits >> (self.count - n)) & ((1 << n) - 1))
    }

    pub(crate) fn bit(&mut self) -> Result<u32, ImageError> {
        let bit = self.peek(1)?;
        self.count -= 1;
        Ok(bit)
    }

    pub(crate) fn receive(&mut self, n: u32) -> Result<i32, ImageError> {
        if n > 16 {
            let high = self.receive(n - 16)?;
            return Ok(high << 16 | self.receive(16)?);
        }
        if n == 0 {
            return Ok(0);
        }
        let v = self.peek(n)?;
        self.count -= n;
        Ok(v as i32)
    }

    pub(crate) fn extend(v: i32, n: u32) -> i32 {
        if n == 0 {
            0
        } else if v < 1 << (n - 1) {
            v - (1 << n) + 1
        } else {
            v
        }
    }

    pub(crate) fn decode(&mut self, table: &Huffman) -> Result<u8, ImageError> {
        let next = self.peek(FAST_BITS)?;
        let entry = table.fast[next as usize];
        if entry != 0 {
            self.count -= u32::from(entry >> 8);
            return Ok(entry as u8);
        }
        // A longer code: bit by bit from there.
        self.count -= FAST_BITS;
        let mut code = next as i32;
        for len in FAST_BITS as usize + 1..=16 {
            code = (code << 1) | self.bit()? as i32;
            if code <= table.max_code[len] {
                let index = (table.offset[len] + code) as usize;
                return table
                    .values
                    .get(index)
                    .copied()
                    .ok_or(ImageError::Invalid("bad Huffman code"));
            }
        }
        Err(ImageError::Invalid("bad Huffman code"))
    }

    /// After a restart interval: realign and expect an RST marker.
    pub(crate) fn restart(&mut self) -> Result<(), ImageError> {
        self.count = 0;
        match self.marker.take() {
            Some(0xd0..=0xd7) => Ok(()),
            Some(_) => Err(ImageError::Invalid("missing restart marker")),
            None => {
                // Find the marker in the data.
                loop {
                    if self.reader.byte()? != 0xff {
                        continue;
                    }
                    let mut next = self.reader.byte()?;
                    while next == 0xff {
                        next = self.reader.byte()?;
                    }
                    match next {
                        0xd0..=0xd7 => return Ok(()),
                        0 => {}
                        _ => return Err(ImageError::Invalid("missing restart marker")),
                    }
                }
            }
        }
    }
}

/// The cosine basis c[u][x] = C(u) cos((2x+1)uπ/2n) / 2, scaled by 2^12,
/// for blocks of n = 8, 4, 2 or 1 pixels a side.
const C8: [[i32; 8]; 8] = [
    [1448, 1448, 1448, 1448, 1448, 1448, 1448, 1448],
    [2009, 1703, 1138, 400, -400, -1138, -1703, -2009],
    [1892, 784, -784, -1892, -1892, -784, 784, 1892],
    [1703, -400, -2009, -1138, 1138, 2009, 400, -1703],
    [1448, -1448, -1448, 1448, 1448, -1448, -1448, 1448],
    [1138, -2009, 400, 1703, -1703, -400, 2009, -1138],
    [784, -1892, 1892, -784, -784, 1892, -1892, 784],
    [400, -1138, 1703, -2009, 2009, -1703, 1138, -400],
];
const C4: [[i32; 8]; 8] = [
    [1448, 1448, 1448, 1448, 0, 0, 0, 0],
    [1892, 784, -784, -1892, 0, 0, 0, 0],
    [1448, -1448, -1448, 1448, 0, 0, 0, 0],
    [784, -1892, 1892, -784, 0, 0, 0, 0],
    [0; 8],
    [0; 8],
    [0; 8],
    [0; 8],
];
const C2: [[i32; 8]; 8] = [
    [1448, 1448, 0, 0, 0, 0, 0, 0],
    [1448, -1448, 0, 0, 0, 0, 0, 0],
    [0; 8],
    [0; 8],
    [0; 8],
    [0; 8],
    [0; 8],
    [0; 8],
];
const C1: [[i32; 8]; 8] = [
    [1448, 0, 0, 0, 0, 0, 0, 0],
    [0; 8],
    [0; 8],
    [0; 8],
    [0; 8],
    [0; 8],
    [0; 8],
    [0; 8],
];

/// Coefficients are kept within this (a valid one is within 2048), so
/// sums stay within 32 bits: the processor of the X4 multiplies 64-bit
/// numbers slowly.
const COEFFICIENT: i32 = 4095;

/// Inverse DCT of the `n`×`n` lowest frequencies of a block, which gives
/// the block at `n`/8 of its size (`n` = 8, 4, 2 or 1).
pub(crate) fn idct_scaled(coef: &[i32; 64], n: usize, out: &mut [u8], stride: usize) {
    if n == 4 {
        return idct4(coef, out, stride);
    }
    let c = match n {
        8 => &C8,
        4 => &C4,
        2 => &C2,
        _ => &C1,
    };
    let n = n.clamp(1, 8);
    let mut tmp = [0i32; 64];
    // Rows: frequencies u (horizontal) to positions x.
    for v in 0..n {
        let row = &coef[v * 8..v * 8 + n];
        if row.iter().all(|&c| c == 0) {
            continue;
        }
        for x in 0..n {
            let mut sum = 0i32;
            for (u, &value) in row.iter().enumerate() {
                sum += value.clamp(-COEFFICIENT, COEFFICIENT) * c[u][x];
            }
            tmp[v * 8 + x] = sum >> 10;
        }
    }
    // Columns.
    for x in 0..n {
        for y in 0..n {
            let mut sum = 0i32;
            for v in 0..n {
                sum += tmp[v * 8 + x] * c[v][y];
            }
            // Scales: 2^12 · 2^12 / 2^10 = 2^14, plus the level shift.
            let value = ((sum + (1 << 13)) >> 14) + 128;
            out[y * stride + x] = value.clamp(0, 255) as u8;
        }
    }
}

/// [`C4`] applied to four frequencies, with 6 multiplications instead of
/// 16 (the same sums, so the same numbers).
fn idct4_line(f: [i32; 4]) -> [i32; 4] {
    let even = (f[0] + f[2]) * 1448;
    let odd = (f[0] - f[2]) * 1448;
    let a = f[1] * 1892 + f[3] * 784;
    let b = f[1] * 784 - f[3] * 1892;
    [even + a, odd + b, odd - b, even - a]
}

/// [`idct_scaled`] at half the size (`n` = 4), the most common.
fn idct4(coef: &[i32; 64], out: &mut [u8], stride: usize) {
    let mut tmp = [[0i32; 4]; 4];
    for (v, row) in tmp.iter_mut().enumerate() {
        let f = core::array::from_fn(|u| coef[v * 8 + u].clamp(-COEFFICIENT, COEFFICIENT));
        if f != [0; 4] {
            *row = idct4_line(f).map(|sum| sum >> 10);
        }
    }
    for x in 0..4 {
        let column = idct4_line([tmp[0][x], tmp[1][x], tmp[2][x], tmp[3][x]]);
        for (y, sum) in column.into_iter().enumerate() {
            out[y * stride + x] = (((sum + (1 << 13)) >> 14) + 128).clamp(0, 255) as u8;
        }
    }
}

/// The level of a block with only its DC coefficient, as
/// [`idct_scaled`] gives it.
pub(crate) fn dc_level(dc: i32) -> u8 {
    let tmp = (dc.clamp(-COEFFICIENT, COEFFICIENT) * 1448) >> 10;
    (((tmp * 1448 + (1 << 13)) >> 14) + 128).clamp(0, 255) as u8
}

pub(crate) fn decode(
    reader: &mut Reader<'_>,
    fit: Fit,
    sink: &mut dyn ImageSink,
) -> Result<(), ImageError> {
    if reader.byte()? != 0xff || reader.byte()? != 0xd8 {
        return Err(ImageError::Invalid("not a JPEG"));
    }
    let mut quant = [[0u16; 64]; 4];
    let mut dc_tables: [Option<Huffman>; 4] = [None, None, None, None];
    let mut ac_tables: [Option<Huffman>; 4] = [None, None, None, None];
    let mut components: Vec<Component> = Vec::new();
    let (mut width, mut height) = (0u32, 0u32);
    let mut restart_interval = 0u32;
    let mut progressive = false;

    loop {
        // Next marker.
        let mut marker = reader.byte()?;
        if marker != 0xff {
            return Err(ImageError::Invalid("expected a marker"));
        }
        while marker == 0xff {
            marker = reader.byte()?;
        }
        match marker {
            0xd8 | 0x01 | 0xd0..=0xd7 => continue,
            0xd9 => return Err(ImageError::Invalid("no image data")),
            _ => {}
        }
        let len = usize::from(reader.u16_be()?)
            .checked_sub(2)
            .ok_or(ImageError::Invalid("bad segment length"))?;
        match marker {
            0xdb => read_quant(reader, len, &mut quant)?,
            0xc0..=0xc2 => {
                progressive = marker == 0xc2;
                if reader.byte()? != 8 {
                    return Err(ImageError::Unsupported("12-bit JPEG"));
                }
                height = u32::from(reader.u16_be()?);
                width = u32::from(reader.u16_be()?);
                let count = reader.byte()?;
                if width == 0 || height == 0 || !(count == 1 || count == 3) {
                    return Err(ImageError::Unsupported("JPEG components"));
                }
                components.clear();
                for _ in 0..count {
                    let id = reader.byte()?;
                    let hv = reader.byte()?;
                    let quant = usize::from(reader.byte()? & 3);
                    let (h, v) = (usize::from(hv >> 4), usize::from(hv & 15));
                    if !(1..=4).contains(&h) || !(1..=4).contains(&v) {
                        return Err(ImageError::Invalid("bad sampling"));
                    }
                    components.push(Component {
                        id,
                        h,
                        v,
                        quant,
                        ..Component::default()
                    });
                }
                reader.skip(len.saturating_sub(6 + 3 * usize::from(count)))?;
            }
            0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf => {
                return Err(ImageError::Unsupported("lossless or arithmetic-coded JPEG"));
            }
            0xc4 => read_huffman(reader, len, &mut dc_tables, &mut ac_tables)?,
            0xdd => {
                restart_interval = u32::from(reader.u16_be()?);
                reader.skip(len.saturating_sub(2))?;
            }
            0xda if progressive => {
                if components.is_empty() {
                    return Err(ImageError::Invalid("scan before frame"));
                }
                let frame = crate::progressive::Frame {
                    width,
                    height,
                    components,
                    quant,
                    dc_tables,
                    ac_tables,
                    restart_interval,
                };
                return crate::progressive::decode(reader, frame, fit, sink);
            }
            0xda => {
                let count = usize::from(reader.byte()?);
                if components.is_empty() || count != components.len() {
                    return Err(ImageError::Unsupported("non-interleaved JPEG scans"));
                }
                for _ in 0..count {
                    let id = reader.byte()?;
                    let tables = reader.byte()?;
                    let component = components
                        .iter_mut()
                        .find(|c| c.id == id)
                        .ok_or(ImageError::Invalid("unknown component"))?;
                    component.dc = usize::from(tables >> 4 & 3);
                    component.ac = usize::from(tables & 3);
                }
                reader.skip(3)?;
                return scan(
                    reader,
                    &mut components,
                    &quant,
                    &dc_tables,
                    &ac_tables,
                    (width, height, restart_interval),
                    fit,
                    sink,
                );
            }
            _ => reader.skip(len)?,
        }
    }
}

/// A quantization table segment (`DQT`) of `len` bytes.
pub(crate) fn read_quant(
    reader: &mut Reader<'_>,
    len: usize,
    quant: &mut [[u16; 64]; 4],
) -> Result<(), ImageError> {
    let mut left = len;
    while left > 0 {
        let pq_tq = reader.byte()?;
        let table = quant
            .get_mut(usize::from(pq_tq & 15))
            .ok_or(ImageError::Invalid("bad table"))?;
        let wide = pq_tq >> 4 != 0;
        for entry in table.iter_mut() {
            *entry = if wide {
                reader.u16_be()?
            } else {
                u16::from(reader.byte()?)
            };
        }
        left = left.saturating_sub(1 + if wide { 128 } else { 64 });
    }
    Ok(())
}

/// A Huffman table segment (`DHT`) of `len` bytes.
pub(crate) fn read_huffman(
    reader: &mut Reader<'_>,
    len: usize,
    dc_tables: &mut [Option<Huffman>; 4],
    ac_tables: &mut [Option<Huffman>; 4],
) -> Result<(), ImageError> {
    let mut left = len;
    while left >= 17 {
        let tc_th = reader.byte()?;
        let mut counts = [0u8; 16];
        reader.fill(&mut counts)?;
        let total: usize = counts.iter().map(|&c| usize::from(c)).sum();
        if total > 256 {
            return Err(ImageError::Invalid("bad Huffman table"));
        }
        let mut values = zeroed(total)?;
        reader.fill(&mut values)?;
        let table = Some(Huffman::new(&counts, values));
        let index = usize::from(tc_th & 3);
        if tc_th >> 4 == 0 {
            dc_tables[index] = table;
        } else {
            ac_tables[index] = table;
        }
        left = left
            .checked_sub(17 + total)
            .ok_or(ImageError::Invalid("bad Huffman table"))?;
    }
    reader.skip(left)
}

#[allow(clippy::too_many_arguments)]
fn scan(
    reader: &mut Reader<'_>,
    components: &mut [Component],
    quant: &[[u16; 64]; 4],
    dc_tables: &[Option<Huffman>; 4],
    ac_tables: &[Option<Huffman>; 4],
    (width, height, restart_interval): (u32, u32, u32),
    fit: Fit,
    sink: &mut dyn ImageSink,
) -> Result<(), ImageError> {
    for c in components.iter() {
        if dc_tables[c.dc].is_none() || ac_tables[c.ac].is_none() {
            return Err(ImageError::Invalid("missing Huffman table"));
        }
    }
    // A single component is not interleaved: its MCU is one block.
    let single = components.len() == 1;
    if single {
        components[0].h = 1;
        components[0].v = 1;
    }
    let h_max = components.iter().map(|c| c.h).max().unwrap_or(1);
    let v_max = components.iter().map(|c| c.v).max().unwrap_or(1);
    let luma = components[0];
    // Luma at less than full resolution is rare; it is stretched.
    let (sx, sy) = (h_max / luma.h, v_max / luma.v);
    let mcus_x = (width as usize).div_ceil(8 * h_max);
    let mcus_y = (height as usize).div_ceil(8 * v_max);

    // Each block decoded at n×n pixels of its 8×8 (8, 4, 2 or 1): an image
    // made smaller is made smaller as it is decoded, from its lowest
    // frequencies, not decoded whole and averaged; then averaged to the
    // size it shows at.
    let size = fit.size(width, height);
    let n = block_side(width, height, size);
    let scale = (8 / n) as u32;
    let (out_width, out_height) = (width.div_ceil(scale), height.div_ceil(scale));
    let mcu_w = n * h_max;
    let mcu_h = n * v_max;
    let row_width = mcus_x * mcu_w;

    // Colour where it is wanted and the image has it (luma and chroma).
    let color = sink.wants_color() && components.len() == 3;
    let mut scaler = Downscaler::new(out_width, out_height, size, if color { 3 } else { 1 })?;
    sink.start(scaler.out_width(), scaler.out_height())?;
    let height = out_height;

    // One row of MCUs of luma at full resolution, and for colour of each
    // chroma component stretched to it.
    let mut rows: Vec<u8> = zeroed(row_width * mcu_h)?;
    let mut chroma: [Vec<u8>; 2] = if color {
        [zeroed(row_width * mcu_h)?, zeroed(row_width * mcu_h)?]
    } else {
        [Vec::new(), Vec::new()]
    };
    let mut rgb: Vec<u8> = if color {
        zeroed(row_width * 3)?
    } else {
        Vec::new()
    };
    let mut block = [0u8; 64];
    let mut bits = Bits {
        reader,
        bits: 0,
        count: 0,
        marker: None,
    };
    let mut until_restart = restart_interval;
    let mut emitted = 0u32;

    for _ in 0..mcus_y {
        for mx in 0..mcus_x {
            if restart_interval > 0 {
                if until_restart == 0 {
                    bits.restart()?;
                    for c in components.iter_mut() {
                        c.prediction = 0;
                    }
                    until_restart = restart_interval;
                }
                until_restart -= 1;
            }
            for (index, component) in components.iter_mut().enumerate() {
                let dc = dc_tables[component.dc]
                    .as_ref()
                    .ok_or(ImageError::Invalid("table"))?;
                let ac = ac_tables[component.ac]
                    .as_ref()
                    .ok_or(ImageError::Invalid("table"))?;
                let q = &quant[component.quant];
                for by in 0..component.v {
                    for bx in 0..component.h {
                        let mut coef = [0i32; 64];
                        let t = bits.decode(dc)?;
                        if t > 16 {
                            return Err(ImageError::Invalid("bad DC"));
                        }
                        let diff = Bits::extend(bits.receive(u32::from(t))?, u32::from(t));
                        // Damaged data must not overflow.
                        component.prediction = component.prediction.wrapping_add(diff);
                        coef[0] = component.prediction.saturating_mul(i32::from(q[0]));
                        // Whether a coefficient decoded counts at this size.
                        let mut shaded = false;
                        let mut k = 1;
                        while k < 64 {
                            let rs = bits.decode(ac)?;
                            let (r, s) = (usize::from(rs >> 4), u32::from(rs & 15));
                            if s == 0 {
                                if r == 15 {
                                    k += 16;
                                    continue;
                                }
                                break;
                            }
                            k += r;
                            if k > 63 {
                                return Err(ImageError::Invalid("bad AC"));
                            }
                            if index == 0 || color {
                                let value = Bits::extend(bits.receive(s)?, s);
                                let at = usize::from(ZIGZAG[k]);
                                coef[at] = value.saturating_mul(i32::from(q[k]));
                                shaded |= at % 8 < n && at / 8 < n;
                            } else {
                                bits.receive(s)?;
                            }
                            k += 1;
                        }
                        if index != 0 && !color {
                            continue;
                        }
                        if shaded {
                            idct_scaled(&coef, n, &mut block, n);
                        } else {
                            // One level: most blocks of a picture made
                            // smaller.
                            block[..n * n].fill(dc_level(coef[0]));
                        }
                        // Place the block, stretched where the component
                        // has less resolution than the largest.
                        let (sx, sy) = if index == 0 {
                            (sx, sy)
                        } else {
                            (h_max / component.h, v_max / component.v)
                        };
                        let plane = match index {
                            0 => &mut rows,
                            1 => &mut chroma[0],
                            _ => &mut chroma[1],
                        };
                        let x0 = (mx * h_max + bx * sx) * n;
                        let y0 = by * sy * n;
                        if (sx, sy) == (1, 1) {
                            for (y, from) in block.chunks_exact(n).take(n).enumerate() {
                                let at = (y0 + y) * row_width + x0;
                                plane[at..at + n].copy_from_slice(from);
                            }
                        } else {
                            for y in 0..n * sy {
                                for x in 0..n * sx {
                                    let at = (y0 + y) * row_width + x0 + x;
                                    plane[at] = block[(y / sy) * n + x / sx];
                                }
                            }
                        }
                    }
                }
            }
        }
        // Hand over the rows of this MCU row that belong to the image.
        for y in 0..mcu_h {
            if emitted == height {
                break;
            }
            let luma_row = &rows[y * row_width..(y + 1) * row_width];
            if color {
                let at = y * row_width;
                let (cb, cr) = (
                    &chroma[0][at..at + row_width],
                    &chroma[1][at..at + row_width],
                );
                for x in 0..row_width {
                    let pixel = ycbcr_to_rgb(luma_row[x], cb[x], cr[x]);
                    rgb[x * 3..x * 3 + 3].copy_from_slice(&pixel);
                }
                scaler.row(&rgb, sink)?;
            } else {
                scaler.row(luma_row, sink)?;
            }
            emitted += 1;
        }
    }
    scaler.finish(sink)
}

/// JFIF's YCbCr as RGB.
pub(crate) fn ycbcr_to_rgb(y: u8, cb: u8, cr: u8) -> [u8; 3] {
    let y = i32::from(y);
    let (cb, cr) = (i32::from(cb) - 128, i32::from(cr) - 128);
    let clamp = |v: i32| v.clamp(0, 255) as u8;
    [
        clamp(y + ((91_881 * cr + 32_768) >> 16)),
        clamp(y - ((22_554 * cb + 46_802 * cr + 32_768) >> 16)),
        clamp(y + ((116_130 * cb + 32_768) >> 16)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_size_idct_as_the_general_one() {
        // Any coefficients: the shortcut gives the general sums' numbers.
        let mut seed = 7u32;
        for _ in 0..500 {
            let mut coef = [0i32; 64];
            for c in coef.iter_mut() {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                *c = (seed >> 16) as i32 % 3000 - 1500;
            }
            let (mut fast, mut general) = ([0u8; 16], [0u8; 16]);
            idct4(&coef, &mut fast, 4);
            let c = &C4;
            let mut tmp = [0i32; 64];
            for v in 0..4 {
                for x in 0..4 {
                    tmp[v * 8 + x] = (0..4).map(|u| coef[v * 8 + u] * c[u][x]).sum::<i32>() >> 10;
                }
            }
            for x in 0..4 {
                for y in 0..4 {
                    let sum: i32 = (0..4).map(|v| tmp[v * 8 + x] * c[v][y]).sum();
                    general[y * 4 + x] = (((sum + (1 << 13)) >> 14) + 128).clamp(0, 255) as u8;
                }
            }
            assert_eq!(fast, general);
        }
    }

    #[test]
    fn ycbcr_primaries() {
        assert_eq!(ycbcr_to_rgb(128, 128, 128), [128, 128, 128]);
        // Pure red, green and blue as JFIF codes them.
        let close = |a: [u8; 3], b: [u8; 3]| a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= 2);
        assert!(close(ycbcr_to_rgb(76, 85, 255), [255, 0, 0]));
        assert!(close(ycbcr_to_rgb(150, 44, 21), [0, 255, 0]));
        assert!(close(ycbcr_to_rgb(29, 255, 107), [0, 0, 255]));
    }

    #[test]
    fn idct_of_a_flat_block_is_flat() {
        let mut coef = [0i32; 64];
        // DC 80 → 80/8 = 10 above mid-gray.
        coef[0] = 80;
        let mut out = [0u8; 64];
        idct_scaled(&coef, 8, &mut out, 8);
        assert!(out.iter().all(|&v| (137..=139).contains(&v)), "{out:?}");
    }
}
