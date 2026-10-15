//! Simulates how an e-ink panel shows the kernel's framebuffer.
//!
//! - Gray levels are mapped between "ink" and "paper" colours.
//! - Colour panels lose saturation and colour resolution (the colour filter
//!   covers several pixels), while gray detail stays sharp.
//! - Fast refreshes show only black and white, like A2/DU waveforms.
//! - Full refreshes flash (black, then white, then the new content).

use std::time::{Duration, Instant};

use quireink_platform::{ColorPanel, FrameView, Rect, RefreshMode};

const INK: [u8; 3] = [0x1e, 0x1e, 0x1e];
const PAPER: [u8; 3] = [0xe9, 0xe6, 0xdd];
/// Duration of each flash phase of a full refresh.
const FLASH_PHASE: Duration = Duration::from_millis(120);
/// How long the refresh-region overlay stays visible.
const HIGHLIGHT: Duration = Duration::from_millis(600);
const HIGHLIGHT_COLOR: u32 = 0x00d0_3030;

/// The simulated panel state: what the e-ink currently shows.
#[derive(Debug)]
pub struct Panel {
    width: u16,
    height: u16,
    color: Option<ColorPanel>,
    /// Shown pixels as `0x00RRGGBB`.
    pixels: Vec<u32>,
    flash: Option<(Rect, Instant)>,
    highlight: Option<(Rect, Instant)>,
    /// Show refreshed regions with an outline.
    pub show_regions: bool,
    /// Simulate flashing full refreshes.
    pub animate: bool,
}

impl Panel {
    pub fn new(width: u16, height: u16, color: Option<ColorPanel>) -> Self {
        Self {
            width,
            height,
            color,
            pixels: vec![rgb(PAPER); usize::from(width) * usize::from(height)],
            flash: None,
            highlight: None,
            show_regions: false,
            animate: true,
        }
    }

    pub fn width(&self) -> u16 {
        self.width
    }

    pub fn height(&self) -> u16 {
        self.height
    }

    /// Applies a refresh of `region` with the content of `frame`.
    pub fn refresh(&mut self, frame: FrameView<'_>, region: Rect, mode: RefreshMode) {
        let Some(region) = region.intersection(&Rect::from_size(self.width, self.height)) else {
            return;
        };
        let divisor = self.color.map_or(1, |c| u16::from(c.color_divisor.max(1)));
        for y in region.y..region.bottom() as u16 {
            for x in region.x..region.right() as u16 {
                let c = frame.pixel(x, y);
                let mut value = [c.r, c.g, c.b];
                if let Some(panel) = self.color {
                    // Keep per-pixel luminance, take colour from the filter cell.
                    let cell = average(&frame, x - x % divisor, y - y % divisor, divisor);
                    value = color_filter(c.luma(), cell, panel.saturation_percent);
                }
                if mode == RefreshMode::Fast {
                    let luma = c.luma();
                    value = if luma >= 128 { [255; 3] } else { [0; 3] };
                }
                self.pixels[usize::from(y) * usize::from(self.width) + usize::from(x)] =
                    on_paper(value);
            }
        }
        let now = Instant::now();
        if mode == RefreshMode::Full && self.animate {
            self.flash = Some((region, now));
        }
        self.highlight = Some((region, now));
    }

    /// Whether an animation or overlay still needs frames.
    pub fn is_animating(&self) -> bool {
        let now = Instant::now();
        let flashing = self
            .flash
            .is_some_and(|(_, start)| now - start < FLASH_PHASE * 2);
        let highlighted = self.show_regions
            && self
                .highlight
                .is_some_and(|(_, start)| now - start < HIGHLIGHT);
        flashing || highlighted
    }

    /// The panel as it currently looks, including flash and overlay.
    pub fn compose(&self) -> Vec<u32> {
        let mut out = self.pixels.clone();
        let now = Instant::now();
        if let Some((region, start)) = self.flash {
            let elapsed = now - start;
            let fill = if elapsed < FLASH_PHASE {
                Some(rgb(INK))
            } else if elapsed < FLASH_PHASE * 2 {
                Some(rgb(PAPER))
            } else {
                None
            };
            if let Some(fill) = fill {
                self.fill(&mut out, region, fill);
            }
        }
        if self.show_regions
            && let Some((region, start)) = self.highlight
            && now - start < HIGHLIGHT
        {
            self.outline(&mut out, region, HIGHLIGHT_COLOR);
        }
        out
    }

    /// The shown pixels as RGB bytes, without animation (for screenshots).
    pub fn rgb_bytes(&self) -> Vec<u8> {
        self.pixels
            .iter()
            .flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8])
            .collect()
    }

    fn fill(&self, out: &mut [u32], region: Rect, value: u32) {
        for y in region.y..region.bottom() as u16 {
            let row = usize::from(y) * usize::from(self.width);
            out[row + usize::from(region.x)..row + region.right() as usize].fill(value);
        }
    }

    fn outline(&self, out: &mut [u32], region: Rect, value: u32) {
        let t = 3u16.min(region.width / 2).min(region.height / 2).max(1);
        let edges = [
            Rect::new(region.x, region.y, region.width, t),
            Rect::new(region.x, region.bottom() as u16 - t, region.width, t),
            Rect::new(region.x, region.y, t, region.height),
            Rect::new(region.right() as u16 - t, region.y, t, region.height),
        ];
        for edge in edges {
            self.fill(out, edge, value);
        }
    }
}

fn rgb([r, g, b]: [u8; 3]) -> u32 {
    (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)
}

/// Maps a colour onto the range from ink to paper of a real panel.
fn on_paper(value: [u8; 3]) -> u32 {
    let channel = |i: usize| {
        let (ink, paper) = (u32::from(INK[i]), u32::from(PAPER[i]));
        (ink + (paper - ink) * u32::from(value[i]) / 255) as u8
    };
    rgb([channel(0), channel(1), channel(2)])
}

fn average(frame: &FrameView<'_>, x0: u16, y0: u16, size: u16) -> [u8; 3] {
    let mut sum = [0u32; 3];
    let mut count = 0u32;
    for y in y0..y0.saturating_add(size).min(frame.height) {
        for x in x0..x0.saturating_add(size).min(frame.width) {
            let c = frame.pixel(x, y);
            sum[0] += u32::from(c.r);
            sum[1] += u32::from(c.g);
            sum[2] += u32::from(c.b);
            count += 1;
        }
    }
    let count = count.max(1);
    [
        (sum[0] / count) as u8,
        (sum[1] / count) as u8,
        (sum[2] / count) as u8,
    ]
}

/// Combines a pixel's luminance with the (desaturated) colour of its
/// filter cell.
fn color_filter(luma: u8, cell: [u8; 3], saturation_percent: u8) -> [u8; 3] {
    let cell_luma =
        (u32::from(cell[0]) * 299 + u32::from(cell[1]) * 587 + u32::from(cell[2]) * 114) / 1000;
    let saturation = i32::from(saturation_percent.min(100));
    cell.map(|c| {
        let chroma = (i32::from(c) - cell_luma as i32) * saturation / 100;
        (i32::from(luma) + chroma).clamp(0, 255) as u8
    })
}
