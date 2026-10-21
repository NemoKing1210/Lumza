//! Lumza core: hardware-independent monitor control.
//!
//! Backends implement [`DisplayBackend`]; the CLI and (later) the controller
//! only talk to that trait, so everything is testable with [`mock::MockBackend`].

pub mod backend;
pub mod caps;
pub mod error;
pub mod link;
pub mod mock;
pub mod scale;
pub mod schedule;
pub mod store;
pub mod types;

#[cfg(windows)]
pub mod dxva2;
#[cfg(windows)]
pub mod hdr;

pub use backend::{DisplayBackend, MonitorPort, PortSource};
pub use error::{Error, Result};
pub use types::{Feature, HdrState, MonitorId, MonitorInfo, VcpValue};
