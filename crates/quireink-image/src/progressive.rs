//! Progressive JPEG: the image comes in several scans, each adding
//! frequencies or bits of precision to every block, so all blocks are
//! kept until the last scan.
//!
//! To stay small, only luma is kept, and only the frequencies the output
//! needs: an image shown at half its size or less needs the 4×4 lowest
//! frequencies of each block, at a quarter 2×2, at an eighth only the DC.
//! Scans of the other components (and, where only the DC is kept, of
//! the other frequencies) are skipped without decoding. Per block that is 2 bytes (an eighth), 16 (a
//! quarter), 40 (half) or 136 (full size); if that cannot be had with
//! some memory left over, the next smaller size is tried.

use alloc::vec::Vec;

use crate::jpeg::{Bits, Component, Huffman, ZIGZAG, idct_scaled, read_huffman, read_quant};
use crate::{Downscaler, ImageError, ImageSink, Reader, factor_for, zeroed};

const MISSING: ImageError = ImageError::Invalid("missing Huffman table");

/// Memory left free beside the kept coefficients, for everything else
/// (small devices then keep fewer frequencies).
const HEADROOM: usize = 48 << 10;

/// Most memory the kept coefficients may take, whatever the device.
const MAX_COEFFICIENT_BYTES: usize = 8 << 20;

/// What the frame header and the segments before the first scan said.
pub(crate) struct Frame {
    pub width: u32,
    pub height: u32,
    pub components: Vec<Component>,
    pub quant: [[u16; 64]; 4],
    pub dc_tables: [Option<Huffman>; 4],
    pub ac_tables: [Option<Huffman>; 4],
    pub restart_interval: u32,
}

/// The kept coefficients of all luma blocks.
struct Coefficients {
    /// Pixels a block side at the output scale (8, 4, 2 or 1).
    n: usize,
    /// Coefficients kept a block (`n`×`n`).
    kept: usize,
    /// For each zig-zag index, its place among a block's kept
    /// coefficients, if kept.
    slot: [Option<u8>; 64],
    /// The kept coefficients, block after block, before dequantization.
    values: Vec<i16>,
    /// Which coefficients are not zero, a bit each by zig-zag index; for
    /// refining scans (not needed when only the DC is kept).
    nonzero: Vec<u64>,
}

impl Coefficients {
    fn new(blocks: usize, n: usize) -> Result<Self, ImageError> {
        let kept = n * n;
        let bytes = blocks * (kept * 2 + if n > 1 { 8 } else { 0 });
        if bytes > MAX_COEFFICIENT_BYTES {
            return Err(ImageError::OutOfMemory);
        }
        let mut slot = [None; 64];
        for (z, &pos) in ZIGZAG.iter().enumerate() {
            let (row, col) = (usize::from(pos) / 8, usize::from(pos) % 8);
            if row < n && col < n {
                slot[z] = Some((row * n + col) as u8);
            }
        }
        // Only if the headroom is left as well.
        let headroom: Vec<u8> = zeroed(HEADROOM)?;
        let coefficients = Self {
            n,
            kept,
            slot,
            values: zeroed(blocks * kept)?,
            nonzero: if n > 1 { zeroed(blocks)? } else { Vec::new() },
        };
        drop(headroom);
        Ok(coefficients)
    }

    fn value(&mut self, block: usize, z: usize) -> Option<&mut i16> {
        let slot = usize::from(self.slot[z]?);
        self.values.get_mut(block * self.kept + slot)
    }

    fn is_nonzero(&self, block: usize, z: usize) -> bool {
        self.nonzero
            .get(block)
            .is_some_and(|bits| bits >> z & 1 == 1)
    }

    fn set(&mut self, block: usize, z: usize, value: i32) {
        if let Some(v) = self.value(block, z) {
            *v = value.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
        }
        if value != 0
            && let Some(bits) = self.nonzero.get_mut(block)
        {
            *bits |= 1 << z;
        }
    }

