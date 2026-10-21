//! Linked sliders (spec §4.1): one common 0–100 % value drives several monitors, each mapped
//! through its own calibration range so panels of different brightness look alike.

use crate::scale::{calibrate, uncalibrate};
use crate::store::Range;

/// Monitor percent each participant must be set to for a common value.
pub fn targets<'a>(common: u8, parts: &[(&'a str, Range)]) -> Vec<(&'a str, u8)> {
    parts.iter().map(|(id, r)| (*id, calibrate(common, r.min, r.max))).collect()
}

/// Common value implied by the monitors' current values: the mean of their un-calibrated
/// percentages. `None` when nobody participates.
pub fn common_value(current: &[(u8, Range)]) -> Option<u8> {
    if current.is_empty() {
        return None;
    }
    let sum: u32 = current.iter().map(|(p, r)| uncalibrate(*p, r.min, r.max) as u32).sum();
    Some(((sum + current.len() as u32 / 2) / current.len() as u32) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LG: Range = Range { min: 0, max: 100 };
    const AOC: Range = Range { min: 10, max: 85 };

    #[test]
    fn spec_example_lg_and_aoc() {
        let t = targets(100, &[("lg", LG), ("aoc", AOC)]);
        assert_eq!(t, vec![("lg", 100), ("aoc", 85)]);
        let t = targets(0, &[("lg", LG), ("aoc", AOC)]);
        assert_eq!(t, vec![("lg", 0), ("aoc", 10)]);
        let t = targets(50, &[("lg", LG), ("aoc", AOC)]);
        assert_eq!(t[0].1, 50);
        assert_eq!(t[1].1, 48);
    }

    #[test]
    fn common_value_roundtrips_within_rounding() {
        for common in 0..=100u8 {
            let monitor_values: Vec<(u8, Range)> =
                [LG, AOC].iter().map(|r| (calibrate(common, r.min, r.max), *r)).collect();
            let back = common_value(&monitor_values).unwrap();
            assert!((back as i32 - common as i32).abs() <= 2, "common={common} back={back}");
        }
    }

    #[test]
    fn no_participants() {
        assert_eq!(common_value(&[]), None);
        assert!(targets(50, &[]).is_empty());
    }

    #[test]
    fn values_outside_calibration_are_clamped() {
        // a monitor sitting at 5 % while calibrated 10–85 reads as common 0 %
        assert_eq!(common_value(&[(5, AOC)]), Some(0));
        assert_eq!(common_value(&[(99, AOC)]), Some(100));
    }
}
