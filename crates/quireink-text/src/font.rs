use alloc::borrow::Cow;
use core::fmt;

use crate::bitmap::BitmapFont;
#[cfg(feature = "outline")]
use crate::outline::OutlineFont;

/// Why a font could not be loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FontError {
    /// Not a valid font file or font pack.
    Invalid,
    /// The font has no character map.
    NoCharmap,
    /// A font pack of a newer format version.
    UnsupportedVersion,
    /// Scalable fonts need the `outline` feature, which this build lacks.
    OutlinesNotSupported,
}

impl fmt::Display for FontError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            FontError::Invalid => "not a valid font or font pack",
            FontError::NoCharmap => "font has no character map",
            FontError::UnsupportedVersion => "font pack version not supported",
            FontError::OutlinesNotSupported => "this build only supports bitmap font packs (.qifp)",
        })
    }
}

impl core::error::Error for FontError {}

/// Vertical metrics of a font at a size, in pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineMetrics {
    /// Distance from the baseline to the top of the line (positive).
    pub ascent: f32,
    /// Distance from the baseline to the bottom of the line (positive).
    pub descent: f32,
    /// Recommended distance between two baselines.
    pub line_height: f32,
}

impl LineMetrics {
    /// Rough metrics when a font cannot provide any.
    pub(crate) fn fallback(size: f32) -> Self {
        Self {
            ascent: size * 0.8,
            descent: size * 0.2,
            line_height: size * 1.2,
        }
    }
}

/// A font: a pre-rendered bitmap pack or (with the `outline` feature) a
/// scalable OpenType/TrueType font.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Font {
    Bitmap(BitmapFont),
    #[cfg(feature = "outline")]
    Outline(OutlineFont),
}

impl Font {
    /// Loads a font pack or, if supported, an OpenType/TrueType font.
    /// `variations` applies to variable fonts, e.g. `[("wght", 700.0)]`.
    pub fn from_data(
        data: impl Into<Cow<'static, [u8]>>,
        variations: &[(&str, f32)],
    ) -> Result<Font, FontError> {
        let data = data.into();
        if BitmapFont::is_pack(&data) {
            return BitmapFont::new(data).map(Font::Bitmap);
        }
        #[cfg(feature = "outline")]
        {
            OutlineFont::new(data, variations).map(Font::Outline)
        }
        #[cfg(not(feature = "outline"))]
        {
            let _ = variations;
            Err(FontError::OutlinesNotSupported)
        }
    }

    pub fn line_metrics(&self, size: f32) -> LineMetrics {
        match self {
            Font::Bitmap(font) => font.line_metrics(size),
            #[cfg(feature = "outline")]
            Font::Outline(font) => font.line_metrics(size),
        }
    }

    pub fn advance(&self, ch: char, size: f32) -> f32 {
        match self {
            Font::Bitmap(font) => font.advance(ch, size),
            #[cfg(feature = "outline")]
            Font::Outline(font) => font.advance(ch, size),
        }
    }

    pub fn has_char(&self, ch: char) -> bool {
        match self {
            Font::Bitmap(font) => font.has_char(ch),
            #[cfg(feature = "outline")]
            Font::Outline(font) => font.has_char(ch),
        }
    }
}
