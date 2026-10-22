#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod autostart;
mod clock;
mod controller;
mod display_modes;
mod flyout;
mod hotkeys;
mod i18n;
mod logging;
mod os_events;
mod osd;
mod scheduler;
mod tray;
mod update;

use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use controller::{Controller, Snapshot};
use display_modes::{DisplayModeDto, DisplayModesDto};
use flyout::Flyout;
use lumza_core::mock::{MockBackend, SharedMock};
use lumza_core::store::{LoadOutcome, Profile, Settings, SettingsStore};
use lumza_core::{Feature, PortSource};
use scheduler::{Preview, ScheduleState, Scheduler};
use serde::Serialize;
use tauri::{
    AppHandle, Emitter, Listener, Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent,
};

/// Every WebView2 window of one process must use identical browser arguments, so both
/// windows are built from this one constant. These flags drop GPU/background services:
/// ~90 MB less memory than the defaults for a UI this small.
pub(crate) const BROWSER_ARGS: &str = "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection \
--disable-gpu --disable-gpu-compositing --renderer-process-limit=1 --in-process-gpu \
--disable-background-networking --disable-component-update --disable-sync --no-pings \
--disable-breakpad --disable-extensions";

/// Set when `settings.json` was corrupt and had to be replaced; the UI shows it once.
struct Notice(Mutex<Option<String>>);

struct Paths {
    settings: PathBuf,
    cache: PathBuf,
    logs: PathBuf,
}

fn data_paths() -> Paths {
    // LUMZA_DATA_DIR redirects both files (used by tests and portable setups).
    if let Some(d) = std::env::var_os("LUMZA_DATA_DIR") {
        let d = PathBuf::from(d);
        return Paths { settings: d.join("settings.json"), cache: d.join("cache.json"), logs: d.join("logs") };
    }
    let base = |var: &str| std::env::var_os(var).map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    Paths {
        settings: base("APPDATA").join("Lumza").join("settings.json"),
        cache: base("LOCALAPPDATA").join("Lumza").join("cache.json"),
        logs: base("LOCALAPPDATA").join("Lumza").join("logs"),
    }
}

// ---------------------------------------------------------------- commands

#[tauri::command]
fn get_monitors(c: State<Arc<Controller>>) -> Snapshot {
    c.snapshot()
}

#[tauri::command]
fn refresh_monitors(c: State<Arc<Controller>>) {
    c.rescan();
}

#[tauri::command]
fn reload_values(c: State<Arc<Controller>>) {
    c.reload_values();
}

#[tauri::command]
fn set_value(app: AppHandle, c: State<Arc<Controller>>, monitor_id: String, feature: String, value: u8) {
    if let Ok(f) = feature.parse::<Feature>() {
        c.set_value(&monitor_id, f, value);
        scheduler::manual(&app);
    }
}

#[tauri::command]
fn set_hdr(c: State<Arc<Controller>>, monitor_id: String, on: bool) {
    c.set_hdr(&monitor_id, on);
}

#[tauri::command]
fn get_display_modes(c: State<Arc<Controller>>) -> Option<DisplayModesDto> {
    c.display_modes()
}

#[tauri::command]
fn set_display_mode(c: State<Arc<Controller>>, mode: DisplayModeDto) {
    c.set_display_mode(mode.into());
}

#[tauri::command]
fn fit_flyout(win: WebviewWindow, f: State<Flyout>, height: f64) {
    if height.is_finite() {
        f.fit(&win, height);
    }
}

#[tauri::command]
fn hide_flyout(win: WebviewWindow, f: State<Flyout>) {
    f.hide(&win);
}

#[tauri::command]
fn is_solid(f: State<Flyout>) -> bool {
    f.solid
}

#[tauri::command]
fn get_settings(s: State<Arc<SettingsStore>>) -> Settings {
    s.get()
}

#[tauri::command]
fn update_settings(
    app: AppHandle,
    s: State<Arc<SettingsStore>>,
    c: State<Arc<Controller>>,
    patch: serde_json::Value,
) -> Result<Settings, String> {
    let old = s.get();
    let new = s.update(&patch).map_err(|e| e.to_string())?;
    after_settings_change(&app, &c, &old, &new);
    Ok(new)
}

