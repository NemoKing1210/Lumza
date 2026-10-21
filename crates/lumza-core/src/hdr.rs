//! HDR on/off through the DisplayConfig advanced-colour API. HDR has no VCP code, so
//! this is plain Windows state and never touches the monitor's DDC/CI channel.

use windows::Win32::Devices::Display::{
    DisplayConfigGetDeviceInfo, DisplayConfigSetDeviceInfo, DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO,
    DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_HEADER,
    DISPLAYCONFIG_DEVICE_INFO_SET_ADVANCED_COLOR_STATE, DISPLAYCONFIG_DEVICE_INFO_TYPE,
    DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO, DISPLAYCONFIG_SET_ADVANCED_COLOR_STATE, DISPLAYCONFIG_SOURCE_DEVICE_NAME,
};
use windows::Win32::Foundation::LUID;

use crate::dxva2::{active_paths, wide_to_string};
use crate::error::{Error, Result};
use crate::types::HdrState;

// Windows 11 24H2 adds `_2` info types that tell real HDR from wide colour gamut only.
// The `windows` crate has no bindings for them; layouts are from wingdi.h (SDK 10.0.26100.0).
const GET_ADVANCED_COLOR_INFO_2: DISPLAYCONFIG_DEVICE_INFO_TYPE = DISPLAYCONFIG_DEVICE_INFO_TYPE(15);
const SET_HDR_STATE: DISPLAYCONFIG_DEVICE_INFO_TYPE = DISPLAYCONFIG_DEVICE_INFO_TYPE(16);

#[repr(C)]
struct AdvancedColorInfo2 {
    header: DISPLAYCONFIG_DEVICE_INFO_HEADER,
    value: u32,
    color_encoding: i32,
    bits_per_color_channel: u32,
    active_color_mode: i32,
}

#[repr(C)]
struct SetHdrState {
    header: DISPLAYCONFIG_DEVICE_INFO_HEADER,
    value: u32,
}

/// The display target (adapter + id) behind a GDI device name such as `\\.\DISPLAY1`.
#[derive(Clone, Copy)]
struct Target {
    adapter: LUID,
    id: u32,
}

fn target_for(device: &str) -> Option<Target> {
    let paths = active_paths()?;
    for p in &paths {
        let mut src = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
            header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
                size: size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
                adapterId: p.sourceInfo.adapterId,
                id: p.sourceInfo.id,
            },
            ..Default::default()
        };
        if unsafe { DisplayConfigGetDeviceInfo(&mut src.header) } != 0 {
            continue;
        }
        if wide_to_string(&src.viewGdiDeviceName) == device {
            return Some(Target { adapter: p.targetInfo.adapterId, id: p.targetInfo.id });
        }
    }
    None
}

