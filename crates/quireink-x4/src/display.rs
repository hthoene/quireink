//! The X4's display: a GDEQ0426T82 panel (800 × 480, black and white)
//! driven by an SSD1677 controller over SPI.
//!
//! The controller has two memories of the whole panel: the frame to show
//! (*new*) and the frame shown (*shown*), which a fast refresh compares
//! with, to move only the pixels that change. QuireInk draws the screen
//! upright (480 × 800) in bands of rows, aligned to 8 (see
//! `KernelBuilder::row_align`); [`Display::write_rows`] writes each band
//! turned into one or both memories:
//!
//! - **Full** and **half** refreshes: the band goes into both, then
//!   [`Display::refresh`].
//! - **Fast** refreshes: the band goes into *new*, then
//!   [`Display::refresh`], then the same bands again into *shown*
//!   (`Kernel::rerender`), for the next one to compare with.
//!
//! So the firmware keeps no frame of its own: 48 KB of the 192 KB it has
//! stay free.
//!
//! The upright screen is the panel turned clockwise: the screen's row `y`
//! is the controller's column `y`, its column `x` the controller's row
//! `x` (its gates are wired bottom to top). A byte of the controller's
//! memory holds 8 pixels of a row (the most significant bit first; set
//! for white), so 8 rows of the screen.

use alloc::vec::Vec;

use embedded_hal::delay::DelayNs;
use embedded_hal::digital::{InputPin, OutputPin};
use embedded_hal::spi::SpiDevice;
use quireink_platform::{FrameView, PixelFormat};

/// The panel, as the controller addresses it.
pub const PANEL_WIDTH: u16 = 800;
pub const PANEL_HEIGHT: u16 = 480;
/// The screen QuireInk draws: upright.
pub const WIDTH: u16 = PANEL_HEIGHT;
pub const HEIGHT: u16 = PANEL_WIDTH;

/// Longest wait for the controller, e.g. a full refresh (about 2 s).
const BUSY_TIMEOUT_MS: u32 = 30_000;
/// Bytes sent at once: each sending has its cost, a column of a band takes
/// 12 to 24 bytes.
const CHUNK: usize = 1024;

/// Turns eight bytes (rows, the leftmost pixel in the highest bit) into
/// eight bytes of their columns (the top row in the highest bit).
fn transpose8(rows: [u8; 8]) -> [u8; 8] {
    let mut x = u64::from_be_bytes(rows);
    let t = (x ^ (x >> 7)) & 0x00AA_00AA_00AA_00AA;
    x ^= t ^ (t << 7);
    let t = (x ^ (x >> 14)) & 0x0000_CCCC_0000_CCCC;
    x ^= t ^ (t << 14);
    let t = (x ^ (x >> 28)) & 0x0000_0000_F0F0_F0F0;
    x ^= t ^ (t << 28);
    x.to_be_bytes()
}

/// The controller's commands used here.
mod cmd {
    pub const DRIVER_OUTPUT: u8 = 0x01;
    pub const BOOSTER_SOFT_START: u8 = 0x0C;
    pub const DEEP_SLEEP: u8 = 0x10;
    pub const DATA_ENTRY: u8 = 0x11;
    pub const SOFT_RESET: u8 = 0x12;
    pub const TEMPERATURE_SENSOR: u8 = 0x18;
    pub const WRITE_TEMPERATURE: u8 = 0x1A;
    pub const ACTIVATE: u8 = 0x20;
    pub const UPDATE_CONTROL_1: u8 = 0x21;
    pub const UPDATE_CONTROL_2: u8 = 0x22;
    pub const WRITE_NEW: u8 = 0x24;
    pub const WRITE_SHOWN: u8 = 0x26;
    pub const BORDER: u8 = 0x3C;
    pub const RAM_X_RANGE: u8 = 0x44;
    pub const RAM_Y_RANGE: u8 = 0x45;
    pub const FILL_NEW: u8 = 0x46;
    pub const FILL_SHOWN: u8 = 0x47;
    pub const RAM_X: u8 = 0x4E;
    pub const RAM_Y: u8 = 0x4F;
}

