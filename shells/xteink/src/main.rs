//! QuireInk on the Xteink X4 (ESP32-C3): the kernel with the standard
//! app, its screen, buttons, battery and SD card.
//!
//! - **Screen**: the kernel draws in bands of [`FRAME_ROWS`] rows; each
//!   goes straight into the display controller's memory, which keeps the
//!   frame (see `quireink_x4::display`).
//! - **Buttons**: read every [`POLL_MS`] and debounced
//!   (`quireink_x4::buttons`).
//! - **Storage**: the SD card's FAT file system, with long file names
//!   (`quireink_fat`). Without a card, QuireInk starts with nothing to
//!   read.
//! - **Sleep**: the sleep screen stays shown; the battery's power is cut,
//!   and the power button starts the X4 again, where it was.

#![no_std]
#![no_main]

extern crate alloc;

mod card;
mod remote;

use alloc::boxed::Box;
use core::cell::RefCell;

use embedded_hal_bus::spi::RefCellDevice;
use esp_backtrace as _;
use esp_hal::analog::adc::{Adc, AdcCalCurve, AdcConfig, Attenuation};
use esp_hal::clock::CpuClock;
use esp_hal::delay::Delay;
use esp_hal::gpio::{Event, Input, InputConfig, Level, Output, OutputConfig, Pull, WakeupConfig};
use esp_hal::main;
use esp_hal::peripherals::ADC1;
use esp_hal::rtc_cntl::sleep::{LowPower, RtcSleepConfig};
use esp_hal::spi::Mode;
use esp_hal::spi::master::{Config as SpiConfig, Spi};
use esp_hal::time::Rate;
use log::{info, warn};
use quireink_core::{Effects, Kernel};
use quireink_fat::fatfs::{FileSystem, FsOptions};
use quireink_fat::{BlockDisk, FatStorage, VolumeError};
use quireink_platform::{
    Battery, ButtonState, DeviceInfo, DisplayInfo, InputCaps, InputEvent, Instant, MemoryClass,
    PixelFormat, RefreshMode, Rotation, SystemStatus,
};
use quireink_x4::battery;
use quireink_x4::buttons::{self, Debouncer, State};
use quireink_x4::display::{self, Display, Planes, Refresh};

esp_bootloader_esp_idf::esp_app_desc!();