fn header(kind: DISPLAYCONFIG_DEVICE_INFO_TYPE, size: usize, t: Target) -> DISPLAYCONFIG_DEVICE_INFO_HEADER {
    DISPLAYCONFIG_DEVICE_INFO_HEADER { r#type: kind, size: size as u32, adapterId: t.adapter, id: t.id }
}

/// The `_2` flag word, or `None` on Windows builds that do not know the type.
fn read_v2(t: Target) -> Option<u32> {
    let mut info = AdvancedColorInfo2 {
        header: header(GET_ADVANCED_COLOR_INFO_2, size_of::<AdvancedColorInfo2>(), t),
        value: 0,
        color_encoding: 0,
        bits_per_color_channel: 0,
        active_color_mode: 0,
    };
    let ok = unsafe { DisplayConfigGetDeviceInfo(&mut info as *mut _ as *mut DISPLAYCONFIG_DEVICE_INFO_HEADER) } == 0;
    ok.then_some(info.value)
}

fn decode_v2(value: u32) -> HdrState {
    let (hdr_supported, limited_by_policy) = (value & (1 << 4) != 0, value & (1 << 3) != 0);
    HdrState { supported: hdr_supported && !limited_by_policy, enabled: value & (1 << 5) != 0 }
}

/// v1 cannot tell HDR from wide colour in general; `wideColorEnforced` at least marks a
/// display that is currently in wide colour (Auto Color Management on SDR), not HDR.
fn decode_v1(value: u32) -> HdrState {
    let (supported, wide_colour, force_disabled) = (value & 1 != 0, value & (1 << 2) != 0, value & (1 << 3) != 0);
    HdrState { supported: supported && !wide_colour && !force_disabled, enabled: value & (1 << 1) != 0 }
}

/// HDR state of the display with this GDI device name; `None` when it cannot be read.
pub fn query(device: &str) -> Option<HdrState> {
    let t = target_for(device)?;
    if let Some(v) = read_v2(t) {
        return Some(decode_v2(v));
    }
    let mut info = DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO {
        header: header(
            DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO,
            size_of::<DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO>(),
            t,
        ),
        ..Default::default()
    };
    unsafe {
        if DisplayConfigGetDeviceInfo(&mut info as *mut _ as *mut DISPLAYCONFIG_DEVICE_INFO_HEADER) != 0 {
            return None;
        }
        Some(decode_v1(info.Anonymous.value))
    }
}

pub fn set(device: &str, on: bool) -> Result<()> {
    let t = target_for(device).ok_or_else(|| Error::NotFound(device.into()))?;
    let code = unsafe {
        if read_v2(t).is_some() {
            let s = SetHdrState { header: header(SET_HDR_STATE, size_of::<SetHdrState>(), t), value: on as u32 };
            DisplayConfigSetDeviceInfo(&s as *const _ as *const DISPLAYCONFIG_DEVICE_INFO_HEADER)
        } else {
            let mut s = DISPLAYCONFIG_SET_ADVANCED_COLOR_STATE {
                header: header(
                    DISPLAYCONFIG_DEVICE_INFO_SET_ADVANCED_COLOR_STATE,
                    size_of::<DISPLAYCONFIG_SET_ADVANCED_COLOR_STATE>(),
                    t,
                ),
                ..Default::default()
            };
            s.Anonymous.value = on as u32;
            DisplayConfigSetDeviceInfo(&s as *const _ as *const DISPLAYCONFIG_DEVICE_INFO_HEADER)
        }
    };
    if code == 0 {
        Ok(())
    } else {
        Err(Error::Os(format!("setting HDR failed ({code})")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const V2_SUPPORTED: u32 = 1 << 4;
    const V2_ENABLED: u32 = 1 << 5;
    const V2_LIMITED_BY_POLICY: u32 = 1 << 3;
    const V2_ADVANCED_COLOR: u32 = 1;
    const V2_WIDE_COLOR: u32 = (1 << 6) | (1 << 7);

    #[test]
    fn v2_hdr_supported_and_enabled() {
        assert_eq!(decode_v2(V2_SUPPORTED | V2_ENABLED), HdrState { supported: true, enabled: true });
    }

    #[test]
    fn v2_hdr_supported_off() {
        assert_eq!(decode_v2(V2_SUPPORTED), HdrState { supported: true, enabled: false });
    }

    #[test]
    fn v2_wide_colour_only_is_not_hdr() {
        assert!(!decode_v2(V2_ADVANCED_COLOR | V2_WIDE_COLOR).supported);
    }

    #[test]
    fn v1_wide_colour_enforced_is_not_hdr() {
        // Pre-24H2 Auto Color Management on an SDR panel: advanced colour is on, but as wide colour.
        assert!(!decode_v1(0b111).supported);
    }

    #[test]
    fn v2_limited_by_policy_is_unsupported() {
        assert!(!decode_v2(V2_LIMITED_BY_POLICY | V2_SUPPORTED).supported);
    }

    #[test]
    fn v1_supported_enabled() {
        assert_eq!(decode_v1(0b11), HdrState { supported: true, enabled: true });
    }

    #[test]
    fn v1_force_disabled_is_unsupported() {
        assert!(!decode_v1(0b1001).supported);
    }
}
