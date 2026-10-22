//! Display layout (duplicate / extend / one display only) through the CCD API. Windows-only, and
//! not DDC/CI: it changes which displays the desktop uses, never what a monitor is told.

use windows::Win32::Devices::Display::{
    DisplayConfigGetDeviceInfo, SetDisplayConfig, DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
    DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME, DISPLAYCONFIG_DEVICE_INFO_HEADER, DISPLAYCONFIG_PATH_INFO,
    DISPLAYCONFIG_SOURCE_DEVICE_NAME, DISPLAYCONFIG_TARGET_DEVICE_NAME, QDC_ALL_PATHS, SDC_ALLOW_PATH_ORDER_CHANGES,
    SDC_APPLY, SDC_TOPOLOGY_CLONE, SDC_TOPOLOGY_EXTEND, SDC_TOPOLOGY_SUPPLIED, SET_DISPLAY_CONFIG_FLAGS,
};
use windows::Win32::Foundation::LUID;

use crate::dxva2::{gdi_number, query_paths, wide_to_string};
use crate::error::{Error, Result};
use crate::topology::{assign_numbers, classify, validate};
use crate::types::{DisplayEntry, DisplayMode, DisplayModes};

/// `DISPLAYCONFIG_PATH_ACTIVE` and `DISPLAYCONFIG_PATH_MODE_IDX_INVALID` from wingdi.h; the
/// `windows` crate has no constants for them.
const PATH_ACTIVE: u32 = 0x1;
const MODE_IDX_INVALID: u32 = 0xFFFF_FFFF;

/// Hand Windows a topology of just the chosen path and let it pick the modes from its database.
const ONLY_FLAGS: SET_DISPLAY_CONFIG_FLAGS =
    SET_DISPLAY_CONFIG_FLAGS(SDC_APPLY.0 | SDC_TOPOLOGY_SUPPLIED.0 | SDC_ALLOW_PATH_ORDER_CHANGES.0);

fn key(adapter: LUID, id: u32) -> String {
    format!("{:x}-{:x}-{:x}", adapter.HighPart, adapter.LowPart, id)
}

fn target_key(p: &DISPLAYCONFIG_PATH_INFO) -> String {
    key(p.targetInfo.adapterId, p.targetInfo.id)
}

fn source_key(p: &DISPLAYCONFIG_PATH_INFO) -> String {
    key(p.sourceInfo.adapterId, p.sourceInfo.id)
}

fn is_active(p: &DISPLAYCONFIG_PATH_INFO) -> bool {
    p.flags & PATH_ACTIVE != 0
}

/// Every path to a physically connected display, active or not.
fn all_paths() -> Result<Vec<DISPLAYCONFIG_PATH_INFO>> {
    let paths = query_paths(QDC_ALL_PATHS).ok_or_else(|| Error::Os("QueryDisplayConfig failed".into()))?;
    Ok(paths.into_iter().filter(|p| p.targetInfo.targetAvailable.as_bool()).collect())
}

fn target_name(p: &DISPLAYCONFIG_PATH_INFO) -> String {
    let mut tgt = DISPLAYCONFIG_TARGET_DEVICE_NAME {
        header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
            r#type: DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
            size: size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32,
            adapterId: p.targetInfo.adapterId,
            id: p.targetInfo.id,
        },
        ..Default::default()
    };
    if unsafe { DisplayConfigGetDeviceInfo(&mut tgt.header) } != 0 {
        return String::new();
    }
    wide_to_string(&tgt.monitorFriendlyDeviceName)
}

/// The number Windows shows for an in-use display: its GDI name `\\.\DISPLAY2` -> 2.
fn windows_number(p: &DISPLAYCONFIG_PATH_INFO) -> Option<u32> {
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
        return None;
    }
    Some(gdi_number(&wide_to_string(&src.viewGdiDeviceName))).filter(|n| *n > 0)
}

