//! What a device knows about the time of day and its battery, reported by
//! the shell (see `Kernel::set_status`).

use crate::input::Instant;

/// The shell's view of time and power. Unknown parts are `None`: a desktop
/// has no battery, a device without a clock chip no time of day.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SystemStatus {
    pub clock: Option<WallClock>,
    pub battery: Option<Battery>,
}

/// Wall-clock time: at the monotonic time `at`, it was `unix_millis`
/// milliseconds since 1970 (UTC). Later times follow from the monotonic
/// clock, so shells report it once and again only when it is corrected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WallClock {
    pub at: Instant,
    pub unix_millis: i64,
    /// The offset of the system's time zone from UTC at `at`, in minutes
    /// (e.g. 120 for summer time in Berlin), if the system has one.
    pub utc_offset_minutes: Option<i32>,
}

impl WallClock {
    /// Milliseconds since 1970 (UTC) at the monotonic time `now`.
    pub fn unix_millis_at(&self, now: Instant) -> i64 {
        self.unix_millis + (now.millis() as i64 - self.at.millis() as i64)
    }
}

/// The battery's charge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Battery {
    /// Charge from 0 to 100.
    pub percent: u8,
    pub charging: bool,
}
