//! QuireInk on the Xteink X4 (ESP32-C3): the kernel with the standard
//! app, its screen, buttons, battery and SD card.
//!
//! - **Screen**: the kernel draws in bands of [`FRAME_ROWS`] rows; each
//!   goes straight into the display controller's memory, which keeps the
//!   frame (see `quireink_x4::display`).
//! - **Buttons**: read in a timer's interrupt and debounced, so presses
//!   count while QuireInk is busy (`input`, `quireink_x4::buttons`).
//! - **Storage**: the SD card's FAT file system, with long file names
//!   (`quireink_fat`). Without a card, QuireInk starts with nothing to
//!   read.
//! - **Sleep**: the sleep screen stays shown; the battery's power is cut,
//!   and the power button starts the X4 again, where it was.

#![no_std]
#![no_main]

extern crate alloc;

mod card;
mod input;
mod remote;
mod sent;
mod traced;

use alloc::boxed::Box;
use core::cell::RefCell;

use embedded_hal_bus::spi::RefCellDevice;
use esp_backtrace as _;
use esp_hal::analog::adc::{Adc, AdcCalBasic, AdcCalCurve, AdcConfig, Attenuation};
use esp_hal::clock::CpuClock;
use esp_hal::delay::Delay;
use esp_hal::gpio::{Event, Input, InputConfig, Level, Output, OutputConfig, Pull, WakeupConfig};
use esp_hal::main;
use esp_hal::peripherals::ADC1;
use esp_hal::rtc_cntl::sleep::{LowPower, RtcSleepConfig};
use esp_hal::spi::Mode;
use esp_hal::spi::master::{Config as SpiConfig, Spi};
use esp_hal::time::Rate;
use esp_hal::timer::PeriodicTimer;
use esp_hal::timer::timg::TimerGroup;
use log::{info, warn};
use quireink_core::{Effects, Kernel};
use quireink_fat::fatfs::{FileSystem, FsOptions};
use quireink_fat::{BlockDisk, FatStorage, VolumeError};
use quireink_platform::{
    Battery, ButtonState, DeviceInfo, DisplayInfo, InputCaps, InputEvent, Instant, MemoryClass,
    PixelFormat, RefreshMode, Rotation, SystemStatus,
};
use quireink_x4::battery;
use quireink_x4::buttons;
use quireink_x4::display::{self, Display, Planes, Refresh};

esp_bootloader_esp_idf::esp_app_desc!();

/// Heap for QuireInk: what the emulator's X4 profile allows.
const HEAP_SIZE: usize = 192 * 1024;
/// More heap in the memory the bootloader used (65 KB are free after it):
/// room for the frame kept for the panel (see [`sent`]).
const RECLAIMED_SIZE: usize = 64 * 1024;
/// Rows the kernel draws at once: 11.3 KiB of black and white. Fewer
/// bands draw faster: each costs the screens a pass over what they show.
const FRAME_ROWS: u16 = 192;
/// How long the main loop rests when there is nothing to do.
const POLL_MS: u64 = 10;
/// How often the battery is read.
const STATUS_MS: u64 = 60_000;
/// SPI speeds: an SD card starts slowly.
const CARD_START_KHZ: u32 = 400;
const SPI_MHZ: u32 = 20;

const DEVICE: DeviceInfo = DeviceInfo {
    id: "xteink-x4",
    name: "Xteink X4",
    display: DisplayInfo {
        width: display::WIDTH,
        height: display::HEIGHT,
        dpi: 220,
        // What the panel shows until its four grays come: drawing in gray
        // only to send black and white costs time and memory.
        format: PixelFormat::Mono1,
        rotation: Rotation::Deg90,
        color: None,
    },
    input: InputCaps {
        buttons: true,
        touch: false,
        pen: false,
        keys: &buttons::ALL,
    },
    memory: MemoryClass::Small,
};

fn now() -> Instant {
    Instant(
        esp_hal::time::Instant::now()
            .duration_since_epoch()
            .as_millis(),
    )
}

