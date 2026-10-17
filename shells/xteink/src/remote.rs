//! Remote control over USB, for testing on the device: a computer sends
//! lines with commands (`tools/x4-remote` does), the X4 answers with lines
//! that start with `QI:`, amid its log.
//!
//! - `press <button>`: the button pressed and let go;
//! - `hold <button> <ms>`: held that long, e.g. for a long press;
//! - `shot`: the screen, as `QI:FRAME <width> <height> <bits per pixel>`,
//!   then `QI:ROW <y> <hex>` for every row (its bytes as the frame holds
//!   them, in runs: a count from 1 to 255, then the byte) and `QI:END`;
//! - `screen`: `QI:SCREEN <id>`, the screen shown;
//! - `describe`: what it shows, a `QI:PART <role> <x> <y> <width> <height>
//!   <state> <label>` for each row, button or heading (state: `-`, or
//!   `focused`, `on`, `off` joined by `+`), then `QI:END`;
//! - `heap`: `QI:HEAP <used> <free>`, in bytes;
//! - `input`: `QI:INPUT <readings> <front> <side> <power> <millivolts>`, how
//!   the buttons and the battery are read;
//! - `put <size> <path>`: a file for the storage (the SD card), answered
//!   with `QI:READY`; then lines `data <hex>` until all its bytes came, and
//!   `QI:OK <size>` once it is written.
//!
//! Any other line is answered with `QI:ERR <line>`.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;

use esp_hal::peripherals::USB_DEVICE;
use esp_println::println;
use quireink_core::Kernel;
use quireink_platform::{DisplayInfo, FrameView};
use quireink_x4::buttons;

/// A line is never longer: `data` and 512 bytes in hexadecimal.
const MAX_LINE: usize = 1100;
/// A file is written in pieces this large.
const CHUNK: usize = 4096;
/// Without data for this long, a file is given up.
const QUIET_MS: u64 = 5000;

/// What the computer asked for.
pub enum Command {
    /// A button pressed, let go after so many milliseconds.
    Press(&'static str, u64),
    Shot,
    Screen,
    Describe,
    Heap,
    Input,
    /// A file of so many bytes for that path.
    Put(usize, String),
}

/// Reads commands from USB, a line at a time.
///
/// It reads the USB unit's receive buffer itself: esp-hal's driver resets
/// the unit when it starts, which cuts the connection the log goes out on.
pub struct Remote {
    _usb: USB_DEVICE<'static>,
    line: Vec<u8>,
}

impl Remote {
    pub fn new(usb: USB_DEVICE<'static>) -> Self {
        Self {
            _usb: usb,
            line: Vec::new(),
        }
    }

    /// The next byte the computer sent, if any.
    fn read_byte(&mut self) -> Option<u8> {
        let regs = USB_DEVICE::regs();
        regs.ep1_conf()
            .read()
            .serial_out_ep_data_avail()
            .bit_is_set()
            .then(|| regs.ep1().read().rdwr_byte().bits())
    }

    /// The next whole line that came, if one did.
    fn next_line(&mut self) -> Option<Vec<u8>> {
        while let Some(byte) = self.read_byte() {
            match byte {
                b'\n' | b'\r' if self.line.is_empty() => {}
                b'\n' | b'\r' => return Some(core::mem::take(&mut self.line)),
                _ if self.line.len() < MAX_LINE => self.line.push(byte),
                _ => {}
            }
        }
        None
    }

    /// The next command, once a whole line has come.
    pub fn poll(&mut self) -> Option<Command> {
        let line = self.next_line()?;
        let line = String::from_utf8_lossy(&line);
        let command = parse(line.trim());
        if command.is_none() {
            println!("QI:ERR {line}");
        }
        command
    }

    /// Answers a command that only asks (the screen, a screenshot, what it
    /// shows, the heap) or brings a file; a button press it gives back for
    /// the caller to make.
    pub fn answer(
        &mut self,
        kernel: &mut Kernel,
        display: &DisplayInfo,
        command: Command,
    ) -> Option<(&'static str, u64)> {
        match command {
            Command::Press(name, ms) => return Some((name, ms)),
            Command::Shot => {
                shot(kernel, display);
            }
            Command::Describe => describe(kernel),
            Command::Screen => {
                let id = kernel.active_screen().map_or("", |id| id.as_str());
                println!("QI:SCREEN {id}");
            }
            Command::Heap => {
                println!(
                    "QI:HEAP {} {}",
                    esp_alloc::HEAP.used(),
                    esp_alloc::HEAP.free()
                );
            }
            Command::Put(size, path) => self.receive(kernel, size, &path),
            Command::Input => {
                let (samples, front, side) = crate::input::stats();
                let power = crate::input::power_down();
                let millivolts = crate::input::millivolts();
                println!("QI:INPUT {samples} {front} {side} {power} {millivolts}");
            }
        }
        None
    }

