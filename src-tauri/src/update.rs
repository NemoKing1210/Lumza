//! Update check and install (spec §6, §10). The only network traffic the app ever makes:
//! one HTTPS request for `latest.json` from the project's GitHub release, and only when the
//! user has not turned it off. Update packages are verified against the key in `tauri.conf.json`.

use std::sync::Arc;
use std::time::Duration;

use lumza_core::store::SettingsStore;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::UpdaterExt;

/// Project page, opened from About. The updater endpoint in `tauri.conf.json` points at the same repo.
pub const REPO_URL: &str = "https://github.com/NemoKing1210/Lumza";

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub available: bool,
    pub version: Option<String>,
    pub notes: Option<String>,
}

pub async fn check(app: &AppHandle) -> Result<UpdateInfo, String> {
    let updater = app.updater().map_err(|e| e.to_string())?;
    match updater.check().await {
        Ok(Some(u)) => Ok(UpdateInfo { available: true, version: Some(u.version.clone()), notes: u.body.clone() }),
        Ok(None) => Ok(UpdateInfo::default()),
        Err(e) => Err(e.to_string()),
    }
}

/// Download, verify and install the newest release, then restart into it.
pub async fn install(app: &AppHandle) -> Result<(), String> {
    let updater = app.updater().map_err(|e| e.to_string())?;
    let Some(update) = updater.check().await.map_err(|e| e.to_string())? else {
        return Ok(());
    };
    update.download_and_install(|_, _| {}, || {}).await.map_err(|e| e.to_string())?;
    app.restart()
}

/// One silent check shortly after start-up; failures (offline, no release yet) are only logged.
pub fn spawn_startup_check(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(20));
        let enabled = app.try_state::<Arc<SettingsStore>>().is_some_and(|s| s.get().general.check_updates);
        if !enabled {
            return;
        }
        match tauri::async_runtime::block_on(check(&app)) {
            Ok(info) => {
                tracing::info!(available = info.available, version = ?info.version, "update check");
                let _ = app.emit("update-available", info);
            }
            Err(e) => tracing::info!(error = %e, "update check failed"),
        }
    });
}
