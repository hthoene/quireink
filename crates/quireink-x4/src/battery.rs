//! The X4's battery: its voltage, halved by a divider, on an ADC pin.

/// The battery's voltage, from the millivolts measured on its pin.
pub fn millivolts(measured: u16) -> u16 {
    measured.saturating_mul(2)
}

/// Empty and full, in millivolts.
pub const EMPTY: u16 = 3250;
pub const FULL: u16 = 4150;

/// The charge left, in percent, from the battery's voltage: a curve
/// fitted to a lithium polymer cell's discharge between [`EMPTY`] and
/// [`FULL`] (outside, it bends back).
pub fn percent(millivolts: u16) -> u8 {
    if millivolts <= EMPTY {
        return 0;
    }
    if millivolts >= FULL {
        return 100;
    }
    let v = f32::from(millivolts) / 1000.0;
    let y = ((-144.939 * v + 1655.8629) * v - 6158.852) * v + 7501.3202;
    if y.is_nan() || y <= 0.0 {
        0
    } else if y >= 100.0 {
        100
    } else {
        // Rounded, without `f32::round` (not in `core`).
        (y + 0.5) as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn charge_by_voltage() {
        assert_eq!(millivolts(2000), 4000);
        assert_eq!(percent(4250), 100);
        assert_eq!(percent(3000), 0);
        let mid = percent(3800);
        assert!((40..70).contains(&mid), "{mid}");
        // Higher is never less.
        let mut last = 0;
        for mv in (3300..4200).step_by(10) {
            let p = percent(mv);
            assert!(p >= last, "{mv} mV: {p} < {last}");
            last = p;
        }
    }
}
