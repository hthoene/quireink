//! Converting frames to the window's RGBA pixels.

use quireink_platform::FrameView;

/// Converts `frame` to RGBA_8888 for a window of `stride` pixels per row,
/// placing it at row `top`: `put(byte offset, rgba)` writes one pixel.
/// Pixels wider than the window are skipped.
pub fn to_rgba(
    frame: &FrameView<'_>,
    top: usize,
    stride: usize,
    mut put: impl FnMut(usize, [u8; 4]),
) {
    for y in 0..usize::from(frame.height) {
        let row = (top + y) * stride * 4;
        for x in 0..usize::from(frame.width).min(stride) {
            let color = frame.pixel(x as u16, y as u16);
            put(row + x * 4, [color.r, color.g, color.b, 0xff]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quireink_platform::PixelFormat;

    #[test]
    fn gray_becomes_opaque_rgba() {
        // Two Gray8 rows of two pixels: black, white / gray, black.
        let data = [0u8, 255, 128, 0];
        let frame = FrameView {
            width: 2,
            height: 2,
            format: PixelFormat::Gray8,
            stride: 2,
            data: &data,
        };
        // A window three pixels wide, written from row 1.
        let mut buffer = [7u8; 3 * 3 * 4];
        to_rgba(&frame, 1, 3, |at, rgba| {
            buffer[at..at + 4].copy_from_slice(&rgba)
        });
        assert_eq!(&buffer[12..20], &[0, 0, 0, 255, 255, 255, 255, 255]);
        assert_eq!(&buffer[24..28], &[128, 128, 128, 255]);
        // Row 0 is untouched, as is the third column.
        assert_eq!(&buffer[0..4], &[7, 7, 7, 7]);
        assert_eq!(&buffer[20..24], &[7, 7, 7, 7]);
    }
}
