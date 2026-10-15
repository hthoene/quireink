//! Scripted, windowless runs for screenshots, CI and bug reports.
//!
//! A script is a comma-separated list of steps:
//!
//! | Step | Meaning |
//! |------|---------|
//! | `page-forward` | short press of a button |
//! | `long:confirm` | long press of a button |
//! | `tap:240:400` | tap at logical coordinates |
//! | `long-tap:240:400` | long press on the touch screen |
//! | `swipe:400:400:100:400` | swipe from one point to another |
//! | `wait:500` | let 500 ms pass |
//! | `serve:20` | answer plugins' servers in real time for 20 s |
//! | `shot:out/a.png` | save a screenshot now (the last one comes with `--screenshot`) |

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use quireink_platform::{ButtonState, InputEvent, Point, PointerEvent, PointerPhase, PointerTool};

use crate::device::Device;

/// Time between script steps, long enough to never look like a long press.
const STEP_MS: u64 = 100;
/// Comfortably above the default long-press threshold.
const LONG_MS: u64 = 1200;

pub fn run_script(device: &mut Device, script: &str) -> Result<(), String> {
    for step in script.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        run_step(device, step).map_err(|e| format!("step `{step}`: {e}"))?;
        // Network answers take real time; wait for them, so runs repeat.
        device.poll_http(Some(std::time::Duration::from_secs(25)));
        device.advance(STEP_MS);
        device.tick_if_due();
    }
    Ok(())
}

fn run_step(device: &mut Device, step: &str) -> Result<(), String> {
    let parts: Vec<&str> = step.split(':').collect();
    let touch_step = matches!(parts.first(), Some(&("tap" | "long-tap" | "swipe")));
    if touch_step && !device.profile.device.input.touch {
        return Err(format!(
            "`{}` has no touch screen",
            device.profile.device.id
        ));
    }
    let number = |s: &str| {
        s.parse::<i32>()
            .map_err(|_| format!("`{s}` is not a number"))
    };
    match parts.as_slice() {
        ["wait", ms] => {
            let ms: u64 = ms.parse().map_err(|_| format!("`{ms}` is not a number"))?;
            hold(device, ms);
        }
        ["shot", path @ ..] if !path.is_empty() => {
            // Paths may hold `:` (`C:/…`).
            let path = path.join(":");
            screenshot(device, Path::new(&path))?;
        }
        ["serve", seconds] => {
            let seconds: u64 = seconds
                .parse()
                .map_err(|_| format!("`{seconds}` is not a number"))?;
            let end = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
            while std::time::Instant::now() < end {
                device.poll_servers();
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }
        ["long", name] => press(device, button(device, name)?, LONG_MS),
        ["tap", x, y] => touch(device, Point::new(number(x)?, number(y)?), None, 60),
        ["long-tap", x, y] => touch(device, Point::new(number(x)?, number(y)?), None, LONG_MS),
        ["swipe", x1, y1, x2, y2] => touch(
            device,
            Point::new(number(x1)?, number(y1)?),
            Some(Point::new(number(x2)?, number(y2)?)),
            150,
        ),
        [name] => press(device, button(device, name)?, 60),
        _ => return Err("unknown step".into()),
    }
    Ok(())
}

fn button(device: &Device, name: &str) -> Result<&'static str, String> {
    device
        .profile
        .buttons
        .iter()
        .copied()
        .find(|b| *b == name)
        .ok_or_else(|| {
            format!(
                "`{}` has no button `{name}` (buttons: {})",
                device.profile.device.id,
                device.profile.buttons.join(", ")
            )
        })
}

fn press(device: &mut Device, name: &'static str, hold_ms: u64) {
    let event = |state| InputEvent::Button { name, state };
    device.input(event(ButtonState::Pressed));
    hold(device, hold_ms);
    device.input(event(ButtonState::Released));
}

fn touch(device: &mut Device, start: Point, end: Option<Point>, hold_ms: u64) {
    let event = |phase, point| {
        InputEvent::Pointer(PointerEvent {
            id: 0,
            phase,
            point,
            tool: PointerTool::Finger,
            pressure: 1000,
        })
    };
    device.input(event(PointerPhase::Down, start));
    hold(device, hold_ms);
    let end = end.unwrap_or(start);
    if end != start {
        device.input(event(PointerPhase::Move, end));
    }
    device.input(event(PointerPhase::Up, end));
}

/// Lets time pass in small steps so due long presses fire on time.
/// Lets `millis` of simulated time pass, waking the kernel at every time it
/// asked for, exactly like a device would.
fn hold(device: &mut Device, millis: u64) {
    let end = device.now().add_millis(millis);
    loop {
        let now = device.now();
        let next = device.next_wake().filter(|&at| at < end).unwrap_or(end);
        // At least 1 ms per round, so "as soon as possible" requests cannot
        // stall simulated time.
        device.advance(next.since(now).max(1));
        device.tick_if_due();
        if next >= end {
            return;
        }
    }
}

/// Writes what the panel currently shows to a PNG file.
pub fn screenshot(device: &Device, path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let file = File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut encoder = png::Encoder::new(
        BufWriter::new(file),
        u32::from(device.panel.width()),
        u32::from(device.panel.height()),
    );
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer
        .write_image_data(&device.panel.rgb_bytes())
        .map_err(|e| e.to_string())
}
