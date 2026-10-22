//! In-memory backend with configurable latency/failures, for tests and UI work.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::thread::sleep;
use std::time::Duration;

use crate::backend::{DisplayBackend, MonitorPort, PortSource};
use crate::error::{Error, Result};
use crate::topology;
use crate::types::{Connection, DisplayEntry, DisplayMode, DisplayModes, HdrState, MonitorId, MonitorInfo, VcpValue};

#[derive(Debug, Clone)]
pub struct MockMonitor {
    pub info: MonitorInfo,
    pub caps: String,
    pub max: u32,
    pub values: HashMap<u8, u32>,
    /// Codes advertised in caps but failing on read (misbehaving firmware).
    pub broken: Vec<u8>,
}

pub struct MockBackend {
    pub monitors: Vec<MockMonitor>,
    pub latency: Duration,
    /// Log of every successful write: (monitor id, code, value).
    pub writes: Vec<(MonitorId, u8, u32)>,
    /// HDR state by GDI device name; displays without an entry have no HDR.
    pub hdr: HashMap<String, HdrState>,
    /// Current display layout; a monitor is attached unless `Only` names another one.
    pub mode: DisplayMode,
}

impl MockBackend {
    pub fn new(monitors: Vec<MockMonitor>) -> Self {
        Self { monitors, latency: Duration::ZERO, writes: Vec::new(), hdr: HashMap::new(), mode: DisplayMode::Extend }
    }

    fn is_active(&self, m: &MockMonitor) -> bool {
        !matches!(&self.mode, DisplayMode::Only(key) if *key != m.info.id)
    }

    fn display_entries(&self) -> Vec<DisplayEntry> {
        let entry = |m: &MockMonitor| DisplayEntry {
            key: m.info.id.clone(),
            number: m.info.number,
            name: m.info.name.clone(),
            active: self.is_active(m),
        };
        self.monitors.iter().map(entry).collect()
    }

    /// Two monitors resembling the reference hardware from the spec.
    pub fn demo() -> Self {
        let mk = |n: u32, id: &str, name: &str, caps: &str| MockMonitor {
            info: MonitorInfo {
                id: id.into(),
                number: n,
                name: name.into(),
                connection: Connection::Ddc,
                device: format!(r"\\.\DISPLAY{n}"),
            },
            caps: caps.into(),
            max: 100,
            values: HashMap::from([(0x10, 50), (0x12, 50), (0x62, 25)]),
            broken: vec![],
        };
        let mut b = Self::new(vec![
            mk(1, "GSM5B09-1234", "LG ULTRAGEAR", "(prot(monitor)type(lcd)model(LG)vcp(10 12 62)mccs_ver(2.2))"),
            mk(2, "AOC2702-5678", "AOC 27G2G3", "(prot(monitor)type(lcd)model(27G2G3)vcp(10 12)mccs_ver(2.2))"),
        ]);
        b.hdr.insert(r"\\.\DISPLAY1".into(), HdrState { supported: true, enabled: false });
        b
    }

    fn find(&mut self, id: &MonitorId) -> Result<&mut MockMonitor> {
        self.monitors.iter_mut().find(|m| &m.info.id == id).ok_or_else(|| Error::NotFound(id.clone()))
    }
}

impl DisplayBackend for MockBackend {
    fn list(&mut self) -> Result<Vec<MonitorInfo>> {
        sleep(self.latency);
        Ok(self.monitors.iter().map(|m| m.info.clone()).collect())
    }

    fn capabilities_string(&mut self, id: &MonitorId) -> Result<String> {
        let lat = self.latency;
        let caps = self.find(id)?.caps.clone();
        sleep(lat);
        Ok(caps)
    }

    fn get_vcp(&mut self, id: &MonitorId, code: u8) -> Result<VcpValue> {
        let lat = self.latency;
        let m = self.find(id)?;
        sleep(lat);
        if m.broken.contains(&code) {
            return Err(Error::Timeout(format!("0x{code:02X}")));
        }
        match m.values.get(&code) {
            Some(&current) => Ok(VcpValue { current, max: m.max }),
            None => Err(Error::Unsupported(code)),
        }
    }

    fn set_vcp(&mut self, id: &MonitorId, code: u8, value: u32) -> Result<()> {
        let lat = self.latency;
        let m = self.find(id)?;
        sleep(lat);
        if !m.values.contains_key(&code) {
            return Err(Error::Unsupported(code));
        }
        let v = value.min(m.max);
        m.values.insert(code, v);
        let mid = m.info.id.clone();
        self.writes.push((mid, code, v));
        Ok(())
    }
}

/// Shared mock usable as a [`PortSource`]; ports hit the same state, so tests can
/// add/remove monitors (hot-plug) between scans.
#[derive(Clone)]
pub struct SharedMock(pub Arc<Mutex<MockBackend>>);

struct MockPort {
    id: MonitorId,
    shared: Arc<Mutex<MockBackend>>,
}

impl MonitorPort for MockPort {
    fn capabilities_string(&mut self) -> Result<String> {
        self.shared.lock().unwrap().capabilities_string(&self.id)
    }
    fn get_vcp(&mut self, code: u8) -> Result<VcpValue> {
        let lat = self.shared.lock().unwrap().latency;
        sleep(lat);
        self.shared.lock().unwrap().get_vcp(&self.id, code)
    }
    fn set_vcp(&mut self, code: u8, value: u32) -> Result<()> {
        let lat = self.shared.lock().unwrap().latency;
        sleep(lat);
        self.shared.lock().unwrap().set_vcp(&self.id, code, value)
    }
}

