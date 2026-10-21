//! Percent <-> raw VCP mapping, including per-monitor calibration ranges (spec §4.1).

/// Raw value -> 0–100 %. A monitor reporting `max == 0` yields 0.
pub fn raw_to_percent(raw: u32, max: u32) -> u8 {
    if max == 0 {
        return 0;
    }
    let raw = raw.min(max) as u64;
    ((raw * 100 + max as u64 / 2) / max as u64) as u8
}

/// 0–100 % -> raw value in `0..=max`. Input above 100 is clamped.
pub fn percent_to_raw(percent: u8, max: u32) -> u32 {
    let p = percent.min(100) as u64;
    ((p * max as u64 + 50) / 100) as u32
}

/// Maps a common percentage onto a calibrated percent range `[lo, hi]`
/// (e.g. LG 0–100, AOC 10–85). Result is clamped into the range.
pub fn calibrate(percent: u8, lo: u8, hi: u8) -> u8 {
    let (lo, hi) = if lo <= hi { (lo, hi) } else { (hi, lo) };
    let p = percent.min(100) as u32;
    let span = (hi - lo) as u32;
    (lo as u32 + (p * span + 50) / 100) as u8
}

/// Inverse of [`calibrate`]: monitor percent -> common percent.
pub fn uncalibrate(percent: u8, lo: u8, hi: u8) -> u8 {
    let (lo, hi) = if lo <= hi { (lo, hi) } else { (hi, lo) };
    if hi == lo {
        return 0;
    }
    let p = percent.clamp(lo, hi) as u32 - lo as u32;
    ((p * 100 + (hi - lo) as u32 / 2) / (hi - lo) as u32) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_roundtrip_nonstandard_max() {
        for max in [100u32, 63, 255, 1000] {
            for p in 0..=100u8 {
                let raw = percent_to_raw(p, max);
                assert!(raw <= max);
                let back = raw_to_percent(raw, max);
                assert!((back as i32 - p as i32).abs() <= 1, "max={max} p={p} back={back}");
            }
        }
    }

    #[test]
    fn endpoints() {
        assert_eq!(percent_to_raw(0, 63), 0);
        assert_eq!(percent_to_raw(100, 63), 63);
        assert_eq!(raw_to_percent(5, 0), 0);
        assert_eq!(raw_to_percent(200, 100), 100);
        assert_eq!(percent_to_raw(250, 100), 100);
    }

    #[test]
    fn calibration_example_from_spec() {
        // AOC 10–85
        assert_eq!(calibrate(0, 10, 85), 10);
        assert_eq!(calibrate(100, 10, 85), 85);
        assert_eq!(calibrate(50, 10, 85), 48);
        // LG 0–100 is identity
        for p in 0..=100u8 {
            assert_eq!(calibrate(p, 0, 100), p);
        }
    }

    #[test]
    fn calibration_inverse_and_clamp() {
        for p in 0..=100u8 {
            let m = calibrate(p, 10, 85);
            assert!((10..=85).contains(&m));
            assert!((uncalibrate(m, 10, 85) as i32 - p as i32).abs() <= 2);
        }
        // value outside range is clamped (spec §9)
        assert_eq!(uncalibrate(5, 10, 85), 0);
        assert_eq!(uncalibrate(99, 10, 85), 100);
        assert_eq!(uncalibrate(30, 40, 40), 0);
    }
}