/// Shared tail of every command that changes settings: autostart, DDC timing, events.
fn after_settings_change(app: &AppHandle, c: &Arc<Controller>, old: &Settings, new: &Settings) {
    if new.general.autostart != old.general.autostart {
        if let Err(e) = autostart::set(new.general.autostart) {
            tracing::warn!(error = %e, "autostart update failed");
        }
    }
    let _ = app.emit("settings-changed", new);
    c.settings_changed();
    if new.ddc != old.ddc {
        c.rescan(); // new timing applies to freshly opened ports
    }
}

#[tauri::command]
async fn export_settings(s: State<'_, Arc<SettingsStore>>) -> Result<bool, String> {
    let text = s.export();
    let picked = tauri::async_runtime::spawn_blocking(move || {
        rfd::FileDialog::new().set_file_name("lumza-settings.json").add_filter("JSON", &["json"]).save_file()
    })
    .await
    .map_err(|e| e.to_string())?;
    match picked {
        Some(path) => std::fs::write(path, text).map(|_| true).map_err(|e| e.to_string()),
        None => Ok(false),
    }
}

#[tauri::command]
async fn import_settings(
    app: AppHandle,
    s: State<'_, Arc<SettingsStore>>,
    c: State<'_, Arc<Controller>>,
) -> Result<bool, String> {
    let picked =
        tauri::async_runtime::spawn_blocking(|| rfd::FileDialog::new().add_filter("JSON", &["json"]).pick_file())
            .await
            .map_err(|e| e.to_string())?;
    let Some(path) = picked else { return Ok(false) };
    let text = lumza_core::store::read_limited(&path).map_err(|e| e.to_string())?;
    let old = s.get();
    let new = s.import(&text).map_err(|e| e.to_string())?;
    after_settings_change(&app, &c, &old, &new);
    Ok(true)
}

#[tauri::command]
fn reset_settings(app: AppHandle, s: State<Arc<SettingsStore>>, c: State<Arc<Controller>>) -> Result<(), String> {
    let old = s.get();
    let new = s.reset().map_err(|e| e.to_string())?;
    after_settings_change(&app, &c, &old, &new);
    c.rescan(); // re-creates the built-in profiles for the monitors that are connected
    Ok(())
}

#[tauri::command]
fn get_system_language() -> &'static str {
    i18n::system_language()
}

#[tauri::command]
fn open_logs(p: State<LogDir>) {
    use std::os::windows::process::CommandExt;
    let _ = std::process::Command::new("explorer.exe").arg(&p.0).creation_flags(0x0800_0000).spawn();
}

#[tauri::command]
fn set_linked_value(app: AppHandle, c: State<Arc<Controller>>, feature: String, value: u8) {
    if let Ok(f) = feature.parse::<Feature>() {
        c.set_linked_value(f, value);
        scheduler::manual(&app);
    }
}

#[tauri::command]
fn apply_profile(app: AppHandle, c: State<Arc<Controller>>, id: String) -> bool {
    scheduler::manual(&app);
    c.apply_profile(&id)
}

#[tauri::command]
fn get_schedule_state(s: State<Arc<Scheduler>>) -> ScheduleState {
    s.state()
}

#[tauri::command]
fn pause_schedule(s: State<Arc<Scheduler>>, minutes: Option<u32>) {
    s.pause(minutes);
}

#[tauri::command]
fn resume_schedule(s: State<Arc<Scheduler>>) {
    s.resume();
}

#[tauri::command]
fn get_schedule_preview(s: State<Arc<Scheduler>>) -> Preview {
    s.preview()
}

