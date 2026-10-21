//! DDC/CI over Dxva2 (Windows high-level monitor configuration API).

use std::collections::HashMap;
use std::sync::Arc;
use std::thread::sleep;
use std::time::Duration;

use windows::core::BOOL;
use windows::Win32::Devices::Display::{
    CapabilitiesRequestAndCapabilitiesReply, DestroyPhysicalMonitors, DisplayConfigGetDeviceInfo,
    GetCapabilitiesStringLength, GetDisplayConfigBufferSizes, GetNumberOfPhysicalMonitorsFromHMONITOR,
    GetPhysicalMonitorsFromHMONITOR, GetVCPFeatureAndVCPFeatureReply, QueryDisplayConfig, SetVCPFeature,
    DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
    DISPLAYCONFIG_DEVICE_INFO_HEADER, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO,
    DISPLAYCONFIG_SOURCE_DEVICE_NAME, DISPLAYCONFIG_TARGET_DEVICE_NAME, PHYSICAL_MONITOR, QDC_ONLY_ACTIVE_PATHS,
};
use windows::Win32::Foundation::{HANDLE, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFOEXW};

use crate::backend::{DisplayBackend, MonitorPort, PortSource};
use crate::error::{Error, Result};
use crate::types::{Connection, HdrState, MonitorId, MonitorInfo, VcpValue};

/// DDC/CI timing (spec §6): ≥50 ms after a write, ≥40 ms after a read.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    pub write_delay: Duration,
    pub read_delay: Duration,
    pub retries: u32,
}

impl Default for Timing {
    fn default() -> Self {
        Self { write_delay: Duration::from_millis(50), read_delay: Duration::from_millis(40), retries: 2 }
    }
}

/// Owns one physical-monitor handle; destroyed when the last `Arc` goes away,
/// so a worker mid-write never sees its handle invalidated by a rescan.
struct Guard(HANDLE);
// A physical-monitor handle may be used from any single thread at a time.
unsafe impl Send for Guard {}
unsafe impl Sync for Guard {}

impl Drop for Guard {
    fn drop(&mut self) {
        let pm = [PHYSICAL_MONITOR { hPhysicalMonitor: self.0, ..Default::default() }];
        unsafe {
            let _ = DestroyPhysicalMonitors(&pm);
        }
    }
}

/// One monitor's DDC/CI channel. Cheap to move into a worker thread.
pub struct Dxva2Port {
    guard: Arc<Guard>,
    timing: Timing,
}

impl MonitorPort for Dxva2Port {
    fn capabilities_string(&mut self) -> Result<String> {
        let (h, t) = (self.guard.0, self.timing);
        retry(t.retries, || unsafe {
            let mut len = 0u32;
            if GetCapabilitiesStringLength(h, &mut len) == 0 || len == 0 {
                return Err(Error::Timeout("capabilities length".into()));
            }
            let mut buf = vec![0u8; len as usize];
            if CapabilitiesRequestAndCapabilitiesReply(h, &mut buf) == 0 {
                return Err(Error::Timeout("capabilities".into()));
            }
            let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
            Ok(String::from_utf8_lossy(&buf[..end]).into_owned())
        })
    }

    fn get_vcp(&mut self, code: u8) -> Result<VcpValue> {
        let (h, t) = (self.guard.0, self.timing);
        retry(t.retries, || unsafe {
            let (mut cur, mut max) = (0u32, 0u32);
            let ok = GetVCPFeatureAndVCPFeatureReply(h, code, None, &mut cur, Some(&mut max));
            sleep(t.read_delay);
            if ok == 0 {
                Err(Error::Unsupported(code))
            } else {
                Ok(VcpValue { current: cur, max })
            }
        })
    }

    fn set_vcp(&mut self, code: u8, value: u32) -> Result<()> {
        let (h, t) = (self.guard.0, self.timing);
        retry(t.retries, || unsafe {
            let ok = SetVCPFeature(h, code, value);
            sleep(t.write_delay);
            if ok == 0 {
                Err(Error::Timeout(format!("set 0x{code:02X}")))
            } else {
                Ok(())
            }
        })
    }
}

struct Entry {
    info: MonitorInfo,
    port: Dxva2Port,
}

pub struct Dxva2Backend {
    entries: Vec<Entry>,
    pub timing: Timing,
}

impl Default for Dxva2Backend {
    fn default() -> Self {
        Self::new()
    }
}

impl Dxva2Backend {
    pub fn new() -> Self {
        Self { entries: Vec::new(), timing: Timing::default() }
    }

    fn port(&mut self, id: &MonitorId) -> Result<&mut Dxva2Port> {
        self.entries
            .iter_mut()
            .find(|e| &e.info.id == id)
            .map(|e| &mut e.port)
            .ok_or_else(|| Error::NotFound(id.clone()))
    }
}

/// Target (monitor) data from DisplayConfig, keyed by GDI source name (`\\.\DISPLAY1`).
#[derive(Default, Clone)]
struct Target {
    friendly: String,
    device_path: String,
}