    /// A correction bit for a coefficient already not zero.
    fn refine(&mut self, block: usize, z: usize, p1: i32) {
        if let Some(v) = self.value(block, z) {
            let c = i32::from(*v);
            if c & p1 == 0 {
                let c = if c >= 0 { c + p1 } else { c - p1 };
                *v = c.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
            }
        }
    }
}

/// The geometry of the luma blocks.
struct Layout {
    h_max: usize,
    v_max: usize,
    mcus_x: usize,
    mcus_y: usize,
    /// Luma blocks a row, and rows, as stored (whole MCUs).
    width: usize,
    height: usize,
}

/// Where the next marker is: read up to it, the marker byte returned.
fn next_marker(reader: &mut Reader<'_>) -> Result<u8, ImageError> {
    loop {
        if reader.byte()? != 0xff {
            continue;
        }
        let mut marker = reader.byte()?;
        while marker == 0xff {
            marker = reader.byte()?;
        }
        // Stuffed zeros and restarts belong to the data.
        if marker != 0 && !(0xd0..=0xd7).contains(&marker) {
            return Ok(marker);
        }
    }
}

pub(crate) fn decode(
    reader: &mut Reader<'_>,
    mut frame: Frame,
    max_side: u32,
    sink: &mut dyn ImageSink,
) -> Result<(), ImageError> {
    let (width, height) = (frame.width as usize, frame.height as usize);
    if frame.components.len() == 1 {
        frame.components[0].h = 1;
        frame.components[0].v = 1;
    }
    let h_max = frame.components.iter().map(|c| c.h).max().unwrap_or(1);
    let v_max = frame.components.iter().map(|c| c.v).max().unwrap_or(1);
    let luma = frame.components[0];
    let mcus_x = width.div_ceil(8 * h_max);
    let mcus_y = height.div_ceil(8 * v_max);
    let layout = Layout {
        h_max,
        v_max,
        mcus_x,
        mcus_y,
        width: mcus_x * luma.h,
        height: mcus_y * luma.v,
    };
    // The smallest output scale the requested size allows, else smaller
    // ones where memory is short.
    let factor = factor_for(frame.width, frame.height, max_side) as usize;
    let mut n = match factor {
        1 => 8,
        2..=3 => 4,
        4..=7 => 2,
        _ => 1,
    };
    let blocks = layout.width * layout.height;
    let mut coefficients = loop {
        match Coefficients::new(blocks, n) {
            Ok(c) => break c,
            Err(_) if n > 1 => n /= 2,
            Err(e) => return Err(e),
        }
    };
    // The first scan's header is next (its length already read).
    let mut marker = 0xda;
    let mut first = true;
    loop {
        match marker {
            0xd9 => break,
            0xda => {
                if !first {
                    reader.u16_be()?;
                }
                first = false;
                let next = scan(reader, &mut frame, &layout, &mut coefficients)?;
                marker = next;
                continue;
            }
            0xd0..=0xd7 | 0x01 | 0xd8 => {}
            _ => {
                let len = usize::from(reader.u16_be()?)
                    .checked_sub(2)
                    .ok_or(ImageError::Invalid("bad segment length"))?;
                match marker {
                    0xc4 => read_huffman(reader, len, &mut frame.dc_tables, &mut frame.ac_tables)?,
                    0xdb => read_quant(reader, len, &mut frame.quant)?,
                    0xdd => {
                        frame.restart_interval = u32::from(reader.u16_be()?);
                        reader.skip(len.saturating_sub(2))?;
                    }
                    _ => reader.skip(len)?,
                }
            }
        }
        marker = match next_marker(reader) {
            Ok(m) => m,
            // Some files end without EOI: show what there is.
            Err(_) => break,
        };
    }
    output(&frame, &layout, &coefficients, max_side, sink)
}