/// "Save current values as profile": captures every feature of every connected monitor.
#[tauri::command]
fn save_current_as_profile(
    app: AppHandle,
    s: State<Arc<SettingsStore>>,
    c: State<Arc<Controller>>,
    name: String,
    icon: Option<String>,
) -> Result<Settings, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("empty profile name".into());
    }
    let millis = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let mut profiles = s.get().profiles;
    profiles.push(Profile {
        id: format!("p-{millis}"),
        name: name.to_string(),
        icon: icon.unwrap_or_else(|| "sun".into()),
        values: c.current_values(),
    });
    let new = s.update(&serde_json::json!({ "profiles": profiles })).map_err(|e| e.to_string())?;
    let _ = app.emit("settings-changed", &new);
    Ok(new)
}

#[tauri::command]
fn get_hotkey_status(s: State<hotkeys::HotkeyStatus>) -> std::collections::BTreeMap<String, String> {
    s.0.lock().unwrap().clone()
}

#[tauri::command]
fn hotkeys_pause(app: AppHandle, paused: bool) {
    hotkeys::pause(&app, paused);
}

#[tauri::command]
fn osd_take_pending(o: State<osd::Osd>) -> Option<osd::OsdPayload> {
    o.take_pending()
}

#[tauri::command]
fn open_settings(app: AppHandle) {
    open_settings_window(&app);
}

#[tauri::command]
fn take_notice(n: State<Notice>) -> Option<String> {
    n.0.lock().unwrap().take()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct About {
    version: &'static str,
    settings_path: String,
    log_path: String,
    repo_url: &'static str,
    autostart_registered: bool,
}

struct LogDir(PathBuf);

#[tauri::command]
async fn check_for_updates(app: AppHandle) -> Result<update::UpdateInfo, String> {
    update::check(&app).await
}

#[tauri::command]
async fn install_update(app: AppHandle) -> Result<(), String> {
    update::install(&app).await
}

/// Opens the project page in the default browser. The URL is a constant, never user input.
#[tauri::command]
fn open_repo() {
    use std::os::windows::process::CommandExt;
    let _ = std::process::Command::new("explorer.exe").arg(update::REPO_URL).creation_flags(0x0800_0000).spawn();
}

#[tauri::command]
fn about(s: State<Arc<SettingsStore>>, p: State<LogDir>) -> About {
    About {
        version: env!("CARGO_PKG_VERSION"),
        settings_path: s.path().display().to_string(),
        log_path: p.0.display().to_string(),
        repo_url: update::REPO_URL,
        autostart_registered: autostart::is_enabled(),
    }
}

// ----------------------------------------------------------------- windows

pub fn open_settings_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("settings") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
        return;
    }
    // Created on demand and destroyed on close, so it costs no memory while unused. Built on a
    // helper thread: callers run on the main thread (sync commands, tray and hotkey handlers), and
    // building a webview window there deadlocks on Windows, leaving the window blank.
    let app = app.clone();
    std::thread::spawn(move || {
        let built = WebviewWindowBuilder::new(&app, "settings", WebviewUrl::App("index.html".into()))
            .title(i18n::t(&i18n::lang_of(&app), "be.window.settings"))
            .inner_size(800.0, 560.0)
            .min_inner_size(640.0, 420.0)
            .center()
            .decorations(false)
            .shadow(true)
            .additional_browser_args(BROWSER_ARGS)
            .build();
        if let Err(e) = built {
            tracing::error!(error = %e, "settings window failed");
        }
    });
}

fn create_flyout(app: &tauri::App) -> tauri::Result<WebviewWindow> {
    WebviewWindowBuilder::new(app, "flyout", WebviewUrl::App("index.html".into()))
        .title("Lumza")
        .inner_size(360.0, 160.0)
        .visible(false)
        .decorations(false)
        .transparent(true)
        .resizable(false)
        .skip_taskbar(true)
        .always_on_top(true)
        .shadow(true)
        .focused(false)
        .additional_browser_args(BROWSER_ARGS)
        .build()
}

/// Mica/Acrylic behind the flyout, in the app's own theme: with "light" chosen on a dark system
/// (or the reverse) a backdrop that follows the system would put light cards on a dark panel.
fn apply_backdrop(win: &WebviewWindow, theme: &str) -> bool {
    let dark = match theme {
        "light" => Some(false),
        "dark" => Some(true),
        _ => None, // "system": follow Windows
    };
    let _ = window_vibrancy::clear_mica(win);
    let _ = window_vibrancy::clear_acrylic(win);
    let tint = if dark == Some(false) { (243, 243, 245, 160) } else { (24, 24, 24, 160) };
    window_vibrancy::apply_mica(win, dark).is_ok() || window_vibrancy::apply_acrylic(win, Some(tint)).is_ok()
}