pub(crate) fn wide_to_string(w: &[u16]) -> String {
    let end = w.iter().position(|&c| c == 0).unwrap_or(w.len());
    String::from_utf16_lossy(&w[..end])
}

/// Active display paths (one per attached monitor), or `None` when Windows refuses the query.
pub(crate) fn active_paths() -> Option<Vec<DISPLAYCONFIG_PATH_INFO>> {
    unsafe {
        let (mut np, mut nm) = (0u32, 0u32);
        if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut np, &mut nm).0 != 0 {
            return None;
        }
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); np as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); nm as usize];
        if QueryDisplayConfig(QDC_ONLY_ACTIVE_PATHS, &mut np, paths.as_mut_ptr(), &mut nm, modes.as_mut_ptr(), None).0
            != 0
        {
            return None;
        }
        paths.truncate(np as usize);
        Some(paths)
    }
}

fn display_config_targets() -> HashMap<String, Target> {
    let mut map = HashMap::new();
    let Some(paths) = active_paths() else { return map };
    unsafe {
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
            if DisplayConfigGetDeviceInfo(&mut src.header) != 0 {
                continue;
            }
            let mut tgt = DISPLAYCONFIG_TARGET_DEVICE_NAME {
                header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
                    r#type: DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
                    size: size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32,
                    adapterId: p.targetInfo.adapterId,
                    id: p.targetInfo.id,
                },
                ..Default::default()
            };
            if DisplayConfigGetDeviceInfo(&mut tgt.header) != 0 {
                continue;
            }
            map.entry(wide_to_string(&src.viewGdiDeviceName)).or_insert(Target {
                friendly: wide_to_string(&tgt.monitorFriendlyDeviceName),
                device_path: wide_to_string(&tgt.monitorDevicePath),
            });
        }
    }
    map
}

/// Pieces of identity pulled out of an EDID block.
#[derive(Default, Debug)]
struct Edid {
    serial: Option<String>,
    name: Option<String>,
}

fn parse_edid(b: &[u8]) -> Edid {
    let mut e = Edid::default();
    if b.len() < 128 {
        return e;
    }
    let numeric = u32::from_le_bytes([b[12], b[13], b[14], b[15]]);
    if numeric != 0 {
        e.serial = Some(format!("{numeric:08X}"));
    }
    for off in [54usize, 72, 90, 108] {
        let d = &b[off..off + 18];
        if d[0] == 0 && d[1] == 0 && d[2] == 0 {
            let text = || {
                let s: String = d[5..18].iter().take_while(|&&c| c != 0x0A && c != 0).map(|&c| c as char).collect();
                s.trim().to_string()
            };
            match d[3] {
                0xFC => e.name = Some(text()),
                0xFF => e.serial = Some(text()), // descriptor text beats the numeric field
                _ => {}
            }
        }
    }
    e
}

/// `\\?\DISPLAY#GSM5B09#5&2b1a3c&0&UID4352#{guid}` -> (`GSM5B09`, `5&2b1a3c&0&UID4352`)
fn split_device_path(path: &str) -> Option<(String, String)> {
    let mut it = path.split('#');
    it.next()?;
    Some((it.next()?.to_string(), it.next()?.to_string()))
}

fn read_edid(pnp: &str, instance: &str) -> Option<Vec<u8>> {
    use winreg::{enums::HKEY_LOCAL_MACHINE, RegKey};
    let key = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey(format!(r"SYSTEM\CurrentControlSet\Enum\DISPLAY\{pnp}\{instance}\Device Parameters"))
        .ok()?;
    key.get_raw_value("EDID").ok().map(|v| v.bytes)
}

struct Hmon {
    hmonitor: HMONITOR,
    device: String,
}

unsafe extern "system" fn enum_proc(h: HMONITOR, _: HDC, _: *mut RECT, lp: LPARAM) -> BOOL {
    let out = &mut *(lp.0 as *mut Vec<Hmon>);
    let mut mi = MONITORINFOEXW::default();
    mi.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
    if GetMonitorInfoW(h, &mut mi as *mut _ as *mut _).as_bool() {
        out.push(Hmon { hmonitor: h, device: wide_to_string(&mi.szDevice) });
    }
    BOOL(1)
}

fn gdi_number(device: &str) -> u32 {
    device.trim_start_matches(r"\\.\DISPLAY").parse().unwrap_or(0)
}

