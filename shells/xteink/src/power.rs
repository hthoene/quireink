//! The processor's sleep while there is nothing to do: on battery it
//! sleeps lightly (some 0.1 mA, where spinning at full speed takes some
//! 20) until something is due, and reads the buttons on waking, since the
//! timer that reads them stops meanwhile. On USB it stays awake: a light
//! sleep cuts the link to the computer (its log, remote control).
//!
//! Two watchdogs start the X4 again if the firmware stops coming by: the
//! RTC's, which counts on in light sleep too, and a timer group's.

use core::cell::RefCell;
use core::sync::atomic::{AtomicBool, Ordering};

use critical_section::Mutex;
use esp_hal::rtc_cntl::sleep::{LowPower, RtcSleepConfig};
use esp_hal::time::{Duration, Instant};

static LOW_POWER: Mutex<RefCell<Option<LowPower<'static>>>> = Mutex::new(RefCell::new(None));

/// Where the firmware was last (see [`mark`]), kept through a reset: after
/// a watchdog's reset it tells where it hung.
#[esp_hal::ram(unstable(rtc_fast, persistent))]
static mut WHERE: u32 = 0;

/// Light sleeps, kept through a reset (for trying them over USB, which
/// they cut off).
#[esp_hal::ram(unstable(rtc_fast, persistent))]
static mut NAPS: u32 = 0;

/// What [`WHERE`] and [`NAPS`] said at start-up, with the reason for the
/// reset.
static AT_START: Mutex<RefCell<(u32, u32, Option<esp_hal::rtc_cntl::SocResetReason>)>> =
    Mutex::new(RefCell::new((0, 0, None)));

/// Places the firmware notes being at (see [`mark`]).
pub mod at {
    pub const LOOP: u32 = 1;
    pub const NAP_IDLE: u32 = 2;
    pub const WOKEN: u32 = 4;
    pub const BUTTONS_READ: u32 = 5;
    pub const INPUT: u32 = 6;
    pub const TICK: u32 = 7;
    pub const SHOWING: u32 = 8;
    pub const STATUS: u32 = 9;
    /// A try of light sleeps over USB ended (by starting again).
    pub const TRIED: u32 = 10;
    /// In a long light sleep tried over USB.
    pub const TRIED_NAP: u32 = 11;
}

/// Notes being at `place` (one of [`at`]).
pub fn mark(place: u32) {
    // SAFETY: one core, written only here and read once at start-up.
    unsafe { core::ptr::addr_of_mut!(WHERE).write_volatile(place) };
}

/// Notes how the last run ended; call once at start-up.
pub fn note_start() {
    // SAFETY: as in `mark`.
    let place = unsafe { core::ptr::addr_of!(WHERE).read_volatile() };
    // SAFETY: as in `mark`.
    let naps = unsafe { core::ptr::addr_of!(NAPS).read_volatile() };
    unsafe { core::ptr::addr_of_mut!(NAPS).write_volatile(0) };
    let reason = esp_hal::system::reset_reason();
    critical_section::with(|cs| AT_START.replace(cs, (place, naps, reason)));
}

/// Where the last run was when it ended, its light sleeps and the reason
/// for the reset.
pub fn last_run() -> (u32, u32, Option<esp_hal::rtc_cntl::SocResetReason>) {
    critical_section::with(|cs| *AT_START.borrow_ref(cs))
}

fn count_nap() {
    // SAFETY: as in `mark`.
    unsafe {
        let naps = core::ptr::addr_of_mut!(NAPS);
        naps.write_volatile(naps.read_volatile().wrapping_add(1));
    }
}

/// Light sleeps are allowed (on battery).
static ALLOWED: AtomicBool = AtomicBool::new(false);

/// Waits shorter than this spin: sleeping takes its own time to start and
/// end.
const MIN_NAP_US: u64 = 3000;

pub fn init(low_power: LowPower<'static>) {
    critical_section::with(|cs| LOW_POWER.replace(cs, Some(low_power)));
}

/// Allows light sleeps (on battery) or not (on USB).
pub fn allow(allowed: bool) {
    ALLOWED.store(allowed, Ordering::Relaxed);
}

/// Sleeps lightly until `until` (milliseconds since start) where allowed,
/// then reads the buttons; `false` if it did not (the caller waits).
pub fn nap_until(until: u64) -> bool {
    let deadline = Instant::EPOCH + Duration::from_millis(until);
    mark(at::NAP_IDLE);
    nap(deadline)
}

