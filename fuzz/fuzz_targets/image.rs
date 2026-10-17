//! Any bytes as a JPEG, PNG or GIF, decoded to gray at a small size.

#![no_main]

use libfuzzer_sys::fuzz_target;
use quireink_image::{Fit, Format, ImageError, ImageSink, decode};

struct Count(u64);

impl ImageSink for Count {
    fn start(&mut self, width: u32, height: u32) -> Result<(), ImageError> {
        assert!(width > 0 && height > 0);
        Ok(())
    }

    fn row(&mut self, gray: &[u8]) -> Result<(), ImageError> {
        self.0 += gray.len() as u64;
        Ok(())
    }
}

fuzz_target!(|data: &[u8]| {
    for format in [Format::Jpeg, Format::Png, Format::Gif] {
        let mut source = data;
        let _ = decode(format, &mut source, Fit::side(64), &mut Count(0));
    }
});
