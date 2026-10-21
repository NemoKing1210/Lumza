//! Start-with-Windows via the per-user `Run` registry key (no admin rights, no extra plugin).

use winreg::{enums::HKEY_CURRENT_USER, RegKey};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE: &str = "Lumza";

pub fn is_enabled() -> bool {
    RegKey::predef(HKEY_CURRENT_USER).open_subkey(RUN_KEY).and_then(|k| k.get_value::<String, _>(VALUE)).is_ok()
}

pub fn set(enabled: bool) -> std::io::Result<()> {
    let key = RegKey::predef(HKEY_CURRENT_USER).create_subkey(RUN_KEY)?.0;
    if enabled {
        let exe = std::env::current_exe()?;
        key.set_value(VALUE, &format!("\"{}\"", exe.display()))
    } else {
        match key.delete_value(VALUE) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    }
}
