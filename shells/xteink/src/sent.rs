//! The bands of a frame as they were sent to the panel, kept in runs of
//! bytes (pages are mostly paper): after a fast refresh the controller
//! needs the frame now shown in its memory for the next one, and sending
//! these again takes milliseconds where drawing the frame anew takes
//! seconds. A frame too varied to keep in [`MAX_BYTES`], or with memory
//! short, is drawn anew instead.

use alloc::vec::Vec;

use quireink_platform::FrameView;

/// What the kept bands may take of the heap.
const MAX_BYTES: usize = 32 * 1024;

/// A band: its first row, its rows, its format and bytes per row, and its
/// bytes in runs (a count from 1 to 255, then the byte).
struct Band {
    top: u16,
    view: (u16, u16, quireink_platform::PixelFormat, usize),
    runs: Vec<u8>,
}

#[derive(Default)]
pub struct Sent {
    bands: Vec<Band>,
    size: usize,
    /// It did not fit: the frame is drawn anew.
    lost: bool,
}

impl Sent {
    /// Keeps a band as it is sent.
    pub fn keep(&mut self, top: u16, rows: &FrameView<'_>) {
        if self.lost {
            return;
        }
        let bytes = &rows.data[..(usize::from(rows.height) * rows.stride).min(rows.data.len())];
        let mut runs = Vec::new();
        let mut rest = bytes;
        while let Some(&byte) = rest.first() {
            let count = rest.iter().take(255).take_while(|b| **b == byte).count();
            if self.size + runs.len() + 2 > MAX_BYTES || runs.try_reserve(2).is_err() {
                return self.lose();
            }
            runs.push(count as u8);
            runs.push(byte);
            rest = &rest[count..];
        }
        runs.shrink_to_fit();
        self.size += runs.len();
        if self.bands.try_reserve(1).is_err() {
            return self.lose();
        }
        self.bands.push(Band {
            top,
            view: (rows.width, rows.height, rows.format, rows.stride),
            runs,
        });
    }

    fn lose(&mut self) {
        self.lost = true;
        self.bands = Vec::new();
        self.size = 0;
    }

    /// Passes every band kept, as it was sent; `false` if they were not
    /// all kept (the frame is to be drawn anew).
    pub fn replay(&self, out: &mut dyn FnMut(u16, FrameView<'_>)) -> bool {
        if self.lost {
            return false;
        }
        let mut data = Vec::new();
        for band in &self.bands {
            let (width, height, format, stride) = band.view;
            data.clear();
            if data.try_reserve(usize::from(height) * stride).is_err() {
                return false;
            }
            for pair in band.runs.as_chunks::<2>().0 {
                data.extend(core::iter::repeat_n(pair[1], usize::from(pair[0])));
            }
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
