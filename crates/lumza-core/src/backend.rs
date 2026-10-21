use crate::caps::Capabilities;
use crate::error::Result;
use crate::types::{Feature, MonitorId, MonitorInfo, VcpValue};

/// Abstraction over real hardware (Dxva2, WMI) and the mock.
/// Implementations must not be called from a UI thread (DDC/CI is slow).
pub trait DisplayBackend: Send {
    /// Enumerate monitors currently attached. Re-scans on every call.
    fn list(&mut self) -> Result<Vec<MonitorInfo>>;
    /// Raw capabilities string, as returned by the monitor.
    fn capabilities_string(&mut self, id: &MonitorId) -> Result<String>;
    fn get_vcp(&mut self, id: &MonitorId, code: u8) -> Result<VcpValue>;
    fn set_vcp(&mut self, id: &MonitorId, code: u8, value: u32) -> Result<()>;
}

/// DDC/CI channel of a single monitor; owned by that monitor's worker thread.
pub trait MonitorPort: Send {
    fn capabilities_string(&mut self) -> Result<String>;
    fn get_vcp(&mut self, code: u8) -> Result<VcpValue>;
    fn set_vcp(&mut self, code: u8, value: u32) -> Result<()>;
}

/// Produces a fresh set of ports on every scan (spec F-01..F-06).
pub trait PortSource: Send {
    fn scan(&mut self) -> Result<Vec<(MonitorInfo, Box<dyn MonitorPort>)>>;

    /// DDC/CI timing for the ports created by the next [`scan`](PortSource::scan).
    /// Backends without real timing (the mock) ignore it.
    fn set_timing(&mut self, _write_delay_ms: u32, _read_delay_ms: u32, _retries: u32) {}
}

/// Capabilities with every advertised feature we control verified by a test read
/// (spec F-04): codes the monitor claims but does not answer are dropped.
/// Returns the filtered capabilities and the dropped codes.
pub fn verified_capabilities(backend: &mut dyn DisplayBackend, id: &MonitorId) -> Result<(Capabilities, Vec<u8>)> {
    let raw = backend.capabilities_string(id)?;
    let mut caps = crate::caps::parse(&raw);
    let mut dropped = Vec::new();
    for feature in Feature::ALL {
        let code = feature.vcp();
        if caps.supports(code) && backend.get_vcp(id, code).is_err() {
            caps.vcp.remove(&code);
            dropped.push(code);
        }
    }
    Ok((caps, dropped))
}
