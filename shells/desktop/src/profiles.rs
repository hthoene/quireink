//! Built-in device profiles.
//!
//! A profile describes everything the emulator needs to behave like a
//! device: screen, pixel format, input hardware, buttons and the heap the
//! kernel may use. Values marked "unverified" are best guesses until checked
//! on real hardware.

use quireink_platform::{
    ColorPanel, DeviceInfo, DisplayInfo, InputCaps, MemoryClass, PixelFormat, Rotation, buttons,
};

/// An emulated device.
#[derive(Debug, Clone, Copy)]
pub struct Profile {
    pub device: DeviceInfo,
    /// Physical buttons the device has; other keys are ignored.
    pub buttons: &'static [&'static str],
    /// Heap available to the kernel and plugins, if limited.
    pub heap_budget: Option<usize>,
    /// Memory for runtime-rasterized glyphs.
    pub glyph_cache: usize,
    /// Use bitmap font packs read in place, like small devices without
    /// runtime font rendering; otherwise scalable fonts.
    pub font_packs: bool,
    /// Draw frames in bands of this many rows (see
    /// `KernelBuilder::frame_rows`); `None` keeps the whole frame in RAM.
    pub frame_rows: Option<u16>,
    /// Device-specific setting defaults, as `(id, value)`.
    pub settings: &'static [(&'static str, &'static str)],
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
            memory: MemoryClass::Small,
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
        glyph_cache: 0,
        font_packs: true,
        // 100 of 800 rows: 11.7 KiB instead of 94 KiB, eight bands a frame.
        frame_rows: Some(100),
        // A 4.3" screen: smaller text, so menus and pages hold enough.
        settings: &[("core.ui-size", "9"), ("reader.font-size", "9")],
        summary: "ESP32-C3, 4.3\" 800x480, buttons only, 192 KiB heap budget",
    },
    Profile {
        device: DeviceInfo {
            id: "xteink-x4-pro",
            name: "Xteink X4 Pro",
            display: DisplayInfo {
                // Unverified: 4.3" at 219 ppi suggests the X4's 800x480 panel.
                width: 480,
                height: 800,
                dpi: 219,
                format: PixelFormat::Gray2,
                rotation: Rotation::Deg90,
                color: None,
            },
            input: InputCaps {
                buttons: true,
                touch: true,
                pen: false,
            },
            memory: MemoryClass::Small,
        },
        // Previous/next, a touch home key and power (reset is hardware).
        buttons: &[
            buttons::PAGE_BACK,
            buttons::PAGE_FORWARD,
            buttons::HOME,
            buttons::POWER,
        ],
        // Unverified: ESP32-S3 (512 KB SRAM) assumed without PSRAM.
        heap_budget: Some(256 * KIB),
        glyph_cache: 0,
        font_packs: true,
        frame_rows: Some(100),
        // A 4.3" screen: smaller text, so menus and pages hold enough.
        settings: &[("core.ui-size", "9"), ("reader.font-size", "9")],
        summary: "ESP32-S3 (unverified), 4.3\" 800x480, touch + buttons, 256 KiB heap budget",
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
            memory: MemoryClass::Large,
        },
        buttons: &[buttons::PAGE_BACK, buttons::PAGE_FORWARD, buttons::POWER],
        heap_budget: None,
        glyph_cache: 256 * KIB,
        font_packs: false,
        frame_rows: None,
        settings: &[],
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
            memory: MemoryClass::Large,
        },
        buttons: &[buttons::PAGE_BACK, buttons::PAGE_FORWARD, buttons::POWER],
        heap_budget: None,
        glyph_cache: 256 * KIB,
        font_packs: false,
        frame_rows: None,
        settings: &[],
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
            memory: MemoryClass::Large,
        },
        buttons: &[buttons::POWER],
        heap_budget: None,
        glyph_cache: 256 * KIB,
        font_packs: false,
        frame_rows: None,
        settings: &[],
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
            memory: MemoryClass::Large,
        },
        buttons: &[buttons::POWER],
        heap_budget: None,
        glyph_cache: 256 * KIB,
        font_packs: false,
        frame_rows: None,
        settings: &[],
        summary: "Linux, 6\" 1072x1448, touch only",
    },
    Profile {
        device: DeviceInfo {
            id: "two-buttons",
            name: "Two-button e-reader",
            display: DisplayInfo {
                width: 600,
                height: 800,
                dpi: 167,
                format: PixelFormat::Gray4,
                rotation: Rotation::Deg0,
                color: None,
            },
            input: InputCaps {
                buttons: true,
                touch: false,
                pen: false,
            },
            memory: MemoryClass::Large,
        },
        buttons: &[buttons::PAGE_BACK, buttons::PAGE_FORWARD, buttons::POWER],
        heap_budget: None,
        glyph_cache: 64 * KIB,
        font_packs: false,
        frame_rows: None,
        // Only two buttons: everything works with forward and back.
        settings: &[("core.keymap-preset", "core.two-buttons")],
        summary: "Generic 6\" 600x800, only page buttons (also: Bluetooth page turners)",
    },
];

pub fn find(id: &str) -> Option<&'static Profile> {
    PROFILES.iter().find(|p| p.device.id == id)
}
