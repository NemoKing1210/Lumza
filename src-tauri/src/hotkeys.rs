//! Global hotkeys (spec §4.3). Registration errors are collected per accelerator, so one
//! conflicting combination never stops the others from working.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use lumza_core::store::{Hotkey, HotkeyAction, SettingsStore};
use lumza_core::Feature;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

use crate::controller::Controller;
use crate::flyout::Flyout;
use crate::i18n;
use crate::osd::{self, OsdPayload};
use crate::scheduler::{self, Scheduler};

/// Accelerator -> why it could not be registered. Empty when everything is fine.
#[derive(Default)]
pub struct HotkeyStatus(pub Mutex<BTreeMap<String, String>>);

/// (Re-)register every hotkey from settings.
pub fn apply(app: &AppHandle) {
    let Some(store) = app.try_state::<Arc<SettingsStore>>() else { return };
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();

    let settings = store.get();
    let lang = settings.general.language;
    let mut errors = BTreeMap::new();
    let mut taken: Vec<Shortcut> = Vec::new();
    // Disabled: leave everything unregistered (and no stale errors) but keep the list.
    let active = if settings.hotkeys_enabled { settings.hotkeys } else { Vec::new() };
    for hk in active {
        if !hk.enabled || hk.accelerator.is_empty() {
            continue;
        }
        let shortcut = match hk.accelerator.parse::<Shortcut>() {
            Ok(s) => s,
            Err(e) => {
                errors.insert(hk.accelerator.clone(), i18n::tf(&lang, "be.hotkey.parse", &[("error", &e.to_string())]));
                continue;
            }
        };
        if taken.contains(&shortcut) {
            errors.insert(hk.accelerator.clone(), i18n::t(&lang, "be.hotkey.dup"));
            continue;
        }
        let action = hk.action.clone();
        let result = gs.on_shortcut(shortcut, move |app, _, event| {
            // Windows auto-repeats a held hotkey: every Pressed is a step, Released is ignored.
            if event.state() == ShortcutState::Pressed {
                let (app, action) = (app.clone(), action.clone());
                // Window creation (OSD) must not block the event loop that delivers this callback.
                std::thread::spawn(move || run(&app, &action));
            }
        });
        match result {
            Ok(()) => taken.push(shortcut),
            Err(e) => {
                tracing::warn!(accelerator = %hk.accelerator, error = %e, "hotkey registration failed");
                errors.insert(hk.accelerator.clone(), i18n::t(&lang, "be.hotkey.taken"));
            }
        }
    }

    if let Some(status) = app.try_state::<HotkeyStatus>() {
        *status.0.lock().unwrap() = errors.clone();
    }
    let _ = app.emit("hotkey-status", errors);
}

/// Unregister everything while the settings page records a new combination,
/// otherwise a registered one would fire instead of reaching the page.
pub fn pause(app: &AppHandle, paused: bool) {
    if paused {
        let _ = app.global_shortcut().unregister_all();
    } else {
        apply(app);
    }
}

fn run(app: &AppHandle, action: &HotkeyAction) {
    let Some(store) = app.try_state::<Arc<SettingsStore>>() else { return };
    let Some(ctrl) = app.try_state::<Arc<Controller>>() else { return };
    let settings = store.get();
    let step = settings.general.step as i32;

    let osd_payload = match action {
        HotkeyAction::Flyout => {
            if let Some(f) = app.try_state::<Flyout>() {
                f.toggle(app, None);
            }
            None
        }
        HotkeyAction::BrightnessAll { delta } => {
            scheduler::manual(app);
            ctrl.adjust(None, Feature::Brightness, step * *delta as i32)
        }
        HotkeyAction::Brightness { monitor_id, delta } => {
            scheduler::manual(app);
            ctrl.adjust(Some(monitor_id), Feature::Brightness, step * *delta as i32)
        }
        HotkeyAction::Volume { monitor_id, delta } => {
            scheduler::manual(app);
            ctrl.adjust(Some(monitor_id), Feature::Volume, step * *delta as i32)
        }
        HotkeyAction::ScheduleToggle => {
            if let Some(s) = app.try_state::<Arc<Scheduler>>() {
                let (paused, text) = s.toggle();
                let icon = if paused { "pause" } else { "play" };
                show_osd(app, OsdPayload { icon: icon.into(), label: text, percent: None });
            }
            return;
        }
        HotkeyAction::Profile { profile_id } => {
            scheduler::manual(app);
            let profile = settings.profiles.iter().find(|p| &p.id == profile_id).cloned();
            if let (Some(p), true) = (profile, ctrl.apply_profile(profile_id)) {
                show_osd(app, OsdPayload { icon: p.icon, label: p.name, percent: None });
            }
            return;
        }
    }
    .map(|r| OsdPayload { icon: r.feature.name().into(), label: r.label, percent: Some(r.percent) });

    if let Some(p) = osd_payload {
        show_osd(app, p);
    }
}

fn show_osd(app: &AppHandle, payload: OsdPayload) {
    let Some(store) = app.try_state::<Arc<SettingsStore>>() else { return };
    let osd = store.get().osd;
    if osd.enabled {
        osd::show(app, payload, &osd.position, osd.duration_ms);
    }
}

/// True if the two hotkey lists would register identically (used to skip needless re-registration).
pub fn same(a: &(bool, Vec<Hotkey>), b: &(bool, Vec<Hotkey>)) -> bool {
    a == b
}