/// Heap for QuireInk: what the emulator's X4 profile allows.
const HEAP_SIZE: usize = 192 * 1024;
/// Rows the kernel draws at once: 11.3 KiB of 2-bit gray.
const FRAME_ROWS: u16 = 96;
/// How often the buttons are read.
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
        format: PixelFormat::Gray2,
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
        match RefCellDevice::new(bus, card_select, delay) {
            Ok(device) => {
                let sd = embedded_sdmmc::SdCard::new(device, delay);
                let size = sd.num_bytes();
                let _ = bus
                    .borrow_mut()
                    .apply_config(&spi_config.with_frequency(Rate::from_mhz(SPI_MHZ)));
                match size {
                    Ok(bytes) => {
                        info!("SD card: {} MB", bytes >> 20);
                        let fs = BlockDisk::volume(card::Card(sd)).and_then(|disk| {
                            FileSystem::new(disk, FsOptions::new()).map_err(|_| VolumeError::NoFat)
                        });
                        match fs {
                            Ok(fs) => Box::new(FatStorage::new(Box::leak(Box::new(fs)))),
                            Err(e) => {
                                match e {
                                    VolumeError::ExFat => {
                                        warn!("SD card: exFAT; QuireInk reads FAT32")
                                    }
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
                }
            }
            Err(_) => Box::new(quireink_platform::NoStorage),
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
    let mut front = adc_config.enable_pin(p.GPIO1, Attenuation::_11dB);
    let mut side = adc_config.enable_pin(p.GPIO2, Attenuation::_11dB);
    let mut cell =
        adc_config.enable_pin_with_cal::<_, AdcCalCurve<ADC1>>(p.GPIO0, Attenuation::_11dB);
    let mut adc = Adc::new(p.ADC1, adc_config);
    let mut power = Input::new(p.GPIO3, InputConfig::default().with_pull(Pull::Up));
    let usb = Input::new(p.GPIO20, InputConfig::default());
    let mut debouncer = Debouncer::new();
    // Commands from a computer on USB; the log goes out the same way.
    let mut remote = remote::Remote::new(p.USB_DEVICE);
    // A button pressed over USB, and when to let it go.
    let mut remote_release: Option<(&'static str, Instant)> = None;
    // Woken by the power button, it is still down: it counts once released.
    let mut power_armed = false;

    let builder = Kernel::builder(DEVICE)
        .storage_boxed(storage)
        .glyph_cache(0)
        .frame_rows(FRAME_ROWS)
        .row_align(8)
        .sleep(true)
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
            while power.is_low() {
                delay.delay_millis(10);
            }
            // On battery, this ends it all: the power button starts the X4
            // again. On USB power, it sleeps until the button.
            power_latch.set_low();
            power.listen(Event::LowLevel);
            let _ = power.apply_wakeup_config(&WakeupConfig::default().with_low_power_path(true));
            let mut low_power = LowPower::new(p.LPWR);
            low_power.sleep_deep(RtcSleepConfig::deep());
        }

        let at = now();
        if status_read.is_none_or(|read| at.since(read) >= STATUS_MS) {
            status_read = Some(at);
            let measured = nb::block!(adc.read_oneshot(&mut cell)).unwrap_or(0);
            let percent = battery::percent(battery::millivolts(measured));
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

        let front_level = nb::block!(adc.read_oneshot(&mut front)).unwrap_or(4095);
        let side_level = nb::block!(adc.read_oneshot(&mut side)).unwrap_or(4095);
        power_armed |= power.is_high();
        let reading = State::read(front_level, side_level, power_armed && power.is_low());
        let mut changes: [Option<(&'static str, bool)>; 7] = [None; 7];
        let mut n = 0;
        debouncer.update(reading, at.0, &mut |name, down| {
            if n < changes.len() {
                changes[n] = Some((name, down));
                n += 1;
            }
        });
        for (name, down) in changes.into_iter().flatten() {
            let state = if down {
                ButtonState::Pressed
            } else {
                ButtonState::Released
            };
            let effects = kernel.handle(now(), InputEvent::Button { name, state });
            sleep |= apply(&mut kernel, &mut display, effects, &mut wake_at);
        }

        if let Some(command) = remote.poll() {
            match command {
                remote::Command::Press(name, ms) if remote_release.is_none() => {
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
                remote::Command::Press(..) => esp_println::println!("QI:BUSY"),
                remote::Command::Shot => {
                    remote::shot(&mut kernel);
                }
                remote::Command::Describe => remote::describe(&mut kernel),
                remote::Command::Screen => {
                    let id = kernel.active_screen().map_or("", |id| id.as_str());
                    esp_println::println!("QI:SCREEN {id}");
                }
                remote::Command::Heap => {
                    esp_println::println!(
                        "QI:HEAP {} {}",
                        esp_alloc::HEAP.used(),
                        esp_alloc::HEAP.free()
                    );
                }
            }
        }
        if let Some((name, _)) = remote_release.filter(|(_, at)| *at <= now()) {
            remote_release = None;
            let released = ButtonState::Released;
            let effects = kernel.handle(
                now(),
                InputEvent::Button {
                    name,
                    state: released,
                },
            );
            sleep |= apply(&mut kernel, &mut display, effects, &mut wake_at);
            esp_println::println!("QI:OK");
        }

        if wake_at.is_some_and(|due| due <= now()) {
            let effects = kernel.tick(now());
            sleep |= apply(&mut kernel, &mut display, effects, &mut wake_at);
        }
        delay.delay_millis(POLL_MS as u32);
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
    if let Some((_, mode)) = effects.refresh() {
        let refresh = match mode {
            RefreshMode::Full => Refresh::Full,
            RefreshMode::Partial | RefreshMode::Fast => Refresh::Fast,
        };
        let planes = if refresh == Refresh::Fast {
            Planes::NEW
        } else {
            Planes::BOTH
        };
        let mut failed = None;
        kernel.render(&mut |top, rows| {
            if let Err(e) = display.write_rows(top, rows, planes) {
                failed = Some(e);
            }
        });
        if let Err(e) = display.refresh(refresh, false) {
            failed = Some(e);
        }
        if refresh == Refresh::Fast {
            // The frame now shown, for the next fast refresh to compare.
            kernel.rerender(&mut |top, rows| {
                if let Err(e) = display.write_rows(top, rows, Planes::SHOWN) {
                    failed = Some(e);
                }
            });
        }
        if let Some(e) = failed {
            warn!("display: {e:?}");
        }
    }
    for warning in kernel.take_warnings() {
        warn!("{warning}");
    }
    *wake_at = effects.wake_at();
    effects.sleep()
}
