//! GIF: the first picture of the file (the first frame of an animation),
//! on the logical screen it belongs to, decoded to gray.
//!
//! Colours go through the global or local colour table, to gray or, for
//! a sink that wants colour, to RGB; transparent pixels and the screen
//! around the picture are white. The
//! LZW codes are decoded as they come, with a table of at most 4096
//! entries (about 20 KiB). Pictures stored in order come out row by row;
//! interlaced ones are gathered in a buffer of their size first, which
//! fails with [`ImageError::OutOfMemory`] where memory is short.

use alloc::vec::Vec;

use crate::{Downscaler, Fit, ImageError, ImageSink, Reader, luma, zeroed};

/// The most codes LZW in GIF uses (12 bits).
const MAX_CODES: usize = 4096;
/// White, for the screen around the picture and transparent pixels.
const WHITE: u8 = 255;
/// Larger screens are refused (64 megapixels): no book needs them, and a
/// damaged size would take long to fill.
const MAX_PIXELS: u64 = 1 << 26;

/// The colours of a colour table of `entries` colours.
fn palette(reader: &mut Reader<'_>, entries: usize) -> Result<[[u8; 3]; 256], ImageError> {
    let mut colors = [[0u8; 3]; 256];
    for slot in colors.iter_mut().take(entries) {
        reader.fill(slot)?;
    }
    Ok(colors)
}

/// Skips data sub-blocks up to their terminator.
fn skip_blocks(reader: &mut Reader<'_>) -> Result<(), ImageError> {
    loop {
        let len = reader.byte()?;
        if len == 0 {
            return Ok(());
        }
        reader.skip(usize::from(len))?;
    }
}

fn u16_le(reader: &mut Reader<'_>) -> Result<u16, ImageError> {
    Ok(u16::from(reader.byte()?) | u16::from(reader.byte()?) << 8)
}

/// The bits of LZW codes from data sub-blocks, least significant first.
struct Bits<'r, 'a> {
    reader: &'r mut Reader<'a>,
    /// Bytes left in the current sub-block.
    block: u8,
    /// The last sub-block ended.
    done: bool,
    acc: u32,
    count: u32,
}

impl Bits<'_, '_> {
    /// The next code of `size` bits; `None` when the data ends.
    fn code(&mut self, size: u32) -> Result<Option<u16>, ImageError> {
        while self.count < size {
            if self.done {
                return Ok(None);
            }
            if self.block == 0 {
                self.block = self.reader.byte()?;
                if self.block == 0 {
                    self.done = true;
                    return Ok(None);
                }
            }
            self.acc |= u32::from(self.reader.byte()?) << self.count;
            self.block -= 1;
            self.count += 8;
        }
        let code = (self.acc & ((1 << size) - 1)) as u16;
        self.acc >>= size;
        self.count -= size;
        Ok(Some(code))
    }

    /// Skips what is left of the picture's data.
    fn finish(self) -> Result<(), ImageError> {
        if self.done {
            return Ok(());
        }
        self.reader.skip(usize::from(self.block))?;
        skip_blocks(self.reader)
    }
}

/// The LZW decoder: each code stands for a string, the string of an
/// earlier code with one more index.
struct Lzw {
    prefix: Vec<u16>,
    suffix: Vec<u8>,
    /// The first index of each code's string.
    first: Vec<u8>,
    /// A string, last index first, while it is written out.
    stack: Vec<u8>,
    min_size: u32,
    clear: u16,
    next: u16,
    size: u32,
    previous: Option<u16>,
}

impl Lzw {
    fn new(min_size: u8) -> Result<Self, ImageError> {
        if !(1..=11).contains(&min_size) {
            return Err(ImageError::Invalid("LZW code size"));
        }
        let min_size = u32::from(min_size);
        let clear = 1u16 << min_size;
        let mut lzw = Self {
            prefix: zeroed(MAX_CODES)?,
            suffix: zeroed(MAX_CODES)?,
            first: zeroed(MAX_CODES)?,
            stack: Vec::new(),
            min_size,
            clear,
            next: clear + 2,
            size: min_size + 1,
            previous: None,
        };
        lzw.stack
            .try_reserve_exact(MAX_CODES)
            .map_err(|_| ImageError::OutOfMemory)?;
        for code in 0..clear {
            lzw.suffix[usize::from(code)] = code as u8;
            lzw.first[usize::from(code)] = code as u8;
        }
        Ok(lzw)
    }

