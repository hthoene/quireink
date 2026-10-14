//! Pixel formats and colours.

/// The memory layout of a framebuffer.
///
/// Packed grayscale formats store pixels most-significant-bit first, rows are
/// padded to whole bytes. A value of `0` is black, the maximum value is white.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PixelFormat {
    /// 1 bit per pixel, black and white.
    Mono1,
    /// 2 bits per pixel, 4 gray levels.
    Gray2,
    /// 4 bits per pixel, 16 gray levels (typical e-ink panel depth).
    Gray4,
    /// 8 bits per pixel, 256 gray levels.
    Gray8,
    /// 16 bits per pixel colour, little endian, 5-6-5.
    Rgb565,
    /// 24 bits per pixel colour, byte order R, G, B.
    Rgb888,
}

impl PixelFormat {
    pub const fn bits_per_pixel(self) -> u8 {
        match self {
            PixelFormat::Mono1 => 1,
            PixelFormat::Gray2 => 2,
            PixelFormat::Gray4 => 4,
            PixelFormat::Gray8 => 8,
            PixelFormat::Rgb565 => 16,
            PixelFormat::Rgb888 => 24,
        }
    }

    pub const fn is_color(self) -> bool {
        matches!(self, PixelFormat::Rgb565 | PixelFormat::Rgb888)
    }

    /// Number of distinct gray levels, or `None` for colour formats.
    pub const fn gray_levels(self) -> Option<u16> {
        match self {
            PixelFormat::Mono1 => Some(2),
            PixelFormat::Gray2 => Some(4),
            PixelFormat::Gray4 => Some(16),
            PixelFormat::Gray8 => Some(256),
            PixelFormat::Rgb565 | PixelFormat::Rgb888 => None,
        }
    }

    /// Bytes needed for one row of `width` pixels.
    pub const fn stride(self, width: u16) -> usize {
        (width as usize * self.bits_per_pixel() as usize).div_ceil(8)
    }
}

/// An sRGB colour. Grayscale devices use its luminance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const BLACK: Color = Color::gray(0);
    pub const WHITE: Color = Color::gray(255);

    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    pub const fn gray(value: u8) -> Self {
        Self::rgb(value, value, value)
    }

    /// Perceived brightness (ITU-R BT.601 weights, integer arithmetic).
    pub const fn luma(self) -> u8 {
        ((self.r as u32 * 299 + self.g as u32 * 587 + self.b as u32 * 114 + 500) / 1000) as u8
    }

    pub const fn inverted(self) -> Self {
        Self::rgb(255 - self.r, 255 - self.g, 255 - self.b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stride_rounds_up_to_bytes() {
        assert_eq!(PixelFormat::Mono1.stride(9), 2);
        assert_eq!(PixelFormat::Gray2.stride(5), 2);
        assert_eq!(PixelFormat::Gray4.stride(3), 2);
        assert_eq!(PixelFormat::Rgb888.stride(3), 9);
    }

    #[test]
    fn luma_extremes() {
        assert_eq!(Color::BLACK.luma(), 0);
        assert_eq!(Color::WHITE.luma(), 255);
        assert_eq!(Color::gray(128).luma(), 128);
    }
}