fn scan_raw(timing: Timing) -> Result<Vec<(MonitorInfo, Dxva2Port)>> {
    let mut found: Vec<(MonitorInfo, Dxva2Port)> = Vec::new();
    let mut hmons: Vec<Hmon> = Vec::new();
    unsafe {
        let ok = EnumDisplayMonitors(None, None, Some(enum_proc), LPARAM(&mut hmons as *mut _ as isize));
        if !ok.as_bool() {
            return Err(Error::Os("EnumDisplayMonitors failed".into()));
        }
    }
    let targets = display_config_targets();
    let mut used_ids: HashMap<String, u32> = HashMap::new();

    for hm in hmons {
        let mut count = 0u32;
        if unsafe { GetNumberOfPhysicalMonitorsFromHMONITOR(hm.hmonitor, &mut count) }.is_err() || count == 0 {
            tracing::debug!(device = %hm.device, "no physical monitors (internal panel or no DDC)");
            continue;
        }
        let mut pms = vec![PHYSICAL_MONITOR::default(); count as usize];
        if unsafe { GetPhysicalMonitorsFromHMONITOR(hm.hmonitor, &mut pms) }.is_err() {
            continue;
        }
        let target = targets.get(&hm.device).cloned().unwrap_or_default();
        let (pnp, instance) = split_device_path(&target.device_path).unwrap_or_default();
        let edid = read_edid(&pnp, &instance).map(|b| parse_edid(&b)).unwrap_or_default();

        for (i, pm) in pms.iter().enumerate() {
            let mut name = target.friendly.clone();
            if name.is_empty() {
                name = edid.name.clone().unwrap_or_default();
            }
            if name.is_empty() {
                let desc = pm.szPhysicalMonitorDescription;
                name = wide_to_string(&desc);
            }
            if name.is_empty() {
                name = format!("Display {}", gdi_number(&hm.device));
            }
            // Stable ID (F-03): model code + EDID serial. Without a serial the
            // device-path instance (port) keeps two identical monitors apart.
            let base = match (&edid.serial, pnp.is_empty()) {
                (Some(s), false) => format!("{pnp}-{s}"),
                (None, false) => format!("{pnp}-{instance}"),
                _ => format!("{}-{}", hm.device, i),
            };
            let n = used_ids.entry(base.clone()).or_insert(0);
            *n += 1;
            let id = if *n > 1 { format!("{base}#{n}") } else { base };

            found.push((
                MonitorInfo {
                    id,
                    number: gdi_number(&hm.device),
                    name,
                    connection: Connection::Ddc,
                    device: hm.device.clone(),
                },
                Dxva2Port { guard: Arc::new(Guard(pm.hPhysicalMonitor)), timing },
            ));
        }
    }
    found.sort_by_key(|(i, _)| i.number);
    Ok(found)
}

impl DisplayBackend for Dxva2Backend {
    fn list(&mut self) -> Result<Vec<MonitorInfo>> {
        self.entries = scan_raw(self.timing)?.into_iter().map(|(info, port)| Entry { info, port }).collect();
        Ok(self.entries.iter().map(|e| e.info.clone()).collect())
    }

    fn capabilities_string(&mut self, id: &MonitorId) -> Result<String> {
        self.port(id)?.capabilities_string()
    }

    fn get_vcp(&mut self, id: &MonitorId, code: u8) -> Result<VcpValue> {
        self.port(id)?.get_vcp(code)
    }

    fn set_vcp(&mut self, id: &MonitorId, code: u8, value: u32) -> Result<()> {
        self.port(id)?.set_vcp(code, value)
    }
}

impl PortSource for Dxva2Backend {
    fn set_timing(&mut self, write_delay_ms: u32, read_delay_ms: u32, retries: u32) {
        self.timing = Timing {
            write_delay: Duration::from_millis(write_delay_ms as u64),
            read_delay: Duration::from_millis(read_delay_ms as u64),
            retries,
        };
    }

    fn scan(&mut self) -> Result<Vec<(MonitorInfo, Box<dyn MonitorPort>)>> {
        Ok(scan_raw(self.timing)?.into_iter().map(|(i, p)| (i, Box::new(p) as Box<dyn MonitorPort>)).collect())
    }

    fn hdr_state(&mut self, device: &str) -> Option<HdrState> {
        crate::hdr::query(device)
    }

    fn set_hdr(&mut self, device: &str, on: bool) -> Result<()> {
        crate::hdr::set(device, on)
    }
}

fn retry<T>(retries: u32, mut f: impl FnMut() -> Result<T>) -> Result<T> {
    let mut last = None;
    for _ in 0..=retries {
        match f() {
            Ok(v) => return Ok(v),
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_path_split() {
        let p = r"\\?\DISPLAY#GSM5B09#5&2b1a3c&0&UID4352#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}";
        assert_eq!(split_device_path(p), Some(("GSM5B09".into(), "5&2b1a3c&0&UID4352".into())));
        assert_eq!(split_device_path(""), None);
    }

    #[test]
    fn edid_name_and_serial() {
        let mut b = vec![0u8; 128];
        b[12..16].copy_from_slice(&[0x78, 0x56, 0x34, 0x12]);
        b[54..59].copy_from_slice(&[0, 0, 0, 0xFC, 0]);
        b[59..59 + 5].copy_from_slice(b"LG UL");
        b[64] = 0x0A;
        let e = parse_edid(&b);
        assert_eq!(e.serial.as_deref(), Some("12345678"));
        assert_eq!(e.name.as_deref(), Some("LG UL"));
        // descriptor serial overrides numeric
        b[72..77].copy_from_slice(&[0, 0, 0, 0xFF, 0]);
        b[77..82].copy_from_slice(b"ABC12");
        b[82] = 0x0A;
        assert_eq!(parse_edid(&b).serial.as_deref(), Some("ABC12"));
    }
}