    fn reset(&mut self) {
        self.next = self.clear + 2;
        self.size = self.min_size + 1;
        self.previous = None;
    }

    /// Decodes `code`, passing its indices to `out` in order; `false` at
    /// the end code.
    fn decode(&mut self, code: u16, out: &mut impl FnMut(u8)) -> Result<bool, ImageError> {
        if code == self.clear {
            self.reset();
            return Ok(true);
        }
        if code == self.clear + 1 {
            return Ok(false);
        }
        let Some(previous) = self.previous else {
            if code >= self.clear {
                return Err(ImageError::Invalid("LZW code"));
            }
            out(code as u8);
            self.previous = Some(code);
            return Ok(true);
        };
        // A code not in the table yet is the previous string and its first
        // index again.
        let known = code < self.next;
        if !known && code != self.next {
            return Err(ImageError::Invalid("LZW code"));
        }
        let first = if known {
            self.first[usize::from(code)]
        } else {
            self.first[usize::from(previous)]
        };
        if usize::from(self.next) < MAX_CODES {
            let n = usize::from(self.next);
            self.prefix[n] = previous;
            self.suffix[n] = first;
            self.first[n] = self.first[usize::from(previous)];
            self.next += 1;
            if usize::from(self.next) == 1 << self.size && self.size < 12 {
                self.size += 1;
            }
        }
        self.stack.clear();
        let mut at = code;
        while at >= self.clear {
            self.stack.push(self.suffix[usize::from(at)]);
            at = self.prefix[usize::from(at)];
        }
        self.stack.push(at as u8);
        for &index in self.stack.iter().rev() {
            out(index);
        }
        self.previous = Some(code);
        Ok(true)
    }
}

pub(crate) fn decode(
    reader: &mut Reader<'_>,
    fit: Fit,
    sink: &mut dyn ImageSink,
) -> Result<(), ImageError> {
    let mut signature = [0u8; 6];
    reader.fill(&mut signature)?;
    if &signature != b"GIF87a" && &signature != b"GIF89a" {
        return Err(ImageError::Invalid("GIF signature"));
    }
    let width = u32::from(u16_le(reader)?);
    let height = u32::from(u16_le(reader)?);
    let packed = reader.byte()?;
    reader.skip(2)?; // background colour, aspect
    if width == 0 || height == 0 {
        return Err(ImageError::Invalid("GIF size"));
    }
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(ImageError::Unsupported("GIF this large"));
    }
    let global = if packed & 0x80 != 0 {
        Some(palette(reader, 2 << (packed & 7))?)
    } else {
        None
    };
    let mut transparent = None;
    loop {
        match reader.byte()? {
            // An extension: only the transparent colour matters.
            0x21 => {
                if reader.byte()? == 0xf9 {
                    let len = reader.byte()?;
                    let mut control = [0u8; 4];
                    if len >= 4 {
                        reader.fill(&mut control)?;
                        reader.skip(usize::from(len - 4))?;
                        transparent = (control[0] & 1 != 0).then_some(control[3]);
                    } else {
                        reader.skip(usize::from(len))?;
                    }
                }
                skip_blocks(reader)?;
            }
            0x2c => return picture(reader, width, height, global, transparent, fit, sink),
            0x3b => return Err(ImageError::Invalid("GIF without a picture")),
            _ => return Err(ImageError::Invalid("GIF block")),
        }
    }
}