/// Sleeps lightly until `deadline` where allowed.
fn nap(deadline: Instant) -> bool {
    if !ALLOWED.load(Ordering::Relaxed) {
        return false;
    }
    sleep_until(deadline)
}

/// Sleeps lightly until `deadline`, if it is far enough; whether it did.
fn sleep_until(deadline: Instant) -> bool {
    feed();
    if deadline <= Instant::now() + Duration::from_micros(MIN_NAP_US) {
        return false;
    }
    // Taken out for the sleep: the interrupt that reads the buttons must
    // find the lock free.
    let Some(mut low_power) = critical_section::with(|cs| LOW_POWER.borrow_ref_mut(cs).take())
    else {
        return false;
    };
    low_power.set_wakeup_deadline(deadline);
    low_power.sleep_light(RtcSleepConfig::default());
    critical_section::with(|cs| LOW_POWER.replace(cs, Some(low_power)));
    mark(at::WOKEN);
    count_nap();
    crate::input::sample_now();
    mark(at::BUTTONS_READ);
    true
}

/// Turns the power off; only the power button wakes the X4 (it starts
/// again).
pub fn deep() -> ! {
    let low_power = critical_section::with(|cs| LOW_POWER.borrow_ref_mut(cs).take());
    let Some(mut low_power) = low_power else {
        panic!("no low power control");
    };
    // No alarm of a light sleep may end it, nor a watchdog.
    low_power.clear_wakeup_deadline();
    critical_section::with(|cs| {
        if let Some((rtc, wdt)) = WATCHDOGS.borrow_ref_mut(cs).as_mut() {
            rtc.rwdt.disable();
            wdt.disable();
        }
    });
    low_power.sleep_deep(RtcSleepConfig::deep())
}

type TimerWatchdog = esp_hal::timer::timg::Wdt<esp_hal::peripherals::TIMG1<'static>>;

static WATCHDOGS: Mutex<RefCell<Option<(esp_hal::rtc_cntl::Rtc<'static>, TimerWatchdog)>>> =
    Mutex::new(RefCell::new(None));

/// Starts the X4 again if the firmware does not come by: after `rtc_s`
/// seconds by the RTC's watchdog, which counts on in light sleep (a sleep
/// that does not end), after `timer_s` seconds by the timer group's. Hung,
/// it would stay on (and on battery drain it) with nothing to be done.
pub fn watch(
    mut rtc: esp_hal::rtc_cntl::Rtc<'static>,
    mut wdt: TimerWatchdog,
    (rtc_s, timer_s): (u64, u64),
) {
    use esp_hal::rtc_cntl::{RwdtStage, RwdtStageAction};
    rtc.rwdt
        .set_stage_action(RwdtStage::Stage0, RwdtStageAction::ResetSystem);
    rtc.rwdt
        .set_timeout(RwdtStage::Stage0, Duration::from_secs(rtc_s));
    rtc.rwdt.enable();
    // It counts on in light sleep too (esp-hal pauses it there): a sleep
    // that does not end is what it is for.
    let regs = esp_hal::peripherals::LPWR::regs();
    // SAFETY: the key that unlocks the watchdog's registers, then locks.
    regs.wdtwprotect().write(|w| unsafe { w.bits(0x50D8_3AA1) });
    regs.wdtconfig0()
        .modify(|_, w| w.wdt_pause_in_slp().clear_bit());
    regs.wdtwprotect().write(|w| unsafe { w.bits(0) });
    wdt.set_timeout(
        esp_hal::timer::timg::MwdtStage::Stage0,
        Duration::from_secs(timer_s),
    );
    wdt.enable();
    critical_section::with(|cs| WATCHDOGS.replace(cs, Some((rtc, wdt))));
}

/// Tells the watchdogs the firmware is alive.
pub fn feed() {
    critical_section::with(|cs| {
        if let Some((rtc, wdt)) = WATCHDOGS.borrow_ref_mut(cs).as_mut() {
            rtc.rwdt.feed();
            wdt.feed();
        }
    });
}

/// One light sleep of `seconds` whatever the rest, then a new start: for
/// trying over USB that the RTC's watchdog ends a sleep that goes on too
/// long (it starts the X4 again before).
pub fn try_long_nap(seconds: u64) -> ! {
    mark(at::TRIED_NAP);
    sleep_until(Instant::now() + Duration::from_secs(seconds));
    mark(at::TRIED);
    esp_hal::system::software_reset()
}