    /// Writes a file of `size` bytes to `path` from the lines `data <hex>`
    /// that follow, a piece at a time.
    fn receive(&mut self, kernel: &Kernel, size: usize, path: &str) {
        let storage = kernel.storage();
        if let Err(e) = storage.write(path, &[]) {
            return println!("QI:ERR {path}: {e:?}");
        }
        println!("QI:READY");
        let millis = || {
            esp_hal::time::Instant::now()
                .duration_since_epoch()
                .as_millis()
        };
        let mut piece = Vec::with_capacity(CHUNK);
        let (mut got, mut last) = (0, millis());
        while got < size {
            let Some(line) = self.next_line() else {
                if millis() - last > QUIET_MS {
                    return println!("QI:ERR {path}: {got} of {size} bytes came");
                }
                continue;
            };
            last = millis();
            let Some(hex) = line.strip_prefix(b"data ") else {
                return println!("QI:ERR {path}: not data");
            };
            for pair in hex.as_chunks::<2>().0 {
                let digit = |c: u8| char::from(c).to_digit(16).unwrap_or(0) as u8;
                piece.push(digit(pair[0]) << 4 | digit(pair[1]));
            }
            got += hex.len() / 2;
            if piece.len() >= CHUNK || got >= size {
                if let Err(e) = storage.append(path, &piece) {
                    return println!("QI:ERR {path}: {e:?}");
                }
                piece.clear();
            }
        }
        println!("QI:OK {got}");
    }
}

fn parse(line: &str) -> Option<Command> {
    let mut words = line.split_ascii_whitespace();
    let command = match words.next()? {
        "press" => Command::Press(button(words.next()?)?, 60),
        "hold" => Command::Press(button(words.next()?)?, words.next()?.parse().ok()?),
        "shot" => Command::Shot,
        "screen" => Command::Screen,
        "describe" => Command::Describe,
        "heap" => Command::Heap,
        "input" => Command::Input,
        "put" => {
            let size = words.next()?.parse().ok()?;
            // The path is the rest of the line, spaces and all.
            let (_, rest) = line.split_once(' ')?;
            let path = rest.trim_start().split_once(' ')?.1.trim();
            return (!path.is_empty()).then(|| Command::Put(size, String::from(path)));
        }
        _ => return None,
    };
    words.next().is_none().then_some(command)
}

/// The X4's button of that name.
fn button(name: &str) -> Option<&'static str> {
    buttons::ALL.iter().find(|b| **b == name).copied()
}

/// Sends the whole screen, `display` (screens may have less of it); `false`
/// while changes wait to be shown (ask again).
pub fn shot(kernel: &mut Kernel, display: &DisplayInfo) -> bool {
    let header = (
        display.width,
        display.height,
        display.format.bits_per_pixel(),
    );
    let mut started = false;
    let mut hex = String::new();
    let done = kernel.capture(&mut |top, rows: FrameView<'_>| {
        if !started {
            println!("QI:FRAME {} {} {}", header.0, header.1, header.2);
            started = true;
        }
        for y in 0..rows.height {
            let at = usize::from(y) * rows.stride;
            let row = rows.data.get(at..at + rows.stride).unwrap_or(&[]);
            hex.clear();
            runs(row, &mut hex);
            println!("QI:ROW {} {hex}", top + y);
        }
    });
    if done {
        println!("QI:END");
    } else {
        println!("QI:BUSY");
    }
    done
}

/// Sends what the screen shows, part by part.
pub fn describe(kernel: &mut Kernel) {
    for part in kernel.describe() {
        let mut state = String::new();
        for (on, word) in [
            (part.focused, "focused"),
            (part.on == Some(true), "on"),
            (part.on == Some(false), "off"),
        ] {
            if on {
                if !state.is_empty() {
                    state.push('+');
                }
                state.push_str(word);
            }
        }
        if state.is_empty() {
            state.push('-');
        }
        let a = part.area;
        let label = part.label.replace('\n', " ");
        println!(
            "QI:PART {:?} {} {} {} {} {state} {label}",
            part.role, a.x, a.y, a.width, a.height
        );
    }
    println!("QI:END");
}

/// `bytes` as runs of one byte, written in hexadecimal.
fn runs(bytes: &[u8], out: &mut String) {
    let mut rest = bytes;
    while let Some(&byte) = rest.first() {
        let count = rest.iter().take(255).take_while(|b| **b == byte).count();
        let _ = write!(out, "{count:02x}{byte:02x}");
        rest = &rest[count..];
    }
}