/// How the panel changes to the new frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refresh {
    /// Flashing, clears all ghosting (about 2 s).
    Full,
    /// Flashing less, warmer waveform (about 1.7 s).
    Half,
    /// Only the pixels that change (a few hundred ms); leaves some
    /// ghosting.
    Fast,
}

/// Which of the controller's memories a band goes into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Planes {
    pub new: bool,
    pub shown: bool,
}

impl Planes {
    /// For a full or half refresh: both.
    pub const BOTH: Self = Self {
        new: true,
        shown: true,
    };
    /// For a fast refresh, before it.
    pub const NEW: Self = Self {
        new: true,
        shown: false,
    };
    /// After a fast refresh, the frame now shown.
    pub const SHOWN: Self = Self {
        new: false,
        shown: true,
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Spi,
    Pin,
    /// The controller stayed busy too long.
    Timeout,
    /// Rows not aligned to 8, or not the screen's width.
    Rows,
}

/// The display, on its SPI device (which selects the chip) and pins.
#[derive(Debug)]
pub struct Display<SPI, DC, RST, BUSY, D> {
    spi: SPI,
    dc: DC,
    reset: RST,
    busy: BUSY,
    delay: D,
    /// The panel's power is on (after a refresh not followed by sleep).
    powered: bool,
    /// Nothing shown since [`Display::init`]: the controller does not know
    /// what the panel shows, so a fast refresh would compare with nothing.
    fresh: bool,
    buffer: Vec<u8>,
}

impl<SPI, DC, RST, BUSY, D> Display<SPI, DC, RST, BUSY, D>
where
    SPI: SpiDevice,
    DC: OutputPin,
    RST: OutputPin,
    BUSY: InputPin,
    D: DelayNs,
{
    pub fn new(spi: SPI, dc: DC, reset: RST, busy: BUSY, delay: D) -> Self {
        Self {
            spi,
            dc,
            reset,
            busy,
            delay,
            powered: false,
            fresh: true,
            buffer: Vec::new(),
        }
    }

    /// Resets and sets up the controller, both memories white.
    pub fn init(&mut self) -> Result<(), Error> {
        for (high, ms) in [(true, 20), (false, 2), (true, 20)] {
            if high {
                self.reset.set_high()
            } else {
                self.reset.set_low()
            }
            .map_err(|_| Error::Pin)?;
            self.delay.delay_ms(ms);
        }
        self.command(cmd::SOFT_RESET, &[])?;
        self.wait()?;
        // The internal temperature sensor.
        self.command(cmd::TEMPERATURE_SENSOR, &[0x80])?;
        // The booster's soft start, as the panel wants it.
        self.command(cmd::BOOSTER_SOFT_START, &[0xAE, 0xC7, 0xC3, 0xC0, 0x40])?;
        // 480 gates, scanned interlaced.
        let gates = PANEL_HEIGHT - 1;
        self.command(
            cmd::DRIVER_OUTPUT,
            &[(gates % 256) as u8, (gates / 256) as u8, 0x02],
        )?;
        self.command(cmd::BORDER, &[0x01])?;
        self.window(0, PANEL_WIDTH)?;
        for fill in [cmd::FILL_NEW, cmd::FILL_SHOWN] {
            self.command(fill, &[0xF7])?;
            self.wait()?;
        }
        self.fresh = true;
        Ok(())
    }

    /// The controller writes into columns `x..x + width` of every row,
    /// row by row from the top of the screen.
    fn window(&mut self, x: u16, width: u16) -> Result<(), Error> {
        let last_x = x + width - 1;
        let last_y = PANEL_HEIGHT - 1;
        // X and then Y increase.
        self.command(cmd::DATA_ENTRY, &[0x03])?;
        self.command(
            cmd::RAM_X_RANGE,
            &[
                (x % 256) as u8,
                (x / 256) as u8,
                (last_x % 256) as u8,
                (last_x / 256) as u8,
            ],
        )?;
        self.command(
            cmd::RAM_Y_RANGE,
            &[0, 0, (last_y % 256) as u8, (last_y / 256) as u8],
        )?;
        self.command(cmd::RAM_X, &[(x % 256) as u8, (x / 256) as u8])?;
        self.command(cmd::RAM_Y, &[0, 0])
    }

    /// Writes upright screen rows `top..top + rows.height` (a band from
    /// `Kernel::render`, 480 pixels wide, starting at a multiple of 8)
    /// into `planes`. Pixels lighter than middle gray are white.
    pub fn write_rows(
        &mut self,
        top: u16,
        rows: FrameView<'_>,
        planes: Planes,
    ) -> Result<(), Error> {
        if top % 8 != 0 || top >= HEIGHT || rows.width != WIDTH || rows.height == 0 {
            return Err(Error::Rows);
        }
        let height = rows.height.min(HEIGHT - top);
        // Whole bytes: rows past the band's end (only at its last byte)
        // are white.
        let bytes = usize::from(height.div_ceil(8));
        // Light pixels show white, dark ones black: gray formats read
        // straight from their bytes, others through their colour.
        let bpp = usize::from(rows.format.bits_per_pixel());
        let gray = rows.format.gray_levels().is_some() && bpp <= 8;
        let max = (1u32 << bpp.min(8)) - 1;
        let light = |x: u16, y: u16| {
            if gray {
                let bit = usize::from(x) * bpp;
                let at = usize::from(y) * rows.stride + bit / 8;
                let byte = rows.data.get(at).copied().unwrap_or(0xFF);
                let level = u32::from(byte >> (8 - bpp - bit % 8)) & max;
                level * 255 >= 128 * max
            } else {
                let c = rows.pixel(x, y);
                u16::from(c.r) * 77 + u16::from(c.g) * 150 + u16::from(c.b) * 29 >= 128 * 256
            }
        };
        for (plane, write) in [
            (planes.new, cmd::WRITE_NEW),
            (planes.shown, cmd::WRITE_SHOWN),
        ] {
            if !plane {
                continue;
            }
            self.window(top, bytes as u16 * 8)?;
            self.command(write, &[])?;
            self.buffer.clear();
            if rows.format == PixelFormat::Mono1 {
                self.write_mono(&rows, height, bytes)?;
                continue;
            }
            for x in 0..WIDTH {
                for byte in 0..bytes {
                    let mut bits = 0u8;
                    for bit in 0..8u16 {
                        let y = byte as u16 * 8 + bit;
                        let white = y >= height || light(x, y);
                        if white {
                            bits |= 0x80 >> bit;
                        }
                    }
                    self.buffer.push(bits);
                }
                if self.buffer.len() >= CHUNK {
                    self.data_buffer()?;
                }
            }
            self.data_buffer()?;
        }
        Ok(())
    }

    /// [`Display::write_rows`] for black and white, as the X4 draws: eight
    /// rows by eight columns at a time, turned with a few shifts where
    /// pixel by pixel took a quarter of a page's time.
    fn write_mono(&mut self, rows: &FrameView<'_>, height: u16, bytes: usize) -> Result<(), Error> {
        // Each of eight columns' bytes, top to bottom.
        let mut columns = [0u8; 8 * (HEIGHT as usize / 8)];
        for column in 0..usize::from(WIDTH) / 8 {
            for group in 0..bytes {
                let mut block = [0xFFu8; 8];
                for (i, row) in block.iter_mut().enumerate() {
                    let y = group * 8 + i;
                    if y < usize::from(height) {
                        *row = rows
                            .data
                            .get(y * rows.stride + column)
                            .copied()
                            .unwrap_or(0xFF);
                    }
                }
                for (c, byte) in transpose8(block).into_iter().enumerate() {
                    columns[c * bytes + group] = byte;
                }
            }
            for c in 0..8 {
                self.buffer
                    .extend_from_slice(&columns[c * bytes..(c + 1) * bytes]);
            }
            if self.buffer.len() >= CHUNK {
                self.data_buffer()?;
            }
        }
        self.data_buffer()
    }

    fn data_buffer(&mut self) -> Result<(), Error> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        self.dc.set_high().map_err(|_| Error::Pin)?;
        self.spi.write(&self.buffer).map_err(|_| Error::Spi)?;
        self.buffer.clear();
        Ok(())
    }