fn main() {
    let started = Instant::now();
    let paths = data_paths();
    logging::init(paths.logs.clone());
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "starting");

    tauri::Builder::default()
        // A second launch just opens the flyout of the running instance (F-25).
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(f) = app.try_state::<Flyout>() {
                f.toggle(app, None);
            }
        }))
        .invoke_handler(tauri::generate_handler![
            get_monitors,
            refresh_monitors,
            reload_values,
            set_value,
            set_hdr,
            get_display_modes,
            set_display_mode,
            fit_flyout,
            hide_flyout,
            is_solid,
            get_settings,
            update_settings,
            set_linked_value,
            apply_profile,
            save_current_as_profile,
            get_schedule_state,
            pause_schedule,
            resume_schedule,
            get_schedule_preview,
            get_hotkey_status,
            hotkeys_pause,
            osd_take_pending,
            open_settings,
            take_notice,
            check_for_updates,
            install_update,
            open_repo,
            export_settings,
            import_settings,
            reset_settings,
            get_system_language,
            open_logs,
            about
        ])
        .on_window_event(|window, event| {
            if window.label() == "flyout" && matches!(event, WindowEvent::Focused(false)) {
                if let (Some(w), Some(f)) =
                    (window.app_handle().get_webview_window("flyout"), window.try_state::<Flyout>())
                {
                    f.hide(&w);
                }
            }
        })
        .setup(move |app| {
            let handle = app.handle().clone();
            app.manage(LogDir(paths.logs.clone()));

            let (store, outcome) = SettingsStore::load(paths.settings);
            let store = Arc::new(store);
            let notice = match outcome {
                LoadOutcome::Recovered(backup) => {
                    tracing::warn!(backup = %backup.display(), "settings.json was corrupt");
                    let lang = store.get().general.language;
                    Some(i18n::tf(&lang, "be.notice.settingsBroken", &[("path", &backup.display().to_string())]))
                }
                _ => None,
            };
            app.manage(Notice(Mutex::new(notice)));
            app.manage(osd::Osd::default());
            app.manage(hotkeys::HotkeyStatus::default());
            app.manage(store.clone());

            // Default is "start with Windows" (F-25); keep the registry in line with the setting.
            // LUMZA_NO_AUTOSTART lets test runs leave the registry alone.
            if std::env::var_os("LUMZA_NO_AUTOSTART").is_none() {
                let want = store.get().general.autostart;
                if want != autostart::is_enabled() {
                    if let Err(e) = autostart::set(want) {
                        tracing::warn!(error = %e, "autostart sync failed");
                    }
                }
            }

            // The tray icon comes first: it must appear well under a second after launch, while the
            // WebView2 window below takes a few hundred milliseconds to create.
            tray::build(app.handle())?;
            tracing::info!(ms = started.elapsed().as_millis() as u64, "tray icon ready");

            let win = create_flyout(app)?;
            let solid = !apply_backdrop(&win, &store.get().general.theme);
            app.manage(Flyout::new(solid));
            tracing::info!(ms = started.elapsed().as_millis() as u64, "flyout window ready");

            let source: Box<dyn PortSource> = if std::env::var_os("LUMZA_MOCK").is_some() {
                Box::new(SharedMock(Arc::new(Mutex::new(MockBackend::demo()))))
            } else {
                Box::new(lumza_core::dxva2::Dxva2Backend::new())
            };
            let controller = Controller::new(handle.clone(), source, store.clone(), paths.cache);
            app.manage(controller.clone());

            // Started after the controller is managed; it reads monitors and settings from state.
            let scheduler = Scheduler::start(&handle);
            app.manage(scheduler.clone());

            // Profiles live in the tray menu, so it is rebuilt whenever settings change.
            let menu_handle = handle.clone();
            app.listen("settings-changed", move |_| {
                tray::rebuild_menu(&menu_handle);
                tray::update_icon(&menu_handle);
            });
            let menu_handle = handle.clone();
            app.listen("schedule-state", move |_| tray::rebuild_menu(&menu_handle));
            // Displays come and go with hot-plug and layout switches, so the display submenu follows them.
            let menu_handle = handle.clone();
            app.listen("display-modes-changed", move |_| tray::rebuild_menu(&menu_handle));

            // The schedule re-evaluates when monitors (re)appear and when its own inputs change.
            let s = scheduler.clone();
            app.listen("scan-finished", move |_| s.nudge());
            let s = scheduler.clone();
            let sched_store = store.clone();
            let last = Arc::new(Mutex::new({
                let g = sched_store.get();
                (g.schedule, g.profiles, g.general.language)
            }));
            app.listen("settings-changed", move |_| {
                let g = sched_store.get();
                let mut last = last.lock().unwrap();
                if last.0 != g.schedule || last.1 != g.profiles || last.2 != g.general.language {
                    *last = (g.schedule, g.profiles, g.general.language);
                    s.nudge();
                }
            });

            // Re-tint the flyout backdrop when the theme setting changes.
            let theme_handle = handle.clone();
            let theme_store = store.clone();
            let last_theme = Mutex::new(store.get().general.theme);
            app.listen("settings-changed", move |_| {
                let theme = theme_store.get().general.theme;
                let mut last = last_theme.lock().unwrap();
                if *last != theme {
                    if let Some(w) = theme_handle.get_webview_window("flyout") {
                        apply_backdrop(&w, &theme);
                    }
                    *last = theme;
                }
            });

            // Keep the settings window title in the UI language.
            let title_handle = handle.clone();
            app.listen("settings-changed", move |_| {
                if let Some(w) = title_handle.get_webview_window("settings") {
                    let _ = w.set_title(&i18n::t(&i18n::lang_of(&title_handle), "be.window.settings"));
                }
            });

            // Hotkeys are re-registered only when the hotkey list itself changed.
            hotkeys::apply(&handle);
            let hk_state = |s: lumza_core::store::Settings| (s.hotkeys_enabled, s.hotkeys);
            let applied = Arc::new(Mutex::new(hk_state(store.get())));
            let (hk_handle, hk_store) = (handle.clone(), store.clone());
            app.listen("settings-changed", move |_| {
                let now = hk_state(hk_store.get());
                let mut last = applied.lock().unwrap();
                if !hotkeys::same(&last, &now) {
                    *last = now;
                    hotkeys::apply(&hk_handle);
                }
            });

            // Debounce bursts of OS events (a hot-plug fires several) into one rescan.
            let (tx, rx) = mpsc::channel::<()>();
            let c = controller.clone();
            std::thread::spawn(move || {
                while rx.recv().is_ok() {
                    while rx.recv_timeout(Duration::from_millis(800)).is_ok() {}
                    c.rescan();
                }
            });
            let tray_handle = handle.clone();
            let time_sched = scheduler.clone();
            let tx = Mutex::new(tx);
            os_events::start(move |ev| match ev {
                os_events::OsEvent::Rescan => {
                    let _ = tx.lock().unwrap().send(());
                }
                os_events::OsEvent::Theme => tray::update_icon(&tray_handle),
                os_events::OsEvent::Time => time_sched.nudge(),
            });

            controller.rescan();
            update::spawn_startup_check(&handle);
            tracing::info!(ms = started.elapsed().as_millis() as u64, "startup complete");

            // Debug aids: open the flyout / settings without a tray click.
            if std::env::var_os("LUMZA_SHOW").is_some() {
                let h = handle.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(2500));
                    if let Some(f) = h.try_state::<Flyout>() {
                        f.toggle(&h, None);
                    }
                });
            }
            if std::env::var_os("LUMZA_SHOW_SETTINGS").is_some() {
                let h = handle.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(2500));
                    open_settings_window(&h);
                });
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Lumza");
}
