//! Schedule maths (spec §4.4): sunrise/sunset from coordinates and the value the schedule
//! prescribes at a given moment, including smooth transitions between points.
//! Pure functions only; the clock, monitors and writes live in the app.

use std::collections::HashMap;

use crate::types::Feature;

pub const DAY: f64 = 1440.0;

/// `"HH:MM"` -> minutes since midnight.
pub fn parse_hhmm(s: &str) -> Option<u16> {
    let (h, m) = s.trim().split_once(':')?;
    let (h, m): (u16, u16) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

pub fn format_hhmm(minute: f64) -> String {
    let m = minute.rem_euclid(DAY).floor() as u32;
    format!("{:02}:{:02}", m / 60, m % 60)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SunTimes {
    /// Minutes after midnight UTC; `None` during polar day/night.
    pub sunrise_utc: Option<f64>,
    pub sunset_utc: Option<f64>,
}

/// Sunrise and sunset for a calendar date (NOAA solar calculator, accurate to ~1–2 minutes
/// outside the polar circles). `lon` is positive east. Results are in minutes after midnight
/// UTC and may fall outside 0..1440 for far-from-Greenwich longitudes; add the local UTC
/// offset and wrap.
pub fn sun_times(year: i32, month: u32, day: u32, lat: f64, lon: f64) -> SunTimes {
    let (y, m) = if month <= 2 { (year - 1, month + 12) } else { (year, month) };
    let a = (y as f64 / 100.0).floor();
    let b = 2.0 - a + (a / 4.0).floor();
    let jd0 = (365.25 * (y as f64 + 4716.0)).floor() + (30.6001 * (m as f64 + 1.0)).floor() + day as f64 + b - 1524.5;

    // Orbital parameters at local solar noon of that date.
    let t = (jd0 + 0.5 - lon / 360.0 - 2451545.0) / 36525.0;
    let rad = f64::to_radians;
    let l0 = (280.46646 + t * (36000.76983 + t * 0.0003032)).rem_euclid(360.0);
    let mean_anom = 357.52911 + t * (35999.05029 - 0.0001537 * t);
    let ecc = 0.016708634 - t * (0.000042037 + 0.0000001267 * t);
    let center = rad(mean_anom).sin() * (1.914602 - t * (0.004817 + 0.000014 * t))
        + rad(2.0 * mean_anom).sin() * (0.019993 - 0.000101 * t)
        + rad(3.0 * mean_anom).sin() * 0.000289;
    let true_long = l0 + center;
    let omega = 125.04 - 1934.136 * t;
    let app_long = true_long - 0.00569 - 0.00478 * rad(omega).sin();
    let mean_obliq = 23.0 + (26.0 + (21.448 - t * (46.815 + t * (0.00059 - t * 0.001813))) / 60.0) / 60.0;
    let obliq = mean_obliq + 0.00256 * rad(omega).cos();
    let decl = (rad(obliq).sin() * rad(app_long).sin()).asin();
    let yy = (rad(obliq) / 2.0).tan().powi(2);
    let eq_time = 4.0
        * (yy * rad(2.0 * l0).sin() - 2.0 * ecc * rad(mean_anom).sin()
            + 4.0 * ecc * yy * rad(mean_anom).sin() * rad(2.0 * l0).cos()
            - 0.5 * yy * yy * rad(4.0 * l0).sin()
            - 1.25 * ecc * ecc * rad(2.0 * mean_anom).sin())
        .to_degrees();

    let cos_ha = (rad(90.833).cos() / (rad(lat).cos() * decl.cos())) - rad(lat).tan() * decl.tan();
    if !(-1.0..=1.0).contains(&cos_ha) {
        return SunTimes { sunrise_utc: None, sunset_utc: None };
    }
    let ha = cos_ha.acos().to_degrees();
    let solar_noon = 720.0 - 4.0 * lon - eq_time;
    SunTimes { sunrise_utc: Some(solar_noon - 4.0 * ha), sunset_utc: Some(solar_noon + 4.0 * ha) }
}

pub type Key = (String, Feature);

/// A schedule point with its trigger time for today and the values it prescribes.
#[derive(Debug, Clone)]
pub struct ResolvedPoint {
    pub id: String,
    /// Minutes after local midnight, 0..1440.
    pub at: f64,
    pub values: HashMap<Key, f64>,
}

#[derive(Debug, Clone)]
pub struct Evaluation {
    /// Index into the (time-sorted) points that is currently in force.
    pub active: usize,
    /// Value to have right now for every key the active point controls.
    pub values: Vec<(Key, f64)>,
    /// Minutes until the next point triggers.
    pub next_trigger_in: f64,
    /// Minutes until the running transition finishes, if one is running.
    pub ramp_remaining: Option<f64>,
}

/// What the schedule prescribes at `now` (minutes after local midnight, fractions allowed).
///
/// The point in force is the latest one at or before `now` (wrapping to yesterday's last point).
/// For `transition_min` minutes after it triggers, values move linearly from the previous
/// point's values; a key the previous point did not control jumps straight to its value.
/// Sorts `points` by time.
pub fn evaluate(points: &mut [ResolvedPoint], now: f64, transition_min: f64) -> Option<Evaluation> {
    if points.is_empty() {
        return None;
    }
    points.sort_by(|a, b| a.at.total_cmp(&b.at));
    let n = points.len();
    let (active, elapsed) = match points.iter().rposition(|p| p.at <= now) {
        Some(i) => (i, now - points[i].at),
        None => (n - 1, now + DAY - points[n - 1].at),
    };
    let prev = &points[(active + n - 1) % n];
    let ramping = transition_min > 0.0 && elapsed < transition_min && n > 1;
    let f = if ramping { elapsed / transition_min } else { 1.0 };

    let values = points[active]
        .values
        .iter()
        .map(|(k, &v)| {
            let out = match prev.values.get(k) {
                Some(&p) if ramping => p + (v - p) * f,
                _ => v,
            };
            (k.clone(), out)
        })
        .collect();

    let next = &points[(active + 1) % n];
    let mut delta = next.at - now;
    if delta <= 0.0 {
        delta += DAY;
    }
    Some(Evaluation {
        active,
        values,
        next_trigger_in: delta,
        ramp_remaining: ramping.then_some(transition_min - elapsed),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(id: &str, at: f64, v: &[(&str, f64)]) -> ResolvedPoint {
        ResolvedPoint {
            id: id.into(),
            at,
            values: v.iter().map(|(m, x)| ((m.to_string(), Feature::Brightness), *x)).collect(),
        }
    }

    fn val(e: &Evaluation, m: &str) -> f64 {
        e.values.iter().find(|((id, _), _)| id == m).unwrap().1
    }

    #[test]
    fn hhmm_roundtrip() {
        assert_eq!(parse_hhmm("07:05"), Some(425));
        assert_eq!(parse_hhmm("24:00"), None);
        assert_eq!(parse_hhmm("7"), None);
        assert_eq!(format_hhmm(425.4), "07:05");
        assert_eq!(format_hhmm(-10.0), "23:50");
    }

    fn near(a: Option<f64>, expect_min: f64, tol: f64) {
        let a = a.expect("sun event expected");
        assert!((a - expect_min).abs() <= tol, "got {a}, expected {expect_min} ± {tol}");
    }

    #[test]
    fn moscow_summer_and_winter() {
        // Moscow, 2026-06-21: sunrise ≈ 03:44, sunset ≈ 21:18 local (UTC+3).
        let s = sun_times(2026, 6, 21, 55.7558, 37.6173);
        near(s.sunrise_utc, 44.0, 4.0); // 00:44 UTC
        near(s.sunset_utc, 18.0 * 60.0 + 18.0, 4.0);
        // 2026-12-21: sunrise ≈ 08:58, sunset ≈ 15:57 local -> 05:58 / 12:57 UTC
        let w = sun_times(2026, 12, 21, 55.7558, 37.6173);
        near(w.sunrise_utc, 5.0 * 60.0 + 58.0, 4.0);
        near(w.sunset_utc, 12.0 * 60.0 + 57.0, 4.0);
    }

    #[test]
    fn equinox_at_equator_is_about_twelve_hours() {
        let s = sun_times(2026, 3, 20, 0.0, 0.0);
        let len = s.sunset_utc.unwrap() - s.sunrise_utc.unwrap();
        assert!((len - 12.0 * 60.0).abs() < 12.0, "day length {len}");
    }

    #[test]
    fn polar_day_and_night() {
        assert_eq!(sun_times(2026, 6, 21, 80.0, 20.0).sunrise_utc, None);
        assert_eq!(sun_times(2026, 12, 21, 80.0, 20.0).sunset_utc, None);
    }

    #[test]
    fn no_points() {
        assert!(evaluate(&mut [], 100.0, 10.0).is_none());
    }

    #[test]
    fn step_without_transition() {
        let mut p = vec![pt("day", 7.0 * 60.0, &[("A", 80.0)]), pt("night", 22.0 * 60.0, &[("A", 25.0)])];
        let e = evaluate(&mut p, 12.0 * 60.0, 0.0).unwrap();
        assert_eq!(val(&e, "A"), 80.0);
        assert!((e.next_trigger_in - 10.0 * 60.0).abs() < 1e-9);
        assert_eq!(e.ramp_remaining, None);
        // after midnight the night point from "yesterday" is still in force
        let e = evaluate(&mut p, 2.0 * 60.0, 0.0).unwrap();
        assert_eq!(val(&e, "A"), 25.0);
        assert!((e.next_trigger_in - 5.0 * 60.0).abs() < 1e-9);
    }

    #[test]
    fn linear_transition_and_midpoint() {
        let mut p = vec![pt("day", 7.0 * 60.0, &[("A", 80.0)]), pt("night", 22.0 * 60.0, &[("A", 20.0)])];
        // 30 min ramp starting 22:00 from 80 -> 20
        let e = evaluate(&mut p, 22.0 * 60.0 + 15.0, 30.0).unwrap();
        assert!((val(&e, "A") - 50.0).abs() < 1e-9);
        assert!((e.ramp_remaining.unwrap() - 15.0).abs() < 1e-9);
        let e = evaluate(&mut p, 22.0 * 60.0 + 30.0, 30.0).unwrap();
        assert_eq!(val(&e, "A"), 20.0);
        assert_eq!(e.ramp_remaining, None);
        // ramp across midnight: the 22:00 point still ramps at 22:10, never at 23:00
        let e = evaluate(&mut p, 22.0 * 60.0 + 10.0, 30.0).unwrap();
        assert!(val(&e, "A") < 80.0 && val(&e, "A") > 20.0);
    }

    #[test]
    fn key_missing_in_previous_point_jumps() {
        let mut p = vec![pt("a", 100.0, &[("A", 10.0)]), pt("b", 200.0, &[("A", 90.0), ("B", 70.0)])];
        let e = evaluate(&mut p, 205.0, 20.0).unwrap();
        assert!(val(&e, "A") > 10.0 && val(&e, "A") < 90.0);
        assert_eq!(val(&e, "B"), 70.0, "no previous value for B: no ramp");
    }

    #[test]
    fn single_point_is_constant_even_with_transition() {
        let mut p = vec![pt("only", 600.0, &[("A", 55.0)])];
        for now in [0.0, 599.0, 600.5, 1000.0] {
            let e = evaluate(&mut p, now, 30.0).unwrap();
            assert_eq!(val(&e, "A"), 55.0);
        }
    }

    #[test]
    fn sorting_and_wrap_before_first_point() {
        let mut p = vec![pt("late", 1300.0, &[("A", 30.0)]), pt("early", 400.0, &[("A", 90.0)])];
        let e = evaluate(&mut p, 100.0, 0.0).unwrap();
        assert_eq!(p[e.active].id, "late");
        assert_eq!(val(&e, "A"), 30.0);
    }
}
