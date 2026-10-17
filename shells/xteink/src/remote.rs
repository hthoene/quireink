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
//! - `heap`: `QI:HEAP <used> <free>`, in bytes.
//!
//! Any other line is answered with `QI:ERR <line>`.

use alloc::string::String;
use core::fmt::Write;

use esp_hal::peripherals::USB_DEVICE;
use esp_println::println;
use quireink_core::Kernel;
use quireink_platform::{DisplayInfo, FrameView};
use quireink_x4::buttons;

/// A line is never longer.
const MAX_LINE: usize = 64;

/// What the computer asked for.
pub enum Command {
    /// A button pressed, let go after so many milliseconds.
    Press(&'static str, u64),
    Shot,
    Screen,
    Describe,
    Heap,
}

/// Reads commands from USB, a line at a time.
///
/// It reads the USB unit's receive buffer itself: esp-hal's driver resets
/// the unit when it starts, which cuts the connection the log goes out on.
pub struct Remote {
    _usb: USB_DEVICE<'static>,
    line: String,
}

impl Remote {
    pub fn new(usb: USB_DEVICE<'static>) -> Self {
        Self {
            _usb: usb,
            line: String::new(),
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

    /// The next command, once a whole line has come.
    pub fn poll(&mut self) -> Option<Command> {
        while let Some(byte) = self.read_byte() {
            match byte {
                b'\n' | b'\r' => {
                    if self.line.is_empty() {
                        continue;
                    }
                    let line = core::mem::take(&mut self.line);
                    match parse(line.trim()) {
                        Some(command) => return Some(command),
                        None => println!("QI:ERR {line}"),
                    }
                }
                _ if self.line.len() < MAX_LINE => self.line.push(char::from(byte)),
                _ => {}
            }
        }
        None
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
        _ => return None,
    };
    words.next().is_none().then_some(command)
}

/// The X4's button of that name.
fn button(name: &str) -> Option<&'static str> {
    buttons::ALL.iter().find(|b| **b == name).copied()
}

/// Answers a command that only asks (the screen, a screenshot, what it
/// shows, the heap); a button press it gives back for the caller to make.
pub fn answer(
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
    }
    None
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
