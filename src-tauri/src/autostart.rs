//! Start-with-Windows via the per-user `Run` registry key (no admin rights, no extra plugin).

use winreg::{enums::HKEY_CURRENT_USER, RegKey};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE: &str = "Lumza";

fn command() -> std::io::Result<String> {
    Ok(format!("\"{}\"", std::env::current_exe()?.display()))
}

pub fn is_enabled() -> bool {
    RegKey::predef(HKEY_CURRENT_USER).open_subkey(RUN_KEY).and_then(|k| k.get_value::<String, _>(VALUE)).is_ok()
}

/// Registered and pointing at this very executable. A stale path (moved install, or a dev build
/// that registered itself) counts as not in sync, so the startup check rewrites it.
pub fn is_current() -> bool {
    let registered =
        RegKey::predef(HKEY_CURRENT_USER).open_subkey(RUN_KEY).and_then(|k| k.get_value::<String, _>(VALUE)).ok();
    registered.is_some() && registered == command().ok()
}

pub fn set(enabled: bool) -> std::io::Result<()> {
    let key = RegKey::predef(HKEY_CURRENT_USER).create_subkey(RUN_KEY)?.0;
    if enabled {
        key.set_value(VALUE, &command()?)
    } else {
        match key.delete_value(VALUE) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    }
}
