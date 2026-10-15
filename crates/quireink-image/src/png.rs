//! Non-interlaced PNG, inflated and unfiltered one row at a time.

use alloc::vec::Vec;

use miniz_oxide::inflate::TINFLStatus;
use miniz_oxide::inflate::core::{DecompressorOxide, decompress, inflate_flags};

use crate::{Downscaler, ImageError, ImageSink, Reader, factor_for, zeroed};

/// The deflate history.
const WINDOW: usize = 32 * 1024;

struct Header {
    width: u32,
    height: u32,
    depth: u8,
    color: u8,
}

impl Header {
    fn channels(&self) -> usize {
        match self.color {
            0 | 3 => 1,
            4 => 2,
            2 => 3,
            _ => 4,
        }
    }

    /// Bytes per row without the filter byte.
    fn row_bytes(&self) -> usize {
        (self.width as usize * self.channels() * usize::from(self.depth)).div_ceil(8)
    }

    /// Bytes per complete pixel for filtering (at least 1).
    fn pixel_bytes(&self) -> usize {
        (self.channels() * usize::from(self.depth))
            .div_ceil(8)
            .max(1)
    }
}

fn luma(r: u32, g: u32, b: u32) -> u32 {
    (r * 299 + g * 587 + b * 114 + 500) / 1000
}

/// Turns PNG rows into gray rows.
struct Rows<'s> {
    header: Header,
    /// Gray of each palette entry, over white.
    palette: [u8; 256],
    previous: Vec<u8>,
    current: Vec<u8>,
    filled: usize,
    gray: Vec<u8>,
    done: u32,
    scaler: Downscaler,
    sink: &'s mut dyn ImageSink,
}

impl Rows<'_> {
    fn push(&mut self, mut data: &[u8]) -> Result<(), ImageError> {
        let row_len = self.header.row_bytes() + 1;
        while !data.is_empty() && self.done < self.header.height {
            let take = (row_len - self.filled).min(data.len());
            self.current[self.filled..self.filled + take].copy_from_slice(&data[..take]);
            self.filled += take;
            data = &data[take..];
            if self.filled == row_len {
                self.unfilter()?;
                self.convert();
                self.scaler.row(&self.gray, self.sink)?;
                core::mem::swap(&mut self.previous, &mut self.current);
                self.filled = 0;
                self.done += 1;
            }
        }
        Ok(())
    }

    fn unfilter(&mut self) -> Result<(), ImageError> {
        let bpp = self.header.pixel_bytes();
        let (filter, row) = self
            .current
            .split_first_mut()
            .ok_or(ImageError::Invalid("empty row"))?;
        let prev = &self.previous[1..];
        match *filter {
            0 => {}
            1 => {
                for i in bpp..row.len() {
                    row[i] = row[i].wrapping_add(row[i - bpp]);
                }
            }
            2 => {
                for i in 0..row.len() {
                    row[i] = row[i].wrapping_add(prev[i]);
                }
            }
            3 => {
                for i in 0..row.len() {
                    let left = if i >= bpp { u16::from(row[i - bpp]) } else { 0 };
                    row[i] = row[i].wrapping_add(((left + u16::from(prev[i])) / 2) as u8);
                }
            }
            4 => {
                for i in 0..row.len() {
                    let a = if i >= bpp { i16::from(row[i - bpp]) } else { 0 };
                    let b = i16::from(prev[i]);
                    let c = if i >= bpp {
                        i16::from(prev[i - bpp])
                    } else {
                        0
                    };
                    let p = a + b - c;
                    let (pa, pb, pc) = ((p - a).abs(), (p - b).abs(), (p - c).abs());
                    let predictor = if pa <= pb && pa <= pc {
                        a
                    } else if pb <= pc {
                        b
                    } else {
                        c
                    };
                    row[i] = row[i].wrapping_add(predictor as u8);
                }
            }
            _ => return Err(ImageError::Invalid("bad PNG filter")),
        }
        Ok(())
    }

    /// Sample `index` of the row, scaled to 0 to 255.
    fn sample(row: &[u8], depth: u8, index: usize) -> u32 {
        match depth {
            8 => u32::from(row[index]),
            16 => u32::from(row[index * 2]),
            bits => {
                let bit = index * usize::from(bits);
                let byte = row[bit / 8];
                let shift = 8 - usize::from(bits) - bit % 8;
                let max = (1u32 << bits) - 1;
                let value = u32::from(byte >> shift) & max;
                value * 255 / max
            }
        }
    }

    fn convert(&mut self) {
        let h = &self.header;
        let row = &self.current[1..];
        let channels = h.channels();
        for x in 0..h.width as usize {
            let s = |c: usize| Self::sample(row, h.depth, x * channels + c);
            let over_white = |v: u32, alpha: u32| (v * alpha + 255 * (255 - alpha)) / 255;
            self.gray[x] = match h.color {
                0 => s(0),
                2 => luma(s(0), s(1), s(2)),
                3 => {
                    let index = if h.depth == 8 {
                        u32::from(row[x])
                    } else {
                        let bit = x * usize::from(h.depth);
                        let shift = 8 - usize::from(h.depth) - bit % 8;
                        u32::from(row[bit / 8] >> shift) & ((1 << h.depth) - 1)
                    };
                    u32::from(self.palette[index as usize])
                }
                4 => over_white(s(0), s(1)),
                _ => over_white(luma(s(0), s(1), s(2)), s(3)),
            } as u8;
        }
    }
}

