//! Built-in device profiles.
//!
//! A profile describes everything the emulator needs to behave like a
//! device: screen, pixel format, input hardware, buttons and the heap the
//! kernel may use. Values marked "unverified" are best guesses until checked
//! on real hardware.

use quireink_platform::{
    ColorPanel, DeviceInfo, DisplayInfo, InputCaps, PixelFormat, Rotation, buttons,
};

/// An emulated device.
#[derive(Debug, Clone, Copy)]
pub struct Profile {
    pub device: DeviceInfo,
    /// Physical buttons the device has; other keys are ignored.
    pub buttons: &'static [&'static str],
    /// Heap available to the kernel and plugins, if limited.
    pub heap_budget: Option<usize>,
    /// Memory for runtime-rasterized glyphs. The emulator always renders
    /// fonts at runtime; small devices use bitmap packs that need no cache,
    /// so their profiles keep it small to stay comparable.
    pub glyph_cache: usize,
    /// One-line description for `--list-profiles`.
    pub summary: &'static str,
}

const KIB: usize = 1024;

pub const PROFILES: &[Profile] = &[
    Profile {
        device: DeviceInfo {
            id: "xteink-x4",
            name: "Xteink X4",
            display: DisplayInfo {
                width: 480,
                height: 800,
                dpi: 220,
                // Unverified: 4 gray levels via the controller's LUTs.
                format: PixelFormat::Gray2,
                rotation: Rotation::Deg90,
                color: None,
            },
            input: InputCaps {
                buttons: true,
                touch: false,
                pen: false,
            },
        },
        buttons: &[
            buttons::PAGE_BACK,
            buttons::PAGE_FORWARD,
            buttons::LEFT,
            buttons::RIGHT,
            buttons::CONFIRM,
            buttons::BACK,
            buttons::POWER,
        ],
        // Unverified: ESP32-C3 SRAM left after firmware, stacks and drivers.
        heap_budget: Some(192 * KIB),
        glyph_cache: 16 * KIB,
        summary: "ESP32-C3, 4.3\" 800x480, buttons only, 192 KiB heap budget",
    },
    Profile {
        device: DeviceInfo {
            id: "boox-go-7",
            name: "Boox Go 7",
            display: DisplayInfo {
                width: 1264,
                height: 1680,
                dpi: 300,
                format: PixelFormat::Gray4,
                rotation: Rotation::Deg0,
                color: None,
            },
            input: InputCaps {
                buttons: true,
                touch: true,
                pen: false,
            },
        },
        buttons: &[buttons::PAGE_BACK, buttons::PAGE_FORWARD, buttons::POWER],
        heap_budget: None,
        glyph_cache: 256 * KIB,
        summary: "Android, 7\" Carta 1300 1264x1680, touch + page buttons",
    },
    Profile {
        device: DeviceInfo {
            id: "kobo-libra-colour",
            name: "Kobo Libra Colour",
            display: DisplayInfo {
                width: 1264,
                height: 1680,
                dpi: 300,
                format: PixelFormat::Rgb888,
                rotation: Rotation::Deg0,
                color: Some(ColorPanel {
                    color_divisor: 2,
                    saturation_percent: 55,
                }),
            },
            input: InputCaps {
                buttons: true,
                touch: true,
                pen: true,
            },
        },
        buttons: &[buttons::PAGE_BACK, buttons::PAGE_FORWARD, buttons::POWER],
        heap_budget: None,
        glyph_cache: 256 * KIB,
        summary: "Linux, 7\" Kaleido 3 colour 1264x1680, touch + page buttons",
    },
    Profile {
        device: DeviceInfo {
            id: "kindle-paperwhite-5",
            name: "Kindle Paperwhite (11th gen)",
            display: DisplayInfo {
                width: 1236,
                height: 1648,
                dpi: 300,
                format: PixelFormat::Gray4,
                rotation: Rotation::Deg0,
                color: None,
            },
            input: InputCaps {
                buttons: false,
                touch: true,
                pen: false,
            },
        },
        buttons: &[buttons::POWER],
        heap_budget: None,
        glyph_cache: 256 * KIB,
        summary: "Linux, 6.8\" 1236x1648, touch only",
    },
    Profile {
        device: DeviceInfo {
            id: "kobo-clara-bw",
            name: "Kobo Clara BW",
            display: DisplayInfo {
                width: 1072,
                height: 1448,
                dpi: 300,
                format: PixelFormat::Gray4,
                rotation: Rotation::Deg0,
                color: None,
            },
            input: InputCaps {
                buttons: false,
                touch: true,
                pen: false,
            },
        },
        buttons: &[buttons::POWER],
        heap_budget: None,
        glyph_cache: 256 * KIB,
        summary: "Linux, 6\" 1072x1448, touch only",
    },
];

pub fn find(id: &str) -> Option<&'static Profile> {
    PROFILES.iter().find(|p| p.device.id == id)
}