/// The kinds of scans.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    DcFirst,
    DcRefine,
    AcFirst,
    AcRefine,
}

/// Decodes one scan (its header next) into `coefficients`; the marker
/// after it.
fn scan(
    reader: &mut Reader<'_>,
    frame: &mut Frame,
    layout: &Layout,
    coefficients: &mut Coefficients,
) -> Result<u8, ImageError> {
    let count = usize::from(reader.byte()?);
    if count == 0 || count > 4 {
        return Err(ImageError::Invalid("bad scan"));
    }
    let mut members: Vec<usize> = Vec::with_capacity(count);
    for _ in 0..count {
        let id = reader.byte()?;
        let tables = reader.byte()?;
        let index = frame
            .components
            .iter()
            .position(|c| c.id == id)
            .ok_or(ImageError::Invalid("unknown component"))?;
        frame.components[index].dc = usize::from(tables >> 4 & 3);
        frame.components[index].ac = usize::from(tables & 3);
        members.push(index);
    }
    let start = usize::from(reader.byte()?);
    let end = usize::from(reader.byte()?);
    let approximation = reader.byte()?;
    let (high, low) = (approximation >> 4, approximation & 15);
    if start > end || end > 63 || (start == 0 && end != 0) || low > 13 {
        return Err(ImageError::Invalid("bad scan"));
    }
    let kind = match (start == 0, high == 0) {
        (true, true) => Kind::DcFirst,
        (true, false) => Kind::DcRefine,
        (false, true) => Kind::AcFirst,
        (false, false) => Kind::AcRefine,
    };
    let luma_in = members.contains(&0);
    // Nothing kept in this scan: skipped without decoding. (A luma band
    // without kept frequencies is still decoded where others are kept:
    // later refining scans may span it and need to know what it set.)
    if !luma_in || start > 0 && (count != 1 || coefficients.n == 1) {
        return next_marker(reader);
    }
    for &c in &members {
        let component = frame.components[c];
        let missing = match kind {
            Kind::DcFirst => frame.dc_tables[component.dc].is_none(),
            Kind::AcFirst | Kind::AcRefine => frame.ac_tables[component.ac].is_none(),
            Kind::DcRefine => false,
        };
        if missing {
            return Err(ImageError::Invalid("missing Huffman table"));
        }
        frame.components[c].prediction = 0;
    }

    let mut bits = Bits {
        reader,
        bits: 0,
        count: 0,
        marker: None,
    };
    let restart = frame.restart_interval;
    let mut until_restart = restart;
    let mut eob_run = 0u32;
    // Before each MCU: a restart, where due.
    let mut before_mcu = |bits: &mut Bits<'_, '_>,
                          components: &mut [Component],
                          eob_run: &mut u32|
     -> Result<(), ImageError> {
        if restart > 0 {
            if until_restart == 0 {
                bits.restart()?;
                for &c in &members {
                    components[c].prediction = 0;
                }
                *eob_run = 0;
                until_restart = restart;
            }
            until_restart -= 1;
        }
        Ok(())
    };

    if count == 1 {
        // Not interleaved: the component's own blocks, row by row.
        let c = members[0];
        let component = frame.components[c];
        let blocks_x = (width_of(frame, layout, c)).div_ceil(8);
        let blocks_y = (height_of(frame, layout, c)).div_ceil(8);
        for by in 0..blocks_y {
            for bx in 0..blocks_x {
                before_mcu(&mut bits, &mut frame.components, &mut eob_run)?;
                let block = by * layout.width + bx;
                let dc = frame.dc_tables[component.dc].as_ref();
                let ac = frame.ac_tables[component.ac].as_ref();
                decode_block(
                    &mut bits,
                    kind,
                    (start, end, low),
                    (dc, ac),
                    &mut frame.components[c].prediction,
                    &mut eob_run,
                    coefficients,
                    Some(block),
                )?;
            }
        }
    } else {
        // Interleaved (DC only): MCU by MCU.
        for my in 0..layout.mcus_y {
            for mx in 0..layout.mcus_x {
                before_mcu(&mut bits, &mut frame.components, &mut eob_run)?;
                for &c in &members {
                    let component = frame.components[c];
                    for by in 0..component.v {
                        for bx in 0..component.h {
                            let block = (c == 0).then(|| {
                                (my * component.v + by) * layout.width + mx * component.h + bx
                            });
                            let dc = frame.dc_tables[component.dc].as_ref();
                            decode_block(
                                &mut bits,
                                kind,
                                (start, end, low),
                                (dc, None),
                                &mut frame.components[c].prediction,
                                &mut eob_run,
                                coefficients,
                                block,
                            )?;
                        }
                    }
                }
            }
        }
    }
    match bits.marker {
        Some(marker) if !(0xd0..=0xd7).contains(&marker) => Ok(marker),
        _ => next_marker(bits.reader),
    }
}