fn modes_of(paths: &[DISPLAYCONFIG_PATH_INFO]) -> DisplayModes {
    let mut targets: Vec<&DISPLAYCONFIG_PATH_INFO> = Vec::new();
    for p in paths {
        if !targets.iter().any(|t| target_key(t) == target_key(p)) {
            targets.push(p);
        }
    }
    targets.sort_by_key(|p| (p.targetInfo.adapterId.HighPart, p.targetInfo.adapterId.LowPart, p.targetInfo.id));
    let in_use: Vec<Option<&DISPLAYCONFIG_PATH_INFO>> =
        targets.iter().map(|t| paths.iter().find(|q| is_active(q) && target_key(q) == target_key(t))).collect();
    let numbers = assign_numbers(&in_use.iter().map(|q| q.and_then(windows_number)).collect::<Vec<_>>());
    let mut displays: Vec<DisplayEntry> = targets
        .iter()
        .zip(&in_use)
        .zip(numbers)
        .map(|((p, in_use), number)| {
            let name = Some(target_name(p)).filter(|n| !n.is_empty()).unwrap_or_else(|| format!("Display {number}"));
            DisplayEntry { key: target_key(p), number, name, active: in_use.is_some() }
        })
        .collect();
    displays.sort_by_key(|d| d.number);
    let active: Vec<(String, String)> =
        paths.iter().filter(|p| is_active(p)).map(|p| (target_key(p), source_key(p))).collect();
    DisplayModes { displays, current: classify(&active) }
}

/// The path that shows only `key`: the one already in use if there is one, else any available path.
fn only_path(paths: &[DISPLAYCONFIG_PATH_INFO], key: &str) -> Result<DISPLAYCONFIG_PATH_INFO> {
    let found = paths.iter().filter(|p| target_key(p) == key).max_by_key(|p| is_active(p));
    let mut path = *found.ok_or_else(|| Error::NotFound(key.to_string()))?;
    path.flags |= PATH_ACTIVE;
    path.sourceInfo.Anonymous.modeInfoIdx = MODE_IDX_INVALID;
    path.targetInfo.Anonymous.modeInfoIdx = MODE_IDX_INVALID;
    Ok(path)
}

/// Connected displays and the current layout.
pub fn query() -> Result<DisplayModes> {
    Ok(modes_of(&all_paths()?))
}

fn set(paths: Option<&[DISPLAYCONFIG_PATH_INFO]>, flags: SET_DISPLAY_CONFIG_FLAGS) -> Result<()> {
    match unsafe { SetDisplayConfig(paths, None, flags) } {
        0 => Ok(()),
        code => Err(Error::Os(format!("setting display mode failed ({code})"))),
    }
}

pub fn apply(mode: &DisplayMode) -> Result<()> {
    let paths = all_paths()?;
    validate(mode, &modes_of(&paths).displays)?;
    match mode {
        DisplayMode::Duplicate => set(None, SET_DISPLAY_CONFIG_FLAGS(SDC_APPLY.0 | SDC_TOPOLOGY_CLONE.0)),
        DisplayMode::Extend => set(None, SET_DISPLAY_CONFIG_FLAGS(SDC_APPLY.0 | SDC_TOPOLOGY_EXTEND.0)),
        DisplayMode::Only(key) => set(Some(&[only_path(&paths, key)?]), ONLY_FLAGS),
    }
}

#[cfg(test)]
mod tests {
    use windows::Win32::Devices::Display::SDC_VALIDATE;

    use super::*;

    #[test]
    #[ignore = "needs a real display session"]
    fn query_reads_the_connected_displays() {
        let m = query().expect("QueryDisplayConfig");
        assert!(!m.displays.is_empty());
        assert!(m.displays.iter().any(|d| d.active));
        assert!(m.displays.iter().all(|d| !d.name.is_empty() && d.number >= 1));
    }

    /// `SDC_VALIDATE` asks Windows whether a topology is acceptable without applying it, so this
    /// exercises the exact "only on N" request for every connected display and changes nothing.
    #[test]
    #[ignore = "needs a real display session"]
    fn windows_accepts_the_only_on_request_for_every_display() {
        let paths = all_paths().expect("QueryDisplayConfig");
        for d in query().unwrap().displays {
            let path = only_path(&paths, &d.key).expect("path for a connected display");
            let flags = SET_DISPLAY_CONFIG_FLAGS((ONLY_FLAGS.0 & !SDC_APPLY.0) | SDC_VALIDATE.0);
            let code = unsafe { SetDisplayConfig(Some(&[path]), None, flags) };
            assert_eq!(code, 0, "validating only-on-{} ({})", d.number, d.name);
        }
    }
}