pub(crate) fn decode(
    reader: &mut Reader<'_>,
    max_side: u32,
    sink: &mut dyn ImageSink,
) -> Result<(), ImageError> {
    let mut signature = [0u8; 8];
    reader.fill(&mut signature)?;
    if &signature != b"\x89PNG\r\n\x1a\n" {
        return Err(ImageError::Invalid("not a PNG"));
    }
    let mut header: Option<Header> = None;
    let mut palette = [255u8; 256];
    let mut alpha = [255u8; 256];
    let mut colors = [(0u8, 0u8, 0u8); 256];
    let mut rows: Option<Rows<'_>> = None;
    let mut sink = Some(sink);
    // Inflate state, created at the first IDAT chunk.
    let mut decoder: Vec<DecompressorOxide> = Vec::new();
    let mut window: Vec<u8> = Vec::new();
    let mut position = 0usize;
    let mut zlib_header = 2usize;
    let mut chunk = [0u8; 1024];

    loop {
        let len = reader.u32_be()? as usize;
        let mut kind = [0u8; 4];
        reader.fill(&mut kind)?;
        match &kind {
            b"IHDR" => {
                let width = reader.u32_be()?;
                let height = reader.u32_be()?;
                let depth = reader.byte()?;
                let color = reader.byte()?;
                let _compression = reader.byte()?;
                let _filter = reader.byte()?;
                let interlace = reader.byte()?;
                reader.skip(len.saturating_sub(13))?;
                let valid = matches!(
                    (color, depth),
                    (0, 1 | 2 | 4 | 8 | 16) | (3, 1 | 2 | 4 | 8) | (2 | 4 | 6, 8 | 16)
                );
                if width == 0 || height == 0 || !valid {
                    return Err(ImageError::Invalid("bad PNG header"));
                }
                if interlace != 0 {
                    return Err(ImageError::Unsupported("interlaced PNG"));
                }
                if width > 1 << 16 || height > 1 << 16 {
                    return Err(ImageError::Unsupported("PNG too large"));
                }
                header = Some(Header {
                    width,
                    height,
                    depth,
                    color,
                });
            }
            b"PLTE" => {
                for color in colors.iter_mut().take(len / 3) {
                    *color = (reader.byte()?, reader.byte()?, reader.byte()?);
                }
                reader.skip(len % 3 + len.saturating_sub(256 * 3))?;
            }
            b"tRNS" if header.as_ref().is_some_and(|h| h.color == 3) => {
                for a in alpha.iter_mut().take(len) {
                    *a = reader.byte()?;
                }
                reader.skip(len.saturating_sub(256))?;
            }
            b"IDAT" => {
                if rows.is_none() {
                    let h = header
                        .take()
                        .ok_or(ImageError::Invalid("IDAT before IHDR"))?;
                    for (i, &(r, g, b)) in colors.iter().enumerate() {
                        let a = u32::from(alpha[i]);
                        let y = luma(u32::from(r), u32::from(g), u32::from(b));
                        palette[i] = ((y * a + 255 * (255 - a)) / 255) as u8;
                    }
                    let factor = factor_for(h.width, h.height, max_side);
                    let scaler = Downscaler::new(h.width, factor)?;
                    let sink = sink.take().ok_or(ImageError::Invalid("state"))?;
                    sink.start(scaler.out_width(), h.height.div_ceil(factor))?;
                    let row_len = h.row_bytes() + 1;
                    rows = Some(Rows {
                        palette,
                        previous: zeroed(row_len)?,
                        current: zeroed(row_len)?,
                        filled: 0,
                        gray: zeroed(h.width as usize)?,
                        done: 0,
                        scaler,
                        sink,
                        header: h,
                    });
                    decoder
                        .try_reserve_exact(1)
                        .map_err(|_| ImageError::OutOfMemory)?;
                    decoder.push(DecompressorOxide::new());
                    window = zeroed(WINDOW)?;
                }
                let Some(state) = rows.as_mut() else {
                    return Err(ImageError::Invalid("state"));
                };
                let mut left = len;
                while left > 0 {
                    let n = left.min(chunk.len());
                    reader.fill(&mut chunk[..n])?;
                    left -= n;
                    let mut input = &chunk[..n];
                    // The 2-byte zlib header precedes the deflate data.
                    let skip = zlib_header.min(input.len());
                    input = &input[skip..];
                    zlib_header -= skip;
                    while !input.is_empty() {
                        let (status, used, written) = decompress(
                            &mut decoder[0],
                            input,
                            &mut window,
                            position,
                            inflate_flags::TINFL_FLAG_HAS_MORE_INPUT,
                        );
                        input = &input[used..];
                        state.push(&window[position..position + written])?;
                        position = (position + written) & (WINDOW - 1);
                        match status {
                            TINFLStatus::Done => {
                                input = &[];
                            }
                            TINFLStatus::HasMoreOutput | TINFLStatus::NeedsMoreInput => {}
                            _ => return Err(ImageError::Invalid("corrupt PNG data")),
                        }
                        if used == 0 && written == 0 && status != TINFLStatus::HasMoreOutput {
                            break;
                        }
                    }
                }
            }
            b"IEND" => {
                let state = rows.as_mut().ok_or(ImageError::Invalid("no image data"))?;
                // Flush what the decoder still holds.
                loop {
                    let (status, _, written) =
                        decompress(&mut decoder[0], &[], &mut window, position, 0);
                    state.push(&window[position..position + written])?;
                    position = (position + written) & (WINDOW - 1);
                    if written == 0 || status != TINFLStatus::HasMoreOutput {
                        break;
                    }
                }
                if state.done < state.header.height {
                    return Err(ImageError::Invalid("truncated PNG data"));
                }
                return state.scaler.finish(state.sink);
            }
            _ => reader.skip(len)?,
        }
        // CRC
        reader.skip(4)?;
    }
}