/// Pixels across component `c` covers.
fn width_of(frame: &Frame, layout: &Layout, c: usize) -> usize {
    (frame.width as usize * frame.components[c].h).div_ceil(layout.h_max)
}

fn height_of(frame: &Frame, layout: &Layout, c: usize) -> usize {
    (frame.height as usize * frame.components[c].v).div_ceil(layout.v_max)
}

/// Decodes one block of a scan; `block` is where a luma block is kept
/// (`None` for the other components, decoded only to go on).
#[allow(clippy::too_many_arguments)]
fn decode_block(
    bits: &mut Bits<'_, '_>,
    kind: Kind,
    (start, end, low): (usize, usize, u8),
    (dc, ac): (Option<&Huffman>, Option<&Huffman>),
    prediction: &mut i32,
    eob_run: &mut u32,
    coefficients: &mut Coefficients,
    block: Option<usize>,
) -> Result<(), ImageError> {
    match kind {
        Kind::DcFirst => {
            let t = bits.decode(dc.ok_or(MISSING)?)?;
            if t > 16 {
                return Err(ImageError::Invalid("bad DC"));
            }
            let diff = Bits::extend(bits.receive(u32::from(t))?, u32::from(t));
            *prediction = prediction.wrapping_add(diff);
            if let Some(block) = block {
                coefficients.set(block, 0, prediction.wrapping_shl(u32::from(low)));
            }
        }
        Kind::DcRefine => {
            if bits.bit()? == 1
                && let Some(v) = block.and_then(|b| coefficients.value(b, 0))
            {
                *v |= 1 << low;
            }
        }
        Kind::AcFirst => {
            let block = block.ok_or(ImageError::Invalid("bad scan"))?;
            if *eob_run > 0 {
                *eob_run -= 1;
                return Ok(());
            }
            let ac = ac.ok_or(MISSING)?;
            let mut k = start;
            while k <= end {
                let rs = bits.decode(ac)?;
                let (r, s) = (u32::from(rs >> 4), u32::from(rs & 15));
                if s == 0 {
                    if r < 15 {
                        // A run of blocks ending here.
                        *eob_run = (1 << r) - 1;
                        if r > 0 {
                            *eob_run += bits.receive(r)? as u32;
                        }
                        break;
                    }
                    k += 16;
                    continue;
                }
                k += r as usize;
                if k > 63 {
                    return Err(ImageError::Invalid("bad AC"));
                }
                let value = Bits::extend(bits.receive(s)?, s);
                coefficients.set(block, k, value.wrapping_shl(u32::from(low)));
                k += 1;
            }
        }
        Kind::AcRefine => {
            let block = block.ok_or(ImageError::Invalid("bad scan"))?;
            let ac = ac.ok_or(MISSING)?;
            let p1 = 1i32 << low;
            let mut k = start;
            if *eob_run == 0 {
                while k <= end {
                    let rs = bits.decode(ac)?;
                    let (mut r, s) = (i32::from(rs >> 4), rs & 15);
                    let mut value = 0;
                    if s != 0 {
                        value = if bits.bit()? == 1 { p1 } else { -p1 };
                    } else if r != 15 {
                        *eob_run = 1 << r;
                        if r > 0 {
                            *eob_run += bits.receive(r as u32)? as u32;
                        }
                        break;
                    }
                    // Over coefficients already set (each with a correction
                    // bit) and `r` still zero ones, to the new one.
                    while k <= end {
                        if coefficients.is_nonzero(block, k) {
                            if bits.bit()? == 1 {
                                coefficients.refine(block, k, p1);
                            }
                        } else {
                            r -= 1;
                            if r < 0 {
                                break;
                            }
                        }
                        k += 1;
                    }
                    if value != 0 && k <= end {
                        coefficients.set(block, k, value);
                    }
                    k += 1;
                }
            }
            if *eob_run > 0 {
                // The rest of the band: only corrections.
                while k <= end {
                    if coefficients.is_nonzero(block, k) && bits.bit()? == 1 {
                        coefficients.refine(block, k, p1);
                    }
                    k += 1;
                }
                *eob_run -= 1;
            }
        }
    }
    Ok(())
}