/// The picture whose descriptor follows, on a `width`×`height` screen.
#[allow(clippy::too_many_arguments)]
fn picture(
    reader: &mut Reader<'_>,
    width: u32,
    height: u32,
    global: Option<[[u8; 3]; 256]>,
    transparent: Option<u8>,
    fit: Fit,
    sink: &mut dyn ImageSink,
) -> Result<(), ImageError> {
    let left = u32::from(u16_le(reader)?);
    let top = u32::from(u16_le(reader)?);
    let w = u32::from(u16_le(reader)?);
    let h = u32::from(u16_le(reader)?);
    let packed = reader.byte()?;
    let interlaced = packed & 0x40 != 0;
    let mut colors = if packed & 0x80 != 0 {
        palette(reader, 2 << (packed & 7))?
    } else {
        global.ok_or(ImageError::Invalid("GIF without colours"))?
    };
    if let Some(index) = transparent {
        colors[usize::from(index)] = [WHITE; 3];
    }
    let color = sink.wants_color();
    let channels = if color { 3 } else { 1 };
    let grays = colors.map(|[r, g, b]| luma(u32::from(r), u32::from(g), u32::from(b)));
    // Pixel `x` of a row in colour `index`.
    let paint = |row: &mut [u8], x: usize, index: u8| {
        if color {
            row[x * 3..x * 3 + 3].copy_from_slice(&colors[usize::from(index)]);
        } else {
            row[x] = grays[usize::from(index)];
        }
    };
    let mut lzw = Lzw::new(reader.byte()?)?;
    let mut bits = Bits {
        reader,
        block: 0,
        done: false,
        acc: 0,
        count: 0,
    };
    // The part of the picture on the screen.
    let right = (left + w).min(width);
    let bottom = (top + h).min(height);
    let size = fit.size(width, height);
    let mut scaler = Downscaler::new(width, height, size, channels)?;
    sink.start(scaler.out_width(), scaler.out_height())?;
    let mut row: Vec<u8> = zeroed(width as usize * channels)?;
    row.fill(WHITE);
    let blank = row.clone();
    // Interlaced: every row of the picture first.
    let mut gathered: Vec<u8> = if interlaced {
        zeroed(w as usize * h as usize)?
    } else {
        Vec::new()
    };
    // Rows above the picture are the screen's.
    let mut y = 0;
    while y < top.min(height) {
        scaler.row(&blank, sink)?;
        y += 1;
    }
    // Where the next index goes: column and row within the picture.
    let (mut x, mut r) = (0u32, 0u32);
    let mut sent = top.min(height);
    let mut pass_rows = interlace_rows(h);
    let mut failure = None;
    'codes: while r < h {
        let Some(code) = bits.code(lzw.size)? else {
            break;
        };
        let mut put = |index: u8| {
            if r >= h {
                return;
            }
            if interlaced {
                gathered[(r * w + x) as usize] = index;
            } else if left + x < right {
                paint(&mut row, (left + x) as usize, index);
            }
            x += 1;
            if x == w {
                x = 0;
                if interlaced {
                    r = pass_rows.next().unwrap_or(h);
                } else {
                    // A row of the picture is done.
                    if top + r < bottom
                        && let Err(e) = scaler.row(&row, sink)
                    {
                        failure = Some(e);
                    }
                    sent = top + r + 1;
                    r += 1;
                }
            }
        };
        let more = lzw.decode(code, &mut put)?;
        if let Some(e) = failure.take() {
            return Err(e);
        }
        if !more {
            break 'codes;
        }
    }
    if interlaced {
        for pr in 0..h {
            let y = top + pr;
            if y >= bottom {
                break;
            }
            for px in 0..(right.saturating_sub(left)) {
                let index = gathered[(pr * w + px) as usize];
                paint(&mut row, (left + px) as usize, index);
            }
            scaler.row(&row, sink)?;
        }
        sent = bottom.max(top.min(height));
    }
    // A picture cut short: what is missing stays white, as the screen
    // below it.
    for _ in sent..height {
        scaler.row(&blank, sink)?;
    }
    scaler.finish(sink)?;
    bits.finish()
}

/// The rows of an interlaced picture `height` rows high in the order
/// they are stored, after the first (row 0): every eighth from 8, every
/// eighth from 4, every fourth from 2, every second from 1.
fn interlace_rows(height: u32) -> impl Iterator<Item = u32> {
    [(0u32, 8u32), (4, 8), (2, 4), (1, 2)]
        .into_iter()
        .flat_map(move |(start, step)| (start..height).step_by(step as usize))
        .skip(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interlaced_rows_in_stored_order() {
        let rows: Vec<u32> = core::iter::once(0).chain(interlace_rows(10)).collect();
        assert_eq!(rows, [0, 8, 4, 2, 6, 1, 3, 5, 7, 9]);
    }
}
