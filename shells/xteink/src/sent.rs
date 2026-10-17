//! The bands of a frame as they were sent to the panel, kept packed (runs
//! of a byte, and bytes as they are between): after every refresh the
//! controller needs the frame now shown in both its memories, and sending
//! these again takes milliseconds where drawing the frame anew takes
//! seconds. Pages of text pack to a few kilobytes; pictures, dithered,
//! hardly pack, but take no more than the frame (48 KB) either. A frame
//! with memory short is drawn anew instead.

use alloc::vec::Vec;

use quireink_platform::FrameView;

/// What the kept bands may take of the heap: a whole frame, packed.
const MAX_BYTES: usize = 56 * 1024;

/// What is left free of the heap for the rest of the frame's drawing.
const SPARE: usize = 40 * 1024;

/// A band: its first row, its rows, its format and bytes per row, and its
/// bytes packed: a byte below 0x80 says that many bytes and one more
/// follow as they are; one from 0x80 on, that the next byte comes as
/// many times as its low 7 bits say, and three more.
struct Band {
    top: u16,
    view: (u16, u16, quireink_platform::PixelFormat, usize),
    packed: Vec<u8>,
}

#[derive(Default)]
pub struct Sent {
    bands: Vec<Band>,
    size: usize,
    /// A band packed, before it is kept at its size.
    scratch: Vec<u8>,
    /// It did not fit: the frame is drawn anew.
    lost: bool,
}

/// Packs `bytes` onto `out`.
fn pack(bytes: &[u8], out: &mut Vec<u8>) {
    let mut at = 0;
    while at < bytes.len() {
        let byte = bytes[at];
        let run = bytes[at..]
            .iter()
            .take(130)
            .take_while(|&&b| b == byte)
            .count();
        // Runs of three on (shorter ones cost more as runs than as bytes
        // between them).
        if run >= 3 {
            out.push(0x80 | (run - 3) as u8);
            out.push(byte);
            at += run;
        } else {
            // As they are, until a run starts.
            let start = at;
            at += 1;
            while at < bytes.len() && at - start < 128 && !run_at(bytes, at) {
                at += 1;
            }
            out.push((at - start - 1) as u8);
            out.extend_from_slice(&bytes[start..at]);
        }
    }
}

/// Whether a run of three starts at `at`.
fn run_at(bytes: &[u8], at: usize) -> bool {
    bytes.get(at + 1) == Some(&bytes[at]) && bytes.get(at + 2) == Some(&bytes[at])
}

/// Unpacks `packed` onto `out`.
fn unpack(packed: &[u8], out: &mut Vec<u8>) {
    let mut at = 0;
    while let Some(&head) = packed.get(at) {
        if head < 0x80 {
            let end = (at + 2 + usize::from(head)).min(packed.len());
            out.extend_from_slice(&packed[at + 1..end]);
            at = end;
        } else {
            let byte = packed.get(at + 1).copied().unwrap_or(0xFF);
            out.extend(core::iter::repeat_n(byte, usize::from(head & 0x7F) + 3));
            at += 2;
        }
    }
}

impl Sent {
    /// Keeps a band as it is sent.
    pub fn keep(&mut self, top: u16, rows: &FrameView<'_>) {
        if self.lost {
            return;
        }
        let bytes = &rows.data[..(usize::from(rows.height) * rows.stride).min(rows.data.len())];
        // At most a byte more for every 128.
        self.scratch.clear();
        if self
            .scratch
            .try_reserve(bytes.len() + bytes.len() / 128 + 1)
            .is_err()
        {
            return self.lose();
        }
        pack(bytes, &mut self.scratch);
        let size = self.scratch.len();
        let mut packed = Vec::new();
        if self.size + size > MAX_BYTES
            || esp_alloc::HEAP.free() < size + SPARE
            || packed.try_reserve_exact(size).is_err()
            || self.bands.try_reserve(1).is_err()
        {
            return self.lose();
        }
        packed.extend_from_slice(&self.scratch);
        self.size += size;
        self.bands.push(Band {
            top,
            view: (rows.width, rows.height, rows.format, rows.stride),
            packed,
        });
    }

    fn lose(&mut self) {
        self.lost = true;
        self.bands = Vec::new();
        self.scratch = Vec::new();
        self.size = 0;
    }

    /// Passes every band kept, as it was sent; `false` if they were not
    /// all kept (the frame is to be drawn anew).
    pub fn replay(&mut self, out: &mut dyn FnMut(u16, FrameView<'_>)) -> bool {
        if self.lost {
            return false;
        }
        // The scratch band's memory, for unpacking.
        let mut data = core::mem::take(&mut self.scratch);
        for band in &self.bands {
            let (width, height, format, stride) = band.view;
            data.clear();
            if data.try_reserve(usize::from(height) * stride).is_err() {
                return false;
            }
            unpack(&band.packed, &mut data);
            out(
                band.top,
                FrameView {
                    width,
                    height,
                    format,
                    stride,
                    data: &data,
                },
            );
        }
        true
    }
}
