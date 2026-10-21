//! Local time and "is something full-screen" for the scheduler.

use std::time::{SystemTime, UNIX_EPOCH};

use windows::Win32::System::SystemInformation::{GetLocalTime, GetSystemTime};
use windows::Win32::UI::Shell::{
    SHQueryUserNotificationState, QUNS_BUSY, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN,
};

#[derive(Debug, Clone, Copy)]
pub struct Now {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    /// Minutes after local midnight, with seconds as the fraction.
    pub minute: f64,
    /// Local time minus UTC, in minutes, DST included.
    pub utc_offset_min: i32,
    pub epoch_secs: u64,
}

pub fn now() -> Now {
    let (l, u) = unsafe { (GetLocalTime(), GetSystemTime()) };
    let local_min = l.wHour as i32 * 60 + l.wMinute as i32;
    let utc_min = u.wHour as i32 * 60 + u.wMinute as i32;
    // Offsets are within ±14 h, so wrapping the difference into (-12 h, +12 h] is not enough;
    // wrap into (-14 h, +14 h] around the date change instead.
    let mut off = local_min - utc_min;
    if off > 14 * 60 {
        off -= 1440;
    } else if off < -14 * 60 {
        off += 1440;
    }
    Now {
        year: l.wYear as i32,
        month: l.wMonth as u32,
        day: l.wDay as u32,
        minute: local_min as f64 + l.wSecond as f64 / 60.0,
        utc_offset_min: off,
        epoch_secs: SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
    }
}

/// A full-screen app, game or presentation owns the screen (`SHQueryUserNotificationState`).
pub fn fullscreen_active() -> bool {
    matches!(
        unsafe { SHQueryUserNotificationState() },
        Ok(s) if s == QUNS_BUSY || s == QUNS_RUNNING_D3D_FULL_SCREEN || s == QUNS_PRESENTATION_MODE
    )
}