#[main]
fn main() -> ! {
    let p = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    // The battery's power stays on only while this pin holds it.
    let mut power_latch = Output::new(p.GPIO13, Level::High, OutputConfig::default());
    esp_println::logger::init_logger_from_env();
    esp_alloc::heap_allocator!(size: HEAP_SIZE);
    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: RECLAIMED_SIZE);
    let delay = Delay::new();
    info!("QuireInk starting");

    // One SPI bus for the display and the card.
    let spi_config = SpiConfig::default()
        .with_frequency(Rate::from_khz(CARD_START_KHZ))
        .with_mode(Mode::_0);
    let spi = match Spi::new(p.SPI2, spi_config) {
        Ok(spi) => spi.with_sck(p.GPIO8).with_mosi(p.GPIO10).with_miso(p.GPIO7),
        Err(e) => panic!("SPI: {e:?}"),
    };
    let bus: &'static RefCell<_> = Box::leak(Box::new(RefCell::new(spi)));
    let display_select = Output::new(p.GPIO21, Level::High, OutputConfig::default());
    let card_select = Output::new(p.GPIO12, Level::High, OutputConfig::default());
    // A card wants clocks with nothing selected before it starts.
    let _ = bus.borrow_mut().write(&[0xFF; 10]);

    // The card, then the bus at full speed.
    let storage: Box<dyn quireink_platform::Storage> =
        match quireink_x4::card::Card::new(bus, card_select, delay) {
            Ok(sd) => {
                let _ = bus
                    .borrow_mut()
                    .apply_config(&spi_config.with_frequency(Rate::from_mhz(SPI_MHZ)));
                info!("SD card: {} MB", sd.blocks() >> 11);
                let fs = BlockDisk::volume(card::Card(sd)).and_then(|disk| {
                    FileSystem::new(disk, FsOptions::new()).map_err(|_| VolumeError::NoFat)
                });
                match fs {
                    Ok(fs) => Box::new(FatStorage::new(Box::leak(Box::new(fs)))),
                    Err(e) => {
                        match e {
                            VolumeError::ExFat => warn!("SD card: exFAT; QuireInk reads FAT32"),
                            VolumeError::NoFat => warn!("SD card: no FAT file system"),
                            VolumeError::Disk => warn!("SD card: not readable"),
                        }
                        Box::new(quireink_platform::NoStorage)
                    }
                }
            }
            Err(e) => {
                warn!("SD card: {e:?}");
                Box::new(quireink_platform::NoStorage)
            }
        };

    // The display.
    let display_device = match RefCellDevice::new(bus, display_select, delay) {
        Ok(device) => device,
        Err(_) => panic!("display SPI"),
    };
    let mut display = Display::new(
        display_device,
        Output::new(p.GPIO4, Level::High, OutputConfig::default()),
        Output::new(p.GPIO5, Level::High, OutputConfig::default()),
        Input::new(p.GPIO6, InputConfig::default()),
        delay,
    );
    if let Err(e) = display.init() {
        warn!("display: {e:?}");
    }

    // Buttons and battery.
    let mut adc_config = AdcConfig::new();
    // Calibrated for the ADC's offset: uncalibrated, readings came out
    // some 1600 too high, and the two highest buttons of each ladder were
    // lost above the top of the range.
    let front = adc_config.enable_pin_with_cal::<_, AdcCalBasic<ADC1>>(p.GPIO1, Attenuation::_11dB);
    let side = adc_config.enable_pin_with_cal::<_, AdcCalBasic<ADC1>>(p.GPIO2, Attenuation::_11dB);
    let cell = adc_config.enable_pin_with_cal::<_, AdcCalCurve<ADC1>>(p.GPIO0, Attenuation::_11dB);
    let adc = Adc::new(p.ADC1, adc_config);
    let power = Input::new(p.GPIO3, InputConfig::default().with_pull(Pull::Up));
    let usb = Input::new(p.GPIO20, InputConfig::default());
    let timers = TimerGroup::new(p.TIMG0);
    input::start(input::Sampler::new(
        PeriodicTimer::new(timers.timer0),
        adc,
        front,
        side,
        cell,
        power,
    ));
    // Commands from a computer on USB; the log goes out the same way.
    let mut remote = remote::Remote::new(p.USB_DEVICE);
    // A button pressed over USB, and when to let it go.
    let mut remote_release: Option<(&'static str, Instant)> = None;
    // The latest time the kernel was given: presses that waited in the
    // queue keep their own time, but never one before it.
    let mut kernel_time = 0;

    let builder = Kernel::builder(DEVICE)
        .storage_boxed(Box::new(traced::Traced(storage)))
        .glyph_cache(0)
        .frame_rows(FRAME_ROWS)
        .row_align(8)
        .sleep(true)
        .button_row(&buttons::ROW)
        // A full refresh takes 3.5 s on its panel: screens change fast.
        .setting_default("core.flash-screens", "false")
        .setting_default("core.ui-size", "9")
        .setting_default("reader.font-size", "9");
    let mut kernel = match quireink_app::install(builder, quireink_app::Fonts::Packs).build() {
        Ok(kernel) => kernel,
        Err(e) => panic!("kernel: {e}"),
    };
    for warning in kernel.take_warnings() {
        warn!("{warning}");
    }

    let mut status_read: Option<Instant> = None;
    let mut wake_at: Option<Instant> = None;
    let effects = kernel.start(now());
    let mut sleep = apply(&mut kernel, &mut display, effects, &mut wake_at);

    loop {
        if sleep {
            // Plugins save what is pending; the screen stays as it is.
            let effects = kernel.suspend(now());
            apply(&mut kernel, &mut display, effects, &mut wake_at);
            let _ = display.sleep();
            info!("sleeping");
            // Released first: held, the button would wake it at once.
            while input::power_down() {
                delay.delay_millis(10);
            }
            // On USB power it stays in reach of a computer (its log, remote
            // control): the sleep screen shows until the power button, or
            // `press power` over USB, wakes it.
            if usb.is_high() {
                loop {
                    delay.delay_millis(POLL_MS as u32);
                    if input::power_down() {
                        break;
                    }
                    if let Some(command) = remote.poll()
                        && let Some((name, _)) =
                            remote.answer(&mut kernel, &DEVICE.display, command)
                    {
                        esp_println::println!("QI:OK");
                        if name == quireink_platform::buttons::POWER {
                            break;
                        }
                    }
                }
                while input::power_down() {
                    delay.delay_millis(10);
                }
                input::rearm();
                if let Err(e) = display.init() {
                    warn!("display: {e:?}");
                }
                info!("awake");
                let effects = kernel.resume(now());
                sleep = apply(&mut kernel, &mut display, effects, &mut wake_at);
                continue;
            }
            // On battery, this ends it all: the power button starts the X4
            // again.
            let Some(mut power) = input::stop() else {
                panic!("the power button is gone");
            };
            power_latch.set_low();
            power.listen(Event::LowLevel);
            let _ = power.apply_wakeup_config(&WakeupConfig::default().with_low_power_path(true));
            let mut low_power = LowPower::new(p.LPWR);
            low_power.sleep_deep(RtcSleepConfig::deep());
        }

        let at = now();
        if status_read.is_none_or(|read| at.since(read) >= STATUS_MS) {
            status_read = Some(at);
            let percent = battery::percent(input::millivolts());
            let status = SystemStatus {
                clock: None,
                battery: Some(Battery {
                    percent,
                    charging: usb.is_high(),
                }),
            };
            let effects = kernel.set_status(at, status);
            sleep |= apply(&mut kernel, &mut display, effects, &mut wake_at);
        }

        // Presses first, before any work in the background.
        while let Some((name, down, at)) = input::next() {
            let state = if down {
                ButtonState::Pressed
            } else {
                ButtonState::Released
            };
            kernel_time = at.max(kernel_time);
            let started = now();
            let effects = kernel.handle(Instant(kernel_time), InputEvent::Button { name, state });
            info!("{name}: {} ms", now().since(started));
            sleep |= apply(&mut kernel, &mut display, effects, &mut wake_at);
        }

        let command = remote.poll();
        if let Some(remote::Command::Refresh(full)) = command {
            let refresh = if full { Refresh::Full } else { Refresh::Fast };
            let result = display.refresh(refresh, false);
            esp_println::println!("QI:OK {result:?}");
        } else if let Some(command) = command
            && let Some((name, ms)) = remote.answer(&mut kernel, &DEVICE.display, command)
        {
            if remote_release.is_some() {
                esp_println::println!("QI:BUSY");
            } else {
                let pressed = ButtonState::Pressed;
                let effects = kernel.handle(
                    now(),
                    InputEvent::Button {
                        name,
                        state: pressed,
                    },
                );
                sleep |= apply(&mut kernel, &mut display, effects, &mut wake_at);
                remote_release = Some((name, Instant(now().0 + ms)));
            }
        }
        if let Some((name, _)) = remote_release.filter(|(_, at)| *at <= now()) {
            remote_release = None;
            let released = ButtonState::Released;
            let started = now();
            let effects = kernel.handle(
                now(),
                InputEvent::Button {
                    name,
                    state: released,
                },
            );
            info!("{name}: {} ms", now().since(started));
            sleep |= apply(&mut kernel, &mut display, effects, &mut wake_at);
            esp_println::println!("QI:OK");
        }

        if wake_at.is_some_and(|due| due <= now()) {
            let started = now();
            kernel_time = started.0;
            let effects = kernel.tick(started);
            let took = now().since(started);
            if took > 50 {
                info!("tick: {took} ms");
            }
            sleep |= apply(&mut kernel, &mut display, effects, &mut wake_at);
        }
        delay.delay_millis(POLL_MS as u32);
    }
}