    /// Shows the new frame; `power_off` turns the panel's power off after
    /// it, e.g. before sleeping. The first after [`Display::init`] is full
    /// in any case.
    pub fn refresh(&mut self, refresh: Refresh, power_off: bool) -> Result<(), Error> {
        let refresh = if self.fresh { Refresh::Full } else { refresh };
        self.fresh = false;
        // A fast refresh compares with the frame shown; the others
        // drive every pixel.
        let compare = if refresh == Refresh::Fast { 0x00 } else { 0x40 };
        self.command(cmd::UPDATE_CONTROL_1, &[compare])?;
        let mut sequence = match refresh {
            Refresh::Full => 0x34,
            Refresh::Half => {
                // A warm panel's waveform: shorter.
                self.command(cmd::WRITE_TEMPERATURE, &[0x5A])?;
                0xD4
            }
            Refresh::Fast => 0x1C,
        };
        if !self.powered {
            // Clock and analog power on first.
            sequence |= 0xC0;
        }
        if power_off {
            sequence |= 0x03;
        }
        self.command(cmd::UPDATE_CONTROL_2, &[sequence])?;
        self.command(cmd::ACTIVATE, &[])?;
        self.powered = !power_off;
        self.wait()
    }

    /// Puts the controller into deep sleep, the picture staying; [`init`]
    /// wakes it again.
    ///
    /// [`init`]: Display::init
    pub fn sleep(&mut self) -> Result<(), Error> {
        if self.powered {
            self.command(cmd::UPDATE_CONTROL_1, &[0x40])?;
            self.command(cmd::UPDATE_CONTROL_2, &[0x03])?;
            self.command(cmd::ACTIVATE, &[])?;
            self.wait()?;
            self.powered = false;
        }
        self.command(cmd::DEEP_SLEEP, &[0x01])
    }

