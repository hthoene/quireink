//! Converting frames to the window's RGBA pixels, in the colours of the
//! settings (`Kernel::colors`).

use quireink_platform::FrameView;

/// What each gray of a frame is shown as: black as the text colour, white
/// as the paper colour, the grays between them in between.
pub fn tint(colors: [[u8; 3]; 2]) -> [[u8; 4]; 256] {
    let [ink, paper] = colors;
    core::array::from_fn(|gray| {
        let mix = |i: usize| {
            let (from, to) = (i32::from(ink[i]), i32::from(paper[i]));
            (from + (to - from) * gray as i32 / 255) as u8
        };
        [mix(0), mix(1), mix(2), 0xff]
    })
}

/// Converts `frame` to RGBA_8888 for a window of `stride` pixels per row,
/// placing it at row `top`, each gray as `tint` shows it: `put(byte
/// offset, rgba)` writes one pixel. Pixels wider than the window are
/// skipped.
pub fn to_rgba(
    frame: &FrameView<'_>,
    top: usize,
    stride: usize,
    tint: &[[u8; 4]; 256],
    mut put: impl FnMut(usize, [u8; 4]),
) {
    for y in 0..usize::from(frame.height) {
        let row = (top + y) * stride * 4;
        for x in 0..usize::from(frame.width).min(stride) {
            let color = frame.pixel(x as u16, y as u16);
            put(row + x * 4, tint[usize::from(color.luma())]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quireink_platform::PixelFormat;

    const BLACK_ON_WHITE: [[u8; 3]; 2] = [[0, 0, 0], [255, 255, 255]];

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
        to_rgba(&frame, 1, 3, &tint(BLACK_ON_WHITE), |at, rgba| {
            buffer[at..at + 4].copy_from_slice(&rgba)
        });
        assert_eq!(&buffer[12..20], &[0, 0, 0, 255, 255, 255, 255, 255]);
        assert_eq!(&buffer[24..28], &[128, 128, 128, 255]);
        // Row 0 is untouched, as is the third column.
        assert_eq!(&buffer[0..4], &[7, 7, 7, 7]);
        assert_eq!(&buffer[20..24], &[7, 7, 7, 7]);
    }

    #[test]
    fn grays_lie_between_the_colours() {
        let sepia = tint([[91, 70, 54], [244, 236, 216]]);
        assert_eq!(sepia[0], [91, 70, 54, 255]);
        assert_eq!(sepia[255], [244, 236, 216, 255]);
        let middle = sepia[128];
        assert!((91..244).contains(&middle[0]) && (54..216).contains(&middle[2]));
        // Black on white leaves the grays as they are.
        assert_eq!(tint(BLACK_ON_WHITE)[77], [77, 77, 77, 255]);
    }
}