/// A check of the rows sent to a memory of the controller: their tops,
/// count and bytes (FNV-1a).
#[derive(Debug, Default, PartialEq, Eq)]
struct Check {
    rows: u32,
    hash: u32,
}

impl Check {
    fn add(&mut self, top: u16, rows: &quireink_platform::FrameView<'_>) {
        let bytes = rows
            .data
            .get(..usize::from(rows.height) * rows.stride)
            .unwrap_or(&[]);
        let mut hash = if self.hash == 0 {
            0x811C_9DC5
        } else {
            self.hash
        };
        for byte in top.to_le_bytes().iter().chain(bytes) {
            hash = (hash ^ u32::from(*byte)).wrapping_mul(0x0100_0193);
        }
        self.hash = hash;
        self.rows += u32::from(rows.height);
    }
}

/// Carries out what the kernel asks: shows what changed, notes when to
/// wake it, restarts. Returns whether to sleep.
fn apply<SPI, DC, RST, BUSY, D>(
    kernel: &mut Kernel,
    display: &mut Display<SPI, DC, RST, BUSY, D>,
    effects: Effects,
    wake_at: &mut Option<Instant>,
) -> bool
where
    SPI: embedded_hal::spi::SpiDevice,
    DC: embedded_hal::digital::OutputPin,
    RST: embedded_hal::digital::OutputPin,
    BUSY: embedded_hal::digital::InputPin,
    D: embedded_hal::delay::DelayNs,
{
    if effects.restart() {
        esp_hal::system::software_reset();
    }
    if let Some((area, mode)) = effects.refresh() {
        let refresh = match mode {
            // Every pixel driven with a warm panel's waveform (1.6 s), as the
            // stock firmware cleans the panel every few pages.
            RefreshMode::Full => Refresh::Half,
            RefreshMode::Partial | RefreshMode::Fast => Refresh::Fast,
        };
        let planes = if refresh == Refresh::Fast {
            Planes::NEW
        } else {
            Planes::BOTH
        };
        let mut failed = None;
        let started = now();
        // What is sent is kept for sending again after a fast refresh.
        let mut sent = sent::Sent::default();
        // What went into each memory of the controller, to check that the
        // frame shown is the frame sent.
        let (mut written, mut again_written) = (Check::default(), Check::default());
        kernel.render(&mut |top, rows| {
            if let Err(e) = display.write_rows(top, rows, planes) {
                failed = Some(e);
            }
            written.add(top, &rows);
            sent.keep(top, &rows);
        });
        let drawn = now();
        if let Err(e) = display.refresh(refresh, false) {
            failed = Some(e);
        }
        let shown = now();
        {
            // After every refresh, both memories again hold the frame now
            // shown, as the stock firmware does: the refresh may leave the
            // memory of the new frame changed, and the next fast refresh
            // compares the two (sent again as kept, or drawn anew).
            let mut again = |top: u16, rows: quireink_platform::FrameView<'_>| {
                if let Err(e) = display.write_rows(top, rows, Planes::BOTH) {
                    failed = Some(e);
                }
                again_written.add(top, &rows);
            };
            let kept = sent.replay(&mut again);
            if !kept {
                kernel.rerender(&mut again);
            }
            if again_written != written {
                warn!(
                    "frame shown differs from frame sent ({}): {written:?} then {again_written:?}",
                    if kept { "kept" } else { "drawn anew" }
                );
            }
        }
        if let Some(e) = failed {
            warn!("display: {e:?}");
        }
        info!(
            "frame {mode:?} {},{} {}x{}: drawn in {} ms, shown in {} ms, sent again in {} ms",
            area.x,
            area.y,
            area.width,
            area.height,
            drawn.since(started),
            shown.since(drawn),
            now().since(shown)
        );
    }
    for warning in kernel.take_warnings() {
        warn!("{warning}");
    }
    *wake_at = effects.wake_at();
    effects.sleep()
}