    fn command(&mut self, command: u8, data: &[u8]) -> Result<(), Error> {
        self.dc.set_low().map_err(|_| Error::Pin)?;
        self.spi.write(&[command]).map_err(|_| Error::Spi)?;
        if !data.is_empty() {
            self.dc.set_high().map_err(|_| Error::Pin)?;
            self.spi.write(data).map_err(|_| Error::Spi)?;
        }
        Ok(())
    }

    /// Waits while the controller is busy (its busy pin high).
    fn wait(&mut self) -> Result<(), Error> {
        for _ in 0..BUSY_TIMEOUT_MS {
            if !self.busy.is_high().map_err(|_| Error::Pin)? {
                return Ok(());
            }
            self.delay.delay_ms(1);
        }
        Err(Error::Timeout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::rc::Rc;
    use alloc::vec;
    use core::cell::RefCell;
    use embedded_hal::spi::{ErrorType, Operation};
    use quireink_platform::PixelFormat;

    /// The controller, simulated: its two memories and what it was told.
    struct Controller {
        data: bool,
        command: u8,
        args: Vec<u8>,
        x_range: (u16, u16),
        y_range: (u16, u16),
        x: u16,
        y: u16,
        /// Rows of 100 bytes, white.
        new: Vec<u8>,
        shown: Vec<u8>,
        commands: Vec<u8>,
        /// The first argument each command had last.
        first: [u8; 256],
        refreshes: Vec<(u8, u8)>,
    }

    impl Controller {
        fn new() -> Rc<RefCell<Self>> {
            let size = usize::from(PANEL_WIDTH / 8) * usize::from(PANEL_HEIGHT);
            Rc::new(RefCell::new(Self {
                data: false,
                command: 0,
                args: Vec::new(),
                x_range: (0, 0),
                y_range: (0, 0),
                x: 0,
                y: 0,
                new: vec![0; size],
                shown: vec![0; size],
                commands: Vec::new(),
                first: [0; 256],
                refreshes: Vec::new(),
            }))
        }

        fn byte(&mut self, b: u8) {
            if !self.data {
                self.command = b;
                self.args.clear();
                self.commands.push(b);
                if b == cmd::ACTIVATE {
                    let control_1 = self.first[usize::from(cmd::UPDATE_CONTROL_1)];
                    let control_2 = self.first[usize::from(cmd::UPDATE_CONTROL_2)];
                    self.refreshes.push((control_1, control_2));
                }
                return;
            }
            self.args.push(b);
            if self.args.len() == 1 {
                self.first[usize::from(self.command)] = b;
            }
            let word = |a: &[u8], i: usize| u16::from(a[i]) | u16::from(a[i + 1]) << 8;
            match self.command {
                cmd::RAM_X_RANGE if self.args.len() == 4 => {
                    self.x_range = (word(&self.args, 0), word(&self.args, 2));
                }
                cmd::RAM_Y_RANGE if self.args.len() == 4 => {
                    self.y_range = (word(&self.args, 0), word(&self.args, 2));
                }
                cmd::RAM_X if self.args.len() == 2 => self.x = word(&self.args, 0),
                cmd::RAM_Y if self.args.len() == 2 => self.y = word(&self.args, 0),
                cmd::DATA_ENTRY => assert_eq!(b, 0x03, "X, then Y increase"),
                cmd::FILL_NEW => self.new.fill(0xFF),
                cmd::FILL_SHOWN => self.shown.fill(0xFF),
                cmd::WRITE_NEW | cmd::WRITE_SHOWN => {
                    let at = usize::from(self.y) * 100 + usize::from(self.x / 8);
                    if self.command == cmd::WRITE_NEW {
                        self.new[at] = b;
                    } else {
                        self.shown[at] = b;
                    }
                    self.x += 8;
                    if self.x > self.x_range.1 {
                        self.x = self.x_range.0;
                        self.y += 1;
                        assert!(self.y <= self.y_range.1 + 1);
                    }
                }
                _ => {}
            }
        }

        /// Whether the panel pixel at upright `(x, y)` is white in `plane`.
        fn white(plane: &[u8], x: u16, y: u16) -> bool {
            plane[usize::from(x) * 100 + usize::from(y / 8)] & (0x80 >> (y % 8)) != 0
        }
    }

    struct Spi(Rc<RefCell<Controller>>);
    struct Dc(Rc<RefCell<Controller>>);
    struct Pin;
    struct Busy;
    struct Delay;

    impl ErrorType for Spi {
        type Error = core::convert::Infallible;
    }
    impl SpiDevice for Spi {
        fn transaction(&mut self, operations: &mut [Operation<'_, u8>]) -> Result<(), Self::Error> {
            for op in operations {
                if let Operation::Write(bytes) = op {
                    for &b in bytes.iter() {
                        self.0.borrow_mut().byte(b);
                    }
                }
            }
            Ok(())
        }
    }
    impl embedded_hal::digital::ErrorType for Dc {
        type Error = core::convert::Infallible;
    }
    impl OutputPin for Dc {
        fn set_low(&mut self) -> Result<(), Self::Error> {
            self.0.borrow_mut().data = false;
            Ok(())
        }
        fn set_high(&mut self) -> Result<(), Self::Error> {
            self.0.borrow_mut().data = true;
            Ok(())
        }
    }
    impl embedded_hal::digital::ErrorType for Pin {
        type Error = core::convert::Infallible;
    }
    impl OutputPin for Pin {
        fn set_low(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
        fn set_high(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
    }
    impl embedded_hal::digital::ErrorType for Busy {
        type Error = core::convert::Infallible;
    }
    impl InputPin for Busy {
        fn is_high(&mut self) -> Result<bool, Self::Error> {
            Ok(false)
        }
        fn is_low(&mut self) -> Result<bool, Self::Error> {
            Ok(true)
        }
    }
    impl DelayNs for Delay {
        fn delay_ns(&mut self, _: u32) {}
    }

    /// An upright screen band in 2-bit gray, black where `ink` says.
    fn band(height: u16, ink: impl Fn(u16, u16) -> bool) -> Vec<u8> {
        let stride = usize::from(WIDTH) / 4;
        let mut data = vec![0xFF; stride * usize::from(height)];
        for y in 0..height {
            for x in 0..WIDTH {
                if ink(x, y) {
                    let at = usize::from(y) * stride + usize::from(x / 4);
                    data[at] &= !(0b11 << (6 - 2 * (x % 4)));
                }
            }
        }
        data
    }

    fn view(data: &[u8], height: u16) -> FrameView<'_> {
        FrameView {
            width: WIDTH,
            height,
            format: PixelFormat::Gray2,
            stride: usize::from(WIDTH) / 4,
            data,
        }
    }

    #[test]
    fn eight_by_eight_turned() {
        let rows = [
            0b1000_0000,
            0b0100_0001,
            0,
            0xFF,
            0b0000_0001,
            0,
            0,
            0b1000_0000,
        ];
        let columns = transpose8(rows);
        for (r, row) in rows.iter().enumerate() {
            for (c, column) in columns.iter().enumerate() {
                let row_bit = row >> (7 - c) & 1;
                let column_bit = column >> (7 - r) & 1;
                assert_eq!(row_bit, column_bit, "row {r}, column {c}");
            }
        }
    }

    #[test]
    fn black_and_white_bands_as_gray_ones() {
        // The same ink in black and white and in gray: the same planes,
        // also for a band whose height is not a multiple of 8.
        let ink = |x: u16, y: u16| (x * 7 + y * 3) % 5 == 0 || x == y;
        for height in [16, 13] {
            let gray = band(height, ink);
            let stride = usize::from(WIDTH) / 8;
            let mut mono = vec![0xFFu8; stride * usize::from(height)];
            for y in 0..height {
                for x in 0..WIDTH {
                    if ink(x, y) {
                        mono[usize::from(y) * stride + usize::from(x) / 8] &= !(0x80 >> (x % 8));
                    }
                }
            }
            let planes = |view: FrameView<'_>| {
                let controller = Controller::new();
                let mut display = Display::new(
                    Spi(controller.clone()),
                    Dc(controller.clone()),
                    Pin,
                    Busy,
                    Delay,
                );
                display.init().unwrap();
                display.write_rows(96, view, Planes::NEW).unwrap();
                controller.borrow().new.clone()
            };
            let mono_view = FrameView {
                width: WIDTH,
                height,
                format: PixelFormat::Mono1,
                stride,
                data: &mono,
            };
            assert!(
                planes(view(&gray, height)) == planes(mono_view),
                "height {height}"
            );
        }
    }

    #[test]
    fn bands_go_in_turned_into_the_planes_asked() {
        let controller = Controller::new();
        let mut display = Display::new(
            Spi(controller.clone()),
            Dc(controller.clone()),
            Pin,
            Busy,
            Delay,
        );
        display.init().unwrap();
        // A band of 16 rows from row 96: ink on its diagonal.
        let ink = |x: u16, y: u16| x == y * 3;
        let data = band(16, ink);
        display
            .write_rows(96, view(&data, 16), Planes::NEW)
            .unwrap();
        let c = controller.borrow();
        for y in 96..112 {
            for x in 0..WIDTH {
                let expected = !ink(x, y - 96);
                assert_eq!(Controller::white(&c.new, x, y), expected, "({x}, {y})");
                assert!(Controller::white(&c.shown, x, y), "shown untouched");
            }
        }
        // Rows of other bands stay white.
        assert!(Controller::white(&c.new, 0, 95));
        assert!(Controller::white(&c.new, 0, 112));
        drop(c);
        // After a fast refresh, the same into the frame shown.
        display.refresh(Refresh::Fast, false).unwrap();
        display
            .write_rows(96, view(&data, 16), Planes::SHOWN)
            .unwrap();
        let c = controller.borrow();
        assert_eq!(c.new, c.shown);
    }

    #[test]
    fn the_kernels_bands_make_its_frame() {
        use quireink_core::Kernel;
        use quireink_platform::{
            DeviceInfo, DisplayInfo, InputCaps, Instant, MemoryClass, Rotation,
        };
        let device = DeviceInfo {
            id: "x4",
            name: "X4",
            display: DisplayInfo {
                width: WIDTH,
                height: HEIGHT,
                dpi: 220,
                format: PixelFormat::Gray2,
                rotation: Rotation::Deg90,
                color: None,
            },
            input: InputCaps {
                buttons: true,
                touch: false,
                pen: false,
                keys: &[],
            },
            memory: MemoryClass::Small,
        };
        let build = |banded: bool| {
            let mut builder = Kernel::builder(device);
            if banded {
                builder = builder.frame_rows(96).row_align(8);
            }
            let mut kernel = quireink_app::install(builder, quireink_app::Fonts::Packs)
                .build()
                .unwrap();
            kernel.start(Instant(0));
            kernel
        };
        // The whole frame at once, thresholded as the display does.
        let mut whole = build(false);
        let mut expected = vec![false; usize::from(WIDTH) * usize::from(HEIGHT)];
        whole.render(&mut |top, rows| {
            for y in 0..rows.height {
                for x in 0..rows.width {
                    let c = rows.pixel(x, y);
                    let white = u16::from(c.r) * 77 + u16::from(c.g) * 150 + u16::from(c.b) * 29
                        >= 128 * 256;
                    expected[usize::from(top + y) * usize::from(WIDTH) + usize::from(x)] = white;
                }
            }
        });
        assert!(expected.iter().any(|w| !w), "something is drawn");
        // In bands, into the controller.
        let controller = Controller::new();
        let mut display = Display::new(
            Spi(controller.clone()),
            Dc(controller.clone()),
            Pin,
            Busy,
            Delay,
        );
        display.init().unwrap();
        let mut banded = build(true);
        let mut bands = 0;
        banded.render(&mut |top, rows| {
            bands += 1;
            display.write_rows(top, rows, Planes::NEW).unwrap();
        });
        assert!(bands > 1);
        let c = controller.borrow();
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                let at = usize::from(y) * usize::from(WIDTH) + usize::from(x);
                assert_eq!(Controller::white(&c.new, x, y), expected[at], "({x}, {y})");
            }
        }
    }

    #[test]
    fn a_short_last_band_is_white_below() {
        let controller = Controller::new();
        let mut display = Display::new(
            Spi(controller.clone()),
            Dc(controller.clone()),
            Pin,
            Busy,
            Delay,
        );
        display.init().unwrap();
        let data = band(5, |_, _| true);
        display
            .write_rows(792, view(&data, 5), Planes::BOTH)
            .unwrap();
        let c = controller.borrow();
        assert!(!Controller::white(&c.new, 10, 796));
        assert!(Controller::white(&c.new, 10, 797));
        assert!(!Controller::white(&c.shown, 10, 792));
        drop(c);
        assert_eq!(
            display.write_rows(4, view(&data, 5), Planes::BOTH),
            Err(Error::Rows),
            "not aligned"
        );
    }

    #[test]
    fn refreshes_power_on_once_and_off_when_asked() {
        let controller = Controller::new();
        let mut display = Display::new(
            Spi(controller.clone()),
            Dc(controller.clone()),
            Pin,
            Busy,
            Delay,
        );
        display.init().unwrap();
        // The first is full whatever is asked.
        display.refresh(Refresh::Fast, false).unwrap();
        display.refresh(Refresh::Fast, false).unwrap();
        display.refresh(Refresh::Half, true).unwrap();
        display.sleep().unwrap();
        let c = controller.borrow();
        let sequences: Vec<u8> = c.refreshes.iter().map(|&(_, two)| two).collect();
        assert_eq!(sequences, [0xF4, 0x1C, 0xD7]);
        assert_eq!(c.commands.last(), Some(&cmd::DEEP_SLEEP));
    }
}
