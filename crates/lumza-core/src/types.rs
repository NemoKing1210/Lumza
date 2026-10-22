use std::fmt;
use std::str::FromStr;

use crate::error::Error;

/// Stable monitor identifier: EDID manufacturer + product + serial,
/// extended with the device path when the EDID has no serial.
pub type MonitorId = String;

/// VCP features Lumza controls (see spec §3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Feature {
    Brightness,
    Contrast,
    Volume,
}

impl Feature {
    pub const ALL: [Feature; 3] = [Feature::Brightness, Feature::Contrast, Feature::Volume];

    pub fn vcp(self) -> u8 {
        match self {
            Feature::Brightness => 0x10,
            Feature::Contrast => 0x12,
            Feature::Volume => 0x62,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Feature::Brightness => "brightness",
            Feature::Contrast => "contrast",
            Feature::Volume => "volume",
        }
    }
}

impl fmt::Display for Feature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for Feature {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Error> {
        match s.to_ascii_lowercase().as_str() {
            "brightness" | "b" => Ok(Feature::Brightness),
            "contrast" | "c" => Ok(Feature::Contrast),
            "volume" | "v" => Ok(Feature::Volume),
            other => Err(Error::Invalid(format!("unknown feature '{other}'"))),
        }
    }
}

/// Raw VCP reading: current and maximum value as the monitor reports them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VcpValue {
    pub current: u32,
    pub max: u32,
}

impl VcpValue {
    /// Value as 0–100 %, scaled against the monitor's own maximum.
    pub fn percent(self) -> u8 {
        crate::scale::raw_to_percent(self.current, self.max)
    }
}

/// HDR as Windows reports it for one display. Not a DDC/CI feature: it comes from
/// the DisplayConfig advanced-colour API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HdrState {
    pub supported: bool,
    pub enabled: bool,
}

/// How Windows spreads the desktop over the connected displays (the Win+P choices).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisplayMode {
    Duplicate,
    Extend,
    /// Only the display with this key; the others are switched off.
    Only(String),
}

/// One physically connected display, whether or not Windows currently uses it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayEntry {
    /// Opaque and stable while the display stays connected; the handle for [`DisplayMode::Only`].
    pub key: String,
    /// 1-based, Lumza's own numbering: it can differ from Windows "Display settings".
    pub number: u32,
    pub name: String,
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayModes {
    pub displays: Vec<DisplayEntry>,
    /// `None` when the current layout is none of the named modes (nothing is active).
    pub current: Option<DisplayMode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Connection {
    Ddc,
    /// Laptop panel controlled through WMI.
    Internal,
}

#[derive(Debug, Clone)]
pub struct MonitorInfo {
    pub id: MonitorId,
    /// 1-based number, matching Windows "Display settings" where possible.
    pub number: u32,
    pub name: String,
    pub connection: Connection,
    /// GDI device name (e.g. `\\.\DISPLAY1`) for diagnostics.
    pub device: String,
}