impl PortSource for SharedMock {
    fn scan(&mut self) -> Result<Vec<(MonitorInfo, Box<dyn MonitorPort>)>> {
        let infos = {
            let mut mock = self.0.lock().unwrap();
            let mut infos = mock.list()?;
            let displays = mock.display_entries();
            infos.retain(|i| displays.iter().any(|d| d.key == i.id && d.active));
            infos
        };
        Ok(infos
            .into_iter()
            .map(|i| {
                let p = MockPort { id: i.id.clone(), shared: self.0.clone() };
                (i, Box::new(p) as Box<dyn MonitorPort>)
            })
            .collect())
    }

    fn hdr_state(&mut self, device: &str) -> Option<HdrState> {
        self.0.lock().unwrap().hdr.get(device).copied()
    }

    fn set_hdr(&mut self, device: &str, on: bool) -> Result<()> {
        match self.0.lock().unwrap().hdr.get_mut(device) {
            Some(s) if s.supported => {
                s.enabled = on;
                Ok(())
            }
            _ => Err(Error::Invalid(format!("{device} has no HDR"))),
        }
    }

    fn display_modes(&mut self) -> Option<DisplayModes> {
        let mock = self.0.lock().unwrap();
        Some(DisplayModes { displays: mock.display_entries(), current: Some(mock.mode.clone()) })
    }

    fn set_display_mode(&mut self, mode: &DisplayMode) -> Result<()> {
        let mut mock = self.0.lock().unwrap();
        topology::validate(mode, &mock.display_entries())?;
        mock.mode = mode.clone();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::verified_capabilities;

    #[test]
    fn set_get_roundtrip() {
        let mut b = MockBackend::demo();
        let id = b.list().unwrap()[0].id.clone();
        b.set_vcp(&id, 0x10, 77).unwrap();
        assert_eq!(b.get_vcp(&id, 0x10).unwrap().current, 77);
        assert!(matches!(b.get_vcp(&id, 0x8D), Err(Error::Unsupported(0x8D))));
    }

    #[test]
    fn verification_drops_claimed_but_dead_codes() {
        let mut b = MockBackend::demo();
        let id = b.monitors[0].info.id.clone();
        b.monitors[0].broken.push(0x62);
        let (caps, dropped) = verified_capabilities(&mut b, &id).unwrap();
        assert_eq!(dropped, vec![0x62]);
        assert!(caps.supports(0x10) && caps.supports(0x12) && !caps.supports(0x62));
    }

    #[test]
    fn unknown_monitor() {
        let mut b = MockBackend::demo();
        assert!(matches!(b.get_vcp(&"nope".to_string(), 0x10), Err(Error::NotFound(_))));
    }

    fn demo_source() -> SharedMock {
        SharedMock(Arc::new(Mutex::new(MockBackend::demo())))
    }

    #[test]
    fn hdr_demo_first_monitor_supports_hdr_second_does_not() {
        let mut s = demo_source();
        assert_eq!(s.hdr_state(r"\\.\DISPLAY1"), Some(HdrState { supported: true, enabled: false }));
        assert_eq!(s.hdr_state(r"\\.\DISPLAY2"), None);
    }

    #[test]
    fn hdr_set_roundtrip() {
        let mut s = demo_source();
        s.set_hdr(r"\\.\DISPLAY1", true).unwrap();
        assert!(s.hdr_state(r"\\.\DISPLAY1").unwrap().enabled);
        s.set_hdr(r"\\.\DISPLAY1", false).unwrap();
        assert!(!s.hdr_state(r"\\.\DISPLAY1").unwrap().enabled);
    }

    #[test]
    fn hdr_set_on_unsupported_device_errors() {
        let mut s = demo_source();
        assert!(matches!(s.set_hdr(r"\\.\DISPLAY2", true), Err(Error::Invalid(_))));
    }

    #[test]
    fn only_hides_the_other_monitor_but_keeps_it_listed() {
        let mut s = demo_source();
        s.set_display_mode(&DisplayMode::Only("GSM5B09-1234".into())).unwrap();
        assert_eq!(s.scan().unwrap().len(), 1);
        let m = s.display_modes().unwrap();
        assert_eq!(m.displays.len(), 2);
        assert_eq!(m.displays.iter().filter(|d| d.active).count(), 1);
        assert_eq!(m.current, Some(DisplayMode::Only("GSM5B09-1234".into())));
    }

    #[test]
    fn extend_and_duplicate_show_every_monitor() {
        let mut s = demo_source();
        s.set_display_mode(&DisplayMode::Only("GSM5B09-1234".into())).unwrap();
        s.set_display_mode(&DisplayMode::Extend).unwrap();
        assert_eq!(s.scan().unwrap().len(), 2);
        s.set_display_mode(&DisplayMode::Duplicate).unwrap();
        assert_eq!(s.display_modes().unwrap().current, Some(DisplayMode::Duplicate));
        assert_eq!(s.scan().unwrap().len(), 2);
    }

    #[test]
    fn unknown_display_key_is_rejected_and_changes_nothing() {
        let mut s = demo_source();
        assert!(matches!(s.set_display_mode(&DisplayMode::Only("gone".into())), Err(Error::NotFound(_))));
        assert_eq!(s.display_modes().unwrap().current, Some(DisplayMode::Extend));
    }
}
