//! The buttons and the battery, read in a timer's interrupt every
//! [`SAMPLE_MS`]: a press is seen while QuireInk is busy for a second or
//! more (laying out a page, counting a book's pages), and waits in a queue
//! until the main loop takes it ([`next`]). Each keeps the time it came,
//! so a long press stays long.
//!
//! The same interrupt can note where the processor is each time, for
//! finding out where the time goes ([`profile`]).

use alloc::vec::Vec;
use core::cell::RefCell;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use critical_section::Mutex;
use esp_hal::Blocking;
use esp_hal::analog::adc::{Adc, AdcCalBasic, AdcCalCurve, AdcPin};
use esp_hal::gpio::Input;
use esp_hal::handler;
use esp_hal::peripherals::{ADC1, GPIO0, GPIO1, GPIO2};
use esp_hal::time::Duration;
use esp_hal::timer::PeriodicTimer;
use quireink_x4::battery;
use quireink_x4::buttons::{Debouncer, State};

/// How often the buttons are read.
const SAMPLE_MS: u64 = 5;
/// How often the battery is read.
const BATTERY_MS: u64 = 1000;
/// Presses and releases waiting; more are dropped.
const QUEUE: usize = 32;

/// A button that went down (`true`) or up, and when (ms).
pub type Change = (&'static str, bool, u64);

/// What the interrupt reads with, and what it found.
pub struct Sampler {
    pub timer: PeriodicTimer<'static, Blocking>,
    pub adc: Adc<'static, ADC1<'static>, Blocking>,
    pub front: AdcPin<GPIO1<'static>, ADC1<'static>, AdcCalBasic<ADC1<'static>>>,
    pub side: AdcPin<GPIO2<'static>, ADC1<'static>, AdcCalBasic<ADC1<'static>>>,
    pub cell: AdcPin<GPIO0<'static>, ADC1<'static>, AdcCalCurve<ADC1<'static>>>,
    pub power: Input<'static>,
    debouncer: Debouncer,
    /// Woken by the power button, it is still down: it counts once let go.
    power_armed: bool,
    queue: [Option<Change>; QUEUE],
    /// Where the next change goes, and how many wait.
    head: usize,
    len: usize,
    battery_read: u64,
}

static SAMPLER: Mutex<RefCell<Option<Sampler>>> = Mutex::new(RefCell::new(None));
/// Where the processor was at each reading, while profiling.
static PROFILE: Mutex<RefCell<Option<Vec<u32>>>> = Mutex::new(RefCell::new(None));
/// Places noted at most (4 bytes each): 15 s.
const PROFILE_SAMPLES: usize = 3000;
/// Memory left free for QuireInk while profiling.
const PROFILE_SPARE: usize = 64 * 1024;
/// The battery's voltage, in millivolts (0 until first read).
static MILLIVOLTS: AtomicU32 = AtomicU32::new(0);
/// The power button is down now (not debounced).
static POWER_DOWN: AtomicBool = AtomicBool::new(false);
/// Readings so far, and the last ones of the two ladders, to look into.
static SAMPLES: AtomicU32 = AtomicU32::new(0);
static LEVELS: AtomicU32 = AtomicU32::new(0);

fn millis() -> u64 {
    esp_hal::time::Instant::now()
        .duration_since_epoch()
        .as_millis()
}

impl Sampler {
    pub fn new(
        mut timer: PeriodicTimer<'static, Blocking>,
        adc: Adc<'static, ADC1<'static>, Blocking>,
        front: AdcPin<GPIO1<'static>, ADC1<'static>, AdcCalBasic<ADC1<'static>>>,
        side: AdcPin<GPIO2<'static>, ADC1<'static>, AdcCalBasic<ADC1<'static>>>,
        cell: AdcPin<GPIO0<'static>, ADC1<'static>, AdcCalCurve<ADC1<'static>>>,
        power: Input<'static>,
    ) -> Self {
        timer.set_interrupt_handler(sample);
        Self {
            timer,
            adc,
            front,
            side,
            cell,
            power,
            debouncer: Debouncer::new(),
            power_armed: false,
            queue: [None; QUEUE],
            head: 0,
            len: 0,
            battery_read: 0,
        }
    }

    /// One reading of everything.
    fn read(&mut self) {
        let now = millis();
        if self.battery_read == 0 || now - self.battery_read >= BATTERY_MS {
            self.battery_read = now;
            if let Ok(raw) = nb::block!(self.adc.read_oneshot(&mut self.cell)) {
                MILLIVOLTS.store(u32::from(battery::millivolts(raw)), Ordering::Relaxed);
            }
        }
        let front = nb::block!(self.adc.read_oneshot(&mut self.front)).unwrap_or(4095);
        let side = nb::block!(self.adc.read_oneshot(&mut self.side)).unwrap_or(4095);
        // Only the interrupt writes it (the chip has no atomic adds).
        SAMPLES.store(
            SAMPLES.load(Ordering::Relaxed).wrapping_add(1),
            Ordering::Relaxed,
        );
        LEVELS.store(u32::from(front) << 16 | u32::from(side), Ordering::Relaxed);
        let power_down = self.power.is_low();
        POWER_DOWN.store(power_down, Ordering::Relaxed);
        self.power_armed |= !power_down;
        let reading = State::read(front, side, self.power_armed && power_down);
        let (queue, head, len) = (&mut self.queue, &mut self.head, &mut self.len);
        self.debouncer.update(reading, now, &mut |name, down| {
            if *len < QUEUE {
                queue[(*head + *len) % QUEUE] = Some((name, down, now));
                *len += 1;
            }
        });
    }
}

#[handler]
fn sample() {
    // The place the interrupt came at.
    let pc: u32;
    // SAFETY: reads a register, changes nothing.
    unsafe { core::arch::asm!("csrr {0}, mepc", out(reg) pc) };
    critical_section::with(|cs| {
        if let Some(places) = PROFILE.borrow_ref_mut(cs).as_mut()
            && places.len() < places.capacity()
        {
            places.push(pc);
        }
        if let Some(sampler) = SAMPLER.borrow_ref_mut(cs).as_mut() {
            sampler.timer.clear_interrupt();
            sampler.read();
        }
    });
}

/// Starts reading in the timer's interrupt.
pub fn start(mut sampler: Sampler) {
    sampler.read();
    let _ = sampler.timer.start(Duration::from_millis(SAMPLE_MS));
    sampler.timer.listen();
    critical_section::with(|cs| SAMPLER.replace(cs, Some(sampler)));
}

/// Reads the buttons now, e.g. after a light sleep, in which the timer
/// that reads them stops.
pub fn sample_now() {
    critical_section::with(|cs| {
        if let Some(sampler) = SAMPLER.borrow_ref_mut(cs).as_mut() {
            sampler.read();
        }
    });
}

/// The next press or release, oldest first.
pub fn next() -> Option<Change> {
    critical_section::with(|cs| {
        let mut sampler = SAMPLER.borrow_ref_mut(cs);
        let s = sampler.as_mut()?;
        if s.len == 0 {
            return None;
        }
        let change = s.queue[s.head].take();
        s.head = (s.head + 1) % QUEUE;
        s.len -= 1;
        change
    })
}

/// Starts noting where the processor is (`true`), or stops and gives what
/// was noted.
pub fn profile(start: bool) -> Option<Vec<u32>> {
    critical_section::with(|cs| {
        let mut profile = PROFILE.borrow_ref_mut(cs);
        if start {
            let mut places = Vec::new();
            if esp_alloc::HEAP.free() > PROFILE_SAMPLES * 4 + PROFILE_SPARE {
                let _ = places.try_reserve_exact(PROFILE_SAMPLES);
            }
            *profile = Some(places);
            None
        } else {
            profile.take()
        }
    })
}

/// The battery's voltage, in millivolts.
pub fn millivolts() -> u16 {
    MILLIVOLTS.load(Ordering::Relaxed) as u16
}

/// Readings so far, and the last readings of the front and side ladders.
pub fn stats() -> (u32, u16, u16) {
    let levels = LEVELS.load(Ordering::Relaxed);
    (
        SAMPLES.load(Ordering::Relaxed),
        (levels >> 16) as u16,
        levels as u16,
    )
}

/// Whether the power button is down now.
pub fn power_down() -> bool {
    POWER_DOWN.load(Ordering::Relaxed)
}

/// After the power button woke the X4 from its light sleep: that press
/// does not count, and what waits from before is dropped.
pub fn rearm() {
    critical_section::with(|cs| {
        if let Some(s) = SAMPLER.borrow_ref_mut(cs).as_mut() {
            s.power_armed = false;
            s.len = 0;
        }
    });
}

/// Stops reading and gives back the power button, e.g. to wake the X4
/// from its deep sleep.
pub fn stop() -> Option<Input<'static>> {
    critical_section::with(|cs| {
        let mut sampler = SAMPLER.borrow_ref_mut(cs).take()?;
        sampler.timer.unlisten();
        let _ = sampler.timer.cancel();
        Some(sampler.power)
    })
}