/// Turns the kept coefficients into rows of gray, scaled down to fit
/// `max_side`.
fn output(
    frame: &Frame,
    layout: &Layout,
    coefficients: &Coefficients,
    max_side: u32,
    sink: &mut dyn ImageSink,
) -> Result<(), ImageError> {
    let n = coefficients.n;
    let luma = frame.components[0];
    // Luma at less than full resolution is rare; it is stretched.
    let (sx, sy) = (layout.h_max / luma.h, layout.v_max / luma.v);
    let scale = 8 / n;
    let out_width = (frame.width as usize).div_ceil(scale);
    let out_height = (frame.height as usize).div_ceil(scale);
    let factor = factor_for(out_width as u32, out_height as u32, max_side);
    let mut scaler = Downscaler::new(out_width as u32, factor)?;
    sink.start(scaler.out_width(), (out_height as u32).div_ceil(factor))?;

    let quant = &frame.quant[luma.quant];
    // The zig-zag index of each kept coefficient.
    let mut zigzag_of = [0usize; 64];
    for (z, slot) in coefficients.slot.iter().enumerate() {
        if let Some(slot) = slot {
            zigzag_of[usize::from(*slot)] = z;
        }
    }
    let band_width = layout.width * n;
    let mut band: Vec<u8> = zeroed(band_width * n)?;
    let mut row: Vec<u8> = zeroed(out_width)?;
    let mut emitted = 0usize;
    let mut pixels = [0u8; 64];
    'rows: for by in 0..layout.height {
        for bx in 0..layout.width {
            let block = by * layout.width + bx;
            let mut coef = [0i32; 64];
            let kept =
                &coefficients.values[block * coefficients.kept..(block + 1) * coefficients.kept];
            for (&z, &value) in zigzag_of.iter().zip(kept) {
                let value = i32::from(value);
                let pos = usize::from(ZIGZAG[z]);
                coef[pos] = value.saturating_mul(i32::from(quant[z]));
            }
            idct_scaled(&coef, n, &mut pixels, n);
            for y in 0..n {
                let at = y * band_width + bx * n;
                band[at..at + n].copy_from_slice(&pixels[y * n..y * n + n]);
            }
        }
        for y in 0..n {
            let line = &band[y * band_width..(y + 1) * band_width];
            for (x, out) in row.iter_mut().enumerate() {
                *out = line.get(x / sx).copied().unwrap_or(255);
            }
            for _ in 0..sy {
                if emitted == out_height {
                    break 'rows;
                }
                scaler.row(&row, sink)?;
                emitted += 1;
            }
        }
    }
    scaler.finish(sink)
}
