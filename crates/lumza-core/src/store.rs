//! Persistent settings (`settings.json`, spec §8) and the values cache (`cache.json`).
//!
//! Settings carry a `schemaVersion` for migrations; a corrupt file is renamed to
//! `settings.broken-<timestamp>.json` and defaults are used. All writes are atomic
//! (temp file + rename) so a crash never leaves a half-written file.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{Error, Result};

pub const SCHEMA_VERSION: u32 = 4;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct General {
    pub autostart: bool,
    /// A `locales/<code>.json` code, or "auto" to follow the system language (English if unsupported).
    pub language: String,
    /// "system" | "light" | "dark"
    pub theme: String,
    /// Wheel/keyboard step in percent.
    pub step: u8,
    /// "hover" | "always"
    pub show_values: String,
    /// Look for a newer release at start-up. The only network request the app makes.
    pub check_updates: bool,
    /// One of `TRAY_ICONS`.
    pub tray_icon: String,
}

/// Tray icon styles; the order is the order of the picker.
pub const TRAY_ICONS: [&str; 6] = ["contrast", "sun", "moon", "monitor", "sliders", "color"];

impl Default for General {
    fn default() -> Self {
        Self {
            autostart: true,
            language: "auto".into(),
            theme: "system".into(),
            step: 2,
            show_values: "always".into(),
            check_updates: true,
            tray_icon: TRAY_ICONS[0].into(),
        }
    }
}

/// Calibrated range, in monitor percent, that the common 0–100 % maps onto (spec §4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Range {
    pub min: u8,
    pub max: u8,
}

impl Default for Range {
    fn default() -> Self {
        Self { min: 0, max: 100 }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MonitorSettings {
    pub custom_name: Option<String>,
    /// Badge colour as `#rrggbb`; `None` uses the accent colour.
    pub color: Option<String>,
    /// Badge icon key (see the UI's `MONITOR_ICONS`); `None` (the default) hides the badge, `"number"` shows the monitor number.
    pub icon: Option<String>,
    pub hidden: bool,
    pub order: i32,
    /// Features the user chose not to show in the flyout ("brightness" | "contrast" | "volume").
    pub hidden_features: Vec<String>,
    /// Per-feature calibration range used by linked sliders.
    pub calibration: BTreeMap<String, Range>,
    /// Monitor ignores the "link monitors" switch.
    pub excluded_from_link: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Linked {
    pub enabled: bool,
    /// Features that get one common slider. Linking is per feature (spec §4.1).
    pub features: Vec<String>,
    /// Keep individual sliders next to the common one.
    pub show_individual: bool,
}

impl Default for Linked {
    fn default() -> Self {
        Self { enabled: false, features: vec!["brightness".into()], show_individual: false }
    }
}

/// What a hotkey does. `delta` is only a direction (+1 / -1); the size of the step comes
/// from `general.step`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum HotkeyAction {
    Flyout,
    BrightnessAll {
        delta: i8,
    },
    Brightness {
        monitor_id: String,
        delta: i8,
    },
    Volume {
        monitor_id: String,
        delta: i8,
    },
    Profile {
        profile_id: String,
    },
    /// Pause the schedule, or resume it if it is paused.
    ScheduleToggle,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hotkey {
    /// Accelerator such as `Ctrl+Alt+ArrowUp`. Empty means "not assigned".
    pub accelerator: String,
    pub action: HotkeyAction,
    /// A disabled hotkey stays in the list but is not registered.
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
}

fn enabled_by_default() -> bool {
    true
}

pub fn default_hotkeys() -> Vec<Hotkey> {
    vec![
        Hotkey {
            accelerator: "Ctrl+Alt+ArrowUp".into(),
            action: HotkeyAction::BrightnessAll { delta: 1 },
            enabled: true,
        },
        Hotkey {
            accelerator: "Ctrl+Alt+ArrowDown".into(),
            action: HotkeyAction::BrightnessAll { delta: -1 },
            enabled: true,
        },
        Hotkey { accelerator: "Ctrl+Alt+B".into(), action: HotkeyAction::Flyout, enabled: true },
    ]
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Location {
    pub lat: f64,
    pub lon: f64,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Trigger {
    /// Fixed local time, `"HH:MM"`.
    Time { at: String },
    /// `event` is "sunrise" or "sunset"; `offset_min` shifts it (negative = earlier).
    Sun { event: String, offset_min: i32 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ScheduleAction {
    Profile {
        profile_id: String,
    },
    /// Brightness 0–100 %. `monitors: None` means all monitors, mapped through each monitor's
    /// calibration range like a linked slider; a list sets those monitors' raw percent.
    Brightness {
        value: u8,
        monitors: Option<Vec<String>>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchedulePoint {
    pub id: String,
    pub trigger: Trigger,
    pub action: ScheduleAction,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Schedule {
    pub enabled: bool,
    /// Smooth transition length, 0–60 minutes.
    pub transition_min: u32,
    /// What a manual change does: "untilNext" | "minutes" | "off".
    pub pause_on_manual: String,
    pub pause_minutes: u32,
    /// Do not touch the monitors while a full-screen app is in the foreground.
    pub skip_fullscreen: bool,
    /// Needed for sunrise/sunset points; entered by hand, never looked up (spec §4.4).
    pub location: Option<Location>,
    pub points: Vec<SchedulePoint>,
}

impl Default for Schedule {
    fn default() -> Self {
        Self {
            enabled: false,
            transition_min: 10,
            pause_on_manual: "untilNext".into(),
            pause_minutes: 60,
            skip_fullscreen: false,
            location: None,
            points: Vec::new(),
        }
    }
}

/// On-screen display shown when a hotkey fires (spec §4.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Osd {
    pub enabled: bool,
    /// "top" | "bottom"
    pub position: String,
    pub duration_ms: u32,
}

impl Default for Osd {
    fn default() -> Self {
        Self { enabled: true, position: "bottom".into(), duration_ms: 1500 }
    }
}

/// DDC/CI timing for "capricious" monitors (spec §6). The standard asks for ≥ 50 ms after a
/// write and ≥ 40 ms after a read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Ddc {
    pub write_delay_ms: u32,
    pub read_delay_ms: u32,
    pub retries: u32,
}

impl Default for Ddc {
    fn default() -> Self {
        Self { write_delay_ms: 50, read_delay_ms: 40, retries: 2 }
    }
}

/// A named set of values. Every entry is optional: a missing monitor or feature is left untouched.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub icon: String,
    /// monitor id -> feature -> 0–100 % of that monitor.
    pub values: BTreeMap<String, BTreeMap<String, u8>>,
}

impl Profile {
    /// The profile's values re-read from `current`: the same monitors and features, at their present levels.
    /// Entries `current` has no value for (a monitor that is unplugged) keep what was stored.
    pub fn refreshed(
        &self,
        current: &BTreeMap<String, BTreeMap<String, u8>>,
    ) -> BTreeMap<String, BTreeMap<String, u8>> {
        self.values
            .iter()
            .map(|(monitor, features)| {
                let now = current.get(monitor);
                let features = features
                    .iter()
                    .map(|(f, old)| (f.clone(), now.and_then(|m| m.get(f)).copied().unwrap_or(*old)))
                    .collect();
                (monitor.clone(), features)
            })
            .collect()
    }
}

impl Default for Profile {
    fn default() -> Self {
        Self { id: String::new(), name: String::new(), icon: "sun".into(), values: BTreeMap::new() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub schema_version: u32,
    pub general: General,
    pub linked: Linked,
    pub profiles: Vec<Profile>,
    pub hotkeys: Vec<Hotkey>,
    /// Master switch: when false no global hotkey is registered, the list is kept.
    pub hotkeys_enabled: bool,
    pub osd: Osd,
    pub schedule: Schedule,
    pub ddc: Ddc,
    /// The built-in "Day / Night / Games" profiles were created once; deleting them sticks.
    pub templates_seeded: bool,
    /// Kept for monitors that are currently unplugged, so they are restored on reconnect.
    pub monitors: BTreeMap<String, MonitorSettings>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            general: General::default(),
            linked: Linked::default(),
            profiles: Vec::new(),
            hotkeys: default_hotkeys(),
            hotkeys_enabled: false,
            osd: Osd::default(),
            schedule: Schedule::default(),
            ddc: Ddc::default(),
            templates_seeded: false,
            monitors: BTreeMap::new(),
        }
    }
}

/// UI languages that have a `locales/<code>.json`.
pub const LANGUAGES: [&str; 7] = ["en", "ru", "es", "de", "fr", "pt", "zh"];

/// A settings or cache file larger than this is not ours (real ones are a few KB); it is refused
/// instead of being read into memory.
pub const MAX_DOCUMENT_BYTES: usize = 1 << 20;
/// Limits for user-entered text and lists. Far above any real use; they only stop an imported or
/// hand-edited file from making every settings broadcast huge.
const MAX_NAME_CHARS: usize = 80;
const MAX_PROFILES: usize = 200;
const MAX_HOTKEYS: usize = 64;
const MAX_SCHEDULE_POINTS: usize = 200;

/// Cut `s` to at most `max` characters.
fn clip(s: &mut String, max: usize) {
    if let Some((i, _)) = s.char_indices().nth(max) {
        s.truncate(i);
    }
}

fn is_icon_key(s: &str) -> bool {
    !s.is_empty() && s.len() <= 24 && s.chars().all(|c| c.is_ascii_alphanumeric())
}

fn is_hex_color(s: &str) -> bool {
    s.len() == 7 && s.starts_with('#') && s[1..].chars().all(|c| c.is_ascii_hexdigit())
}

impl Settings {
    fn sanitize(&mut self) {
        self.general.step = self.general.step.clamp(1, 20);
        if !["system", "light", "dark"].contains(&self.general.theme.as_str()) {
            self.general.theme = "system".into();
        }
        if self.general.language != "auto" && !LANGUAGES.contains(&self.general.language.as_str()) {
            self.general.language = "auto".into();
        }
        if !["hover", "always"].contains(&self.general.show_values.as_str()) {
            self.general.show_values = "always".into();
        }
        if !TRAY_ICONS.contains(&self.general.tray_icon.as_str()) {
            self.general.tray_icon = TRAY_ICONS[0].into();
        }
        for m in self.monitors.values_mut() {
            if m.color.as_deref().is_some_and(|c| !is_hex_color(c)) {
                m.color = None;
            }
            if m.icon.as_deref().is_some_and(|i| !is_icon_key(i)) {
                m.icon = None;
            }
            if let Some(n) = &mut m.custom_name {
                clip(n, MAX_NAME_CHARS);
            }
            for r in m.calibration.values_mut() {
                r.max = r.max.min(100);
                r.min = r.min.min(r.max);
            }
        }
        self.profiles.truncate(MAX_PROFILES);
        for p in &mut self.profiles {
            clip(&mut p.name, MAX_NAME_CHARS);
            if !is_icon_key(&p.icon) {
                p.icon = "sun".into();
            }
            for feats in p.values.values_mut() {
                for v in feats.values_mut() {
                    *v = (*v).min(100);
                }
            }
        }
        self.osd.duration_ms = self.osd.duration_ms.clamp(500, 5000);
        if !["top", "bottom"].contains(&self.osd.position.as_str()) {
            self.osd.position = "bottom".into();
        }
        self.hotkeys.truncate(MAX_HOTKEYS);
        for h in &mut self.hotkeys {
            h.accelerator = h.accelerator.trim().to_string();
            clip(&mut h.accelerator, MAX_NAME_CHARS);
            match &mut h.action {
                HotkeyAction::BrightnessAll { delta }
                | HotkeyAction::Brightness { delta, .. }
                | HotkeyAction::Volume { delta, .. } => *delta = if *delta < 0 { -1 } else { 1 },
                _ => {}
            }
        }
        self.ddc.write_delay_ms = self.ddc.write_delay_ms.clamp(20, 500);
        self.ddc.read_delay_ms = self.ddc.read_delay_ms.clamp(20, 500);
        self.ddc.retries = self.ddc.retries.min(5);
        let sc = &mut self.schedule;
        sc.transition_min = sc.transition_min.min(60);
        sc.pause_minutes = sc.pause_minutes.clamp(1, 1440);
        if !["untilNext", "minutes", "off"].contains(&sc.pause_on_manual.as_str()) {
            sc.pause_on_manual = "untilNext".into();
        }
        if let Some(l) = &mut sc.location {
            l.lat = l.lat.clamp(-90.0, 90.0);
            l.lon = l.lon.clamp(-180.0, 180.0);
            clip(&mut l.label, MAX_NAME_CHARS);
        }
        sc.points.truncate(MAX_SCHEDULE_POINTS);
        for p in &mut sc.points {
            if let Trigger::Time { at } = &mut p.trigger {
                if crate::schedule::parse_hhmm(at).is_none() {
                    *at = "00:00".into();
                }
            }
            if let Trigger::Sun { event, offset_min } = &mut p.trigger {
                if event != "sunrise" && event != "sunset" {
                    *event = "sunrise".into();
                }
                *offset_min = (*offset_min).clamp(-720, 720);
            }
            if let ScheduleAction::Brightness { value, .. } = &mut p.action {
                *value = (*value).min(100);
            }
        }
        self.schema_version = SCHEMA_VERSION;
    }
}

/// Upgrades an older settings document in place, one version at a time.
fn migrate(doc: &mut Value) {
    let version = doc.get("schemaVersion").and_then(Value::as_u64).unwrap_or(0);
    if version < 1 {
        // v0 (pre-release drafts) kept `autostart` and `step` at the top level.
        if let Some(obj) = doc.as_object_mut() {
            let mut general = obj.remove("general").unwrap_or_else(|| Value::Object(Default::default()));
            for key in ["autostart", "step", "theme", "language"] {
                if let Some(v) = obj.remove(key) {
                    general[key] = v;
                }
            }
            obj.insert("general".into(), general);
            obj.insert("schemaVersion".into(), 1.into());
        }
    }
    if version < 2 {
        // v2 only added fields (linked, profiles, calibration); serde defaults fill them in.
        if let Some(obj) = doc.as_object_mut() {
            obj.insert("schemaVersion".into(), 2.into());
        }
    }
    if version < 3 {
        // v3 only added hotkeys and osd; serde defaults supply the stock hotkeys.
        if let Some(obj) = doc.as_object_mut() {
            obj.insert("schemaVersion".into(), 3.into());
        }
    }
    if version < 4 {
        // v4 only added the schedule block; it is disabled by default.
        if let Some(obj) = doc.as_object_mut() {
            obj.insert("schemaVersion".into(), 4.into());
        }
    }
}

/// RFC 7396 JSON merge patch: objects merge recursively, `null` deletes, anything else replaces.
pub fn merge_patch(target: &mut Value, patch: &Value) {
    match patch {
        Value::Object(p) => {
            if !target.is_object() {
                *target = Value::Object(Default::default());
            }
            let t = target.as_object_mut().unwrap();
            for (k, v) in p {
                if v.is_null() {
                    t.remove(k);
                } else {
                    merge_patch(t.entry(k.clone()).or_insert(Value::Null), v);
                }
            }
        }
        other => *target = other.clone(),
    }
}

/// Temp file + rename. Writers are serialised: two of them sharing one `.tmp` name would
/// interleave their bytes, and the rename would publish the mix.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    static WRITING: Mutex<()> = Mutex::new(());
    let _guard = WRITING.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    let mut file = fs::File::create(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?; // on disk before the rename, so a power cut cannot leave an empty file
    drop(file);
    fs::rename(&tmp, path)
}

#[derive(Debug, PartialEq, Eq)]
pub enum LoadOutcome {
    Fresh,
    Loaded,
    /// The file was unreadable; it was moved to this path and defaults are in use.
    Recovered(PathBuf),
}

pub struct SettingsStore {
    path: PathBuf,
    current: Mutex<Settings>,
    /// Serialises edits from read to disk write. `current` is only held for an instant, so readers
    /// (sliders, hotkeys, the controller) never wait for an fsync.
    writer: Mutex<()>,
}

impl SettingsStore {
    pub fn load(path: PathBuf) -> (Self, LoadOutcome) {
        let (settings, outcome) = match read_limited(&path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => (Settings::default(), LoadOutcome::Fresh),
            Err(_) => (Settings::default(), LoadOutcome::Recovered(quarantine(&path))),
            Ok(text) => match parse(&text) {
                Some(s) => (s, LoadOutcome::Loaded),
                None => (Settings::default(), LoadOutcome::Recovered(quarantine(&path))),
            },
        };
        let store = Self { path, current: Mutex::new(settings), writer: Mutex::new(()) };
        if outcome != LoadOutcome::Loaded {
            let _ = store.save(&store.get());
        }
        (store, outcome)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn get(&self) -> Settings {
        self.read(Settings::clone)
    }

    /// Look at the settings without copying them all: for hot paths that need one field.
    pub fn read<R>(&self, f: impl FnOnce(&Settings) -> R) -> R {
        f(&self.current.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// Apply a JSON merge patch (IPC `update_settings(patch)`), validate, persist.
    /// On error nothing changes.
    pub fn update(&self, patch: &Value) -> Result<Settings> {
        let _edit = self.writer.lock().unwrap_or_else(PoisonError::into_inner);
        let mut doc = serde_json::to_value(self.get()).map_err(|e| Error::Invalid(e.to_string()))?;
        merge_patch(&mut doc, patch);
        let mut next: Settings =
            serde_json::from_value(doc).map_err(|e| Error::Invalid(format!("bad settings patch: {e}")))?;
        next.sanitize();
        self.commit(&next)?;
        Ok(next)
    }

    /// Replace everything with a settings document exported earlier (any schema version).
    /// A document that does not parse changes nothing.
    pub fn import(&self, text: &str) -> Result<Settings> {
        if text.len() > MAX_DOCUMENT_BYTES {
            return Err(Error::Invalid("settings file is too large".into()));
        }
        let next = parse(text).ok_or_else(|| Error::Invalid("not a Lumza settings file".into()))?;
        // Under the writer lock, like `update`, so a concurrent edit cannot be overwritten on disk
        // by an older document.
        let _edit = self.writer.lock().unwrap_or_else(PoisonError::into_inner);
        self.commit(&next)?;
        Ok(next)
    }

    /// Factory defaults. Unplugged monitors' names and calibration are forgotten too.
    pub fn reset(&self) -> Result<Settings> {
        let next = Settings::default();
        let _edit = self.writer.lock().unwrap_or_else(PoisonError::into_inner);
        self.commit(&next)?;
        Ok(next)
    }

    /// Disk first, memory after: a failed write changes nothing. The caller holds `writer`.
    fn commit(&self, next: &Settings) -> Result<()> {
        self.save(next).map_err(|e| Error::Os(e.to_string()))?;
        *self.current.lock().unwrap_or_else(PoisonError::into_inner) = next.clone();
        Ok(())
    }

    /// The current settings as pretty JSON, for export.
    pub fn export(&self) -> String {
        serde_json::to_string_pretty(&self.get()).unwrap_or_default()
    }

    fn save(&self, s: &Settings) -> io::Result<()> {
        let text = serde_json::to_vec_pretty(s).map_err(io::Error::other)?;
        write_atomic(&self.path, &text)
    }
}

/// Reads a text file of at most [`MAX_DOCUMENT_BYTES`]; a bigger one is an `InvalidData` error.
pub fn read_limited(path: &Path) -> io::Result<String> {
    if fs::metadata(path)?.len() > MAX_DOCUMENT_BYTES as u64 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "file is too large"));
    }
    fs::read_to_string(path)
}

fn parse(text: &str) -> Option<Settings> {
    let mut doc: Value = serde_json::from_str(text).ok()?;
    if !doc.is_object() {
        return None;
    }
    migrate(&mut doc);
    let mut s: Settings = serde_json::from_value(doc).ok()?;
    s.sanitize();
    Some(s)
}

fn quarantine(path: &Path) -> PathBuf {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let dest = path.with_file_name(format!("settings.broken-{secs}.json"));
    let _ = fs::rename(path, &dest);
    dest
}

// ------------------------------------------------------------------ cache

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CachedFeature {
    pub feature: String,
    pub max: u32,
    pub percent: u8,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CachedMonitor {
    pub id: String,
    pub number: u32,
    pub name: String,
    pub features: Vec<CachedFeature>,
}

/// Last known monitor values, shown instantly while the first live read is in flight.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Cache {
    pub monitors: Vec<CachedMonitor>,
}

impl Cache {
    /// A missing or corrupt cache is not an error: it is only an optimisation.
    pub fn load(path: &Path) -> Self {
        read_limited(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        write_atomic(path, &serde_json::to_vec(self).map_err(io::Error::other)?)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn refreshed_profile_keeps_its_scope_and_unplugged_values() {
        let map = |pairs: &[(&str, &[(&str, u8)])]| -> BTreeMap<String, BTreeMap<String, u8>> {
            pairs.iter().map(|(m, f)| (m.to_string(), f.iter().map(|(k, v)| (k.to_string(), *v)).collect())).collect()
        };
        let p =
            Profile { values: map(&[("a", &[("brightness", 10)]), ("gone", &[("volume", 5)])]), ..Profile::default() };
        // `a` also has contrast now, which the profile never covered; `gone` is not connected.
        let current = map(&[("a", &[("brightness", 70), ("contrast", 40)]), ("b", &[("brightness", 1)])]);
        assert_eq!(p.refreshed(&current), map(&[("a", &[("brightness", 70)]), ("gone", &[("volume", 5)])]));
    }

    use super::*;
    use serde_json::json;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lumza-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn fresh_defaults_and_roundtrip() {
        let dir = tmp("fresh");
        let p = dir.join("settings.json");
        let (s, out) = SettingsStore::load(p.clone());
        assert_eq!(out, LoadOutcome::Fresh);
        assert!(s.get().general.autostart);
        s.update(&json!({"general": {"step": 5, "theme": "dark"}})).unwrap();
        drop(s);
        let (s2, out) = SettingsStore::load(p);
        assert_eq!(out, LoadOutcome::Loaded);
        assert_eq!(s2.get().general.step, 5);
        assert_eq!(s2.get().general.theme, "dark");
    }

    #[test]
    fn patch_merges_and_null_deletes() {
        let dir = tmp("patch");
        let (s, _) = SettingsStore::load(dir.join("settings.json"));
        s.update(&json!({"monitors": {"A": {"customName": "Left", "order": 1}}})).unwrap();
        s.update(&json!({"monitors": {"A": {"hidden": true}}})).unwrap();
        let m = s.get().monitors["A"].clone();
        assert_eq!((m.custom_name.as_deref(), m.hidden, m.order), (Some("Left"), true, 1));
        s.update(&json!({"monitors": {"A": {"customName": null}}})).unwrap();
        assert_eq!(s.get().monitors["A"].custom_name, None);
    }

    #[test]
    fn concurrent_edits_all_land_in_memory_and_on_disk() {
        let dir = tmp("concurrent");
        let p = dir.join("settings.json");
        let (s, _) = SettingsStore::load(p.clone());
        let s = std::sync::Arc::new(s);
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let s = s.clone();
                std::thread::spawn(move || {
                    for n in 0..5 {
                        s.update(&json!({"monitors": {format!("m{i}"): {"order": n}}})).unwrap();
                        let _ = s.get(); // readers run alongside the writers
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let (on_disk, _) = SettingsStore::load(p);
        assert_eq!(s.get(), on_disk.get(), "disk and memory disagree");
        assert!((0..8).all(|i| s.get().monitors[&format!("m{i}")].order == 4));
    }

    #[test]
    fn monitor_color_and_icon_are_validated() {
        let dir = tmp("monitor-look");
        let (s, _) = SettingsStore::load(dir.join("settings.json"));
        s.update(&json!({"monitors": {"A": {"color": "#0067c0", "icon": "game"}}})).unwrap();
        let m = s.get().monitors["A"].clone();
        assert_eq!((m.color.as_deref(), m.icon.as_deref()), (Some("#0067c0"), Some("game")));
        s.update(&json!({"monitors": {"A": {"color": "red; x", "icon": "a b"}}})).unwrap();
        let m = s.get().monitors["A"].clone();
        assert_eq!((m.color, m.icon), (None, None));
    }

    #[test]
    fn invalid_values_are_sanitized_or_rejected() {
        let dir = tmp("sanitize");
        let (s, _) = SettingsStore::load(dir.join("settings.json"));
        let r = s.update(&json!({"general": {"step": 99, "theme": "neon"}})).unwrap();
        assert_eq!((r.general.step, r.general.theme.as_str()), (20, "system"));
        assert_eq!(s.get().general.language, "auto", "language follows the system by default");
        let r = s.update(&json!({"general": {"language": "de"}})).unwrap();
        assert_eq!(r.general.language, "de");
        let r = s.update(&json!({"general": {"language": "klingon"}})).unwrap();
        assert_eq!(r.general.language, "auto");
        assert_eq!(r.general.tray_icon, "contrast");
        let r = s.update(&json!({"general": {"trayIcon": "sun"}})).unwrap();
        assert_eq!(r.general.tray_icon, "sun");
        let r = s.update(&json!({"general": {"trayIcon": "../evil"}})).unwrap();
        assert_eq!(r.general.tray_icon, "contrast");
        assert!(s.update(&json!({"general": {"autostart": "yes"}})).is_err());
        assert!(s.get().general.autostart, "failed patch must not change anything");
    }

    #[test]
    fn migrate_ends_at_the_latest_version() {
        for v in 0..SCHEMA_VERSION {
            let mut doc = json!({ "schemaVersion": v });
            migrate(&mut doc);
            assert_eq!(doc["schemaVersion"], SCHEMA_VERSION, "from v{v}");
        }
    }

    #[test]
    fn migrates_v0_document() {
        let dir = tmp("migrate");
        let p = dir.join("settings.json");
        fs::write(&p, r#"{"autostart": false, "step": 4, "monitors": {"X": {"hidden": true}}}"#).unwrap();
        let (s, out) = SettingsStore::load(p);
        assert_eq!(out, LoadOutcome::Loaded);
        let g = s.get();
        assert_eq!(g.schema_version, SCHEMA_VERSION);
        assert!(!g.general.autostart);
        assert_eq!(g.general.step, 4);
        assert!(g.monitors["X"].hidden);
    }

    #[test]
    fn schedule_defaults_patch_and_sanitize() {
        let dir = tmp("schedule");
        let p = dir.join("settings.json");
        fs::write(&p, r#"{"schemaVersion": 3}"#).unwrap();
        let (s, _) = SettingsStore::load(p);
        let g = s.get();
        assert_eq!(g.schema_version, 4);
        assert!(!g.schedule.enabled && g.schedule.points.is_empty() && g.schedule.transition_min == 10);
        let r = s
            .update(&json!({"schedule": {
                "enabled": true, "transitionMin": 500, "pauseOnManual": "weird", "pauseMinutes": 0,
                "location": {"lat": 120.0, "lon": -200.0, "label": "x"},
                "points": [
                    {"id": "a", "trigger": {"type": "time", "at": "25:99"}, "action": {"type": "brightness", "value": 200, "monitors": null}},
                    {"id": "b", "trigger": {"type": "sun", "event": "noon", "offsetMin": 9999}, "action": {"type": "profile", "profileId": "tpl-night"}}
                ]
            }}))
            .unwrap();
        let sc = r.schedule;
        assert!(sc.enabled);
        assert_eq!((sc.transition_min, sc.pause_minutes, sc.pause_on_manual.as_str()), (60, 1, "untilNext"));
        let l = sc.location.unwrap();
        assert_eq!((l.lat, l.lon), (90.0, -180.0));
        assert_eq!(sc.points[0].trigger, Trigger::Time { at: "00:00".into() });
        assert_eq!(sc.points[0].action, ScheduleAction::Brightness { value: 100, monitors: None });
        assert_eq!(sc.points[1].trigger, Trigger::Sun { event: "sunrise".into(), offset_min: 720 });
    }

    #[test]
    fn export_import_reset_roundtrip() {
        let dir = tmp("exportimport");
        let (a, _) = SettingsStore::load(dir.join("a.json"));
        a.update(&json!({"general": {"step": 7, "language": "en"}, "ddc": {"writeDelayMs": 120}})).unwrap();
        let exported = a.export();

        let (b, _) = SettingsStore::load(dir.join("b.json"));
        assert_eq!(b.get().general.step, 2);
        let imported = b.import(&exported).unwrap();
        assert_eq!(
            (imported.general.step, imported.general.language.as_str(), imported.ddc.write_delay_ms),
            (7, "en", 120)
        );
        // persisted, not just in memory
        let (b2, _) = SettingsStore::load(dir.join("b.json"));
        assert_eq!(b2.get().general.step, 7);

        assert!(b.import("not json").is_err());
        assert!(b.import("[1, 2]").is_err());
        assert_eq!(b.get().general.step, 7, "a bad import changes nothing");

        let r = b.reset().unwrap();
        assert_eq!(r, Settings::default());
    }

    #[test]
    fn ddc_timing_is_clamped() {
        let dir = tmp("ddc");
        let (s, _) = SettingsStore::load(dir.join("settings.json"));
        let r = s.update(&json!({"ddc": {"writeDelayMs": 1, "readDelayMs": 9999, "retries": 40}})).unwrap();
        assert_eq!((r.ddc.write_delay_ms, r.ddc.read_delay_ms, r.ddc.retries), (20, 500, 5));
        assert_eq!(Ddc::default(), Ddc { write_delay_ms: 50, read_delay_ms: 40, retries: 2 });
    }

    #[test]
    fn stock_hotkeys_and_osd_defaults() {
        let dir = tmp("hotkeys");
        let p = dir.join("settings.json");
        fs::write(&p, r#"{"schemaVersion": 2}"#).unwrap();
        let (s, _) = SettingsStore::load(p);
        let g = s.get();
        assert_eq!(g.schema_version, SCHEMA_VERSION);
        assert_eq!(g.hotkeys, default_hotkeys());
        assert!(!g.hotkeys_enabled, "global hotkeys are opt-in");
        assert!(g.osd.enabled && g.osd.duration_ms == 1500 && g.osd.position == "bottom");
    }

    #[test]
    fn hotkey_patch_roundtrip_and_sanitize() {
        let dir = tmp("hk2");
        let (s, _) = SettingsStore::load(dir.join("settings.json"));
        let r = s
            .update(&json!({
                "hotkeys": [
                    {"accelerator": " Ctrl+F1 ", "action": {"type": "brightness", "monitorId": "A", "delta": -5}},
                    {"accelerator": "", "action": {"type": "profile", "profileId": "tpl-night"}}
                ],
                "osd": {"durationMs": 10, "position": "left"}
            }))
            .unwrap();
        assert_eq!(r.hotkeys[0].accelerator, "Ctrl+F1");
        assert_eq!(r.hotkeys[0].action, HotkeyAction::Brightness { monitor_id: "A".into(), delta: -1 });
        assert_eq!(r.hotkeys[1].action, HotkeyAction::Profile { profile_id: "tpl-night".into() });
        assert_eq!((r.osd.duration_ms, r.osd.position.as_str()), (500, "bottom"));
        // arrays replace whole: an empty list means "no hotkeys", not "defaults"
        let r = s.update(&json!({"hotkeys": []})).unwrap();
        assert!(r.hotkeys.is_empty());
    }

    #[test]
    fn v1_document_gets_v2_defaults() {
        let dir = tmp("v1");
        let p = dir.join("settings.json");
        fs::write(&p, r#"{"schemaVersion": 1, "general": {"step": 3}, "monitors": {"X": {"order": 2}}}"#).unwrap();
        let (s, _) = SettingsStore::load(p);
        let g = s.get();
        assert_eq!(g.schema_version, SCHEMA_VERSION);
        assert_eq!(g.general.step, 3);
        assert!(!g.linked.enabled && g.profiles.is_empty() && !g.templates_seeded);
        assert_eq!(g.monitors["X"].order, 2);
        assert!(g.monitors["X"].calibration.is_empty());
    }

    #[test]
    fn calibration_and_profiles_are_sanitized() {
        let dir = tmp("calib");
        let (s, _) = SettingsStore::load(dir.join("settings.json"));
        let r = s
            .update(&json!({
                "monitors": {"A": {"calibration": {"brightness": {"min": 90, "max": 250}}}},
                "profiles": [{"id": "p1", "name": "Night", "values": {"A": {"brightness": 200}}}]
            }))
            .unwrap();
        let c = r.monitors["A"].calibration["brightness"];
        assert_eq!((c.min, c.max), (90, 100));
        assert_eq!(r.profiles[0].values["A"]["brightness"], 100);
        assert_eq!(r.profiles[0].icon, "sun");
    }

    #[test]
    fn oversized_text_and_lists_are_cut_and_big_files_refused() {
        let dir = tmp("limits");
        let (s, _) = SettingsStore::load(dir.join("settings.json"));
        let long = "я".repeat(500);
        let many: Vec<_> = (0..300).map(|i| json!({"id": format!("p{i}"), "name": long, "icon": "../x"})).collect();
        let r = s.update(&json!({"monitors": {"A": {"customName": long}}, "profiles": many})).unwrap();
        assert_eq!(r.monitors["A"].custom_name.as_deref().unwrap().chars().count(), MAX_NAME_CHARS);
        assert_eq!(r.profiles.len(), MAX_PROFILES);
        assert_eq!(r.profiles[0].name.chars().count(), MAX_NAME_CHARS);
        assert_eq!(r.profiles[0].icon, "sun", "an icon key that is not a plain word is replaced");

        let huge = " ".repeat(MAX_DOCUMENT_BYTES + 1);
        assert!(s.import(&huge).is_err());
        // an oversized file on disk is quarantined like a corrupt one
        let p = dir.join("big.json");
        fs::write(&p, &huge).unwrap();
        let (s2, out) = SettingsStore::load(p);
        assert!(matches!(out, LoadOutcome::Recovered(_)));
        assert_eq!(s2.get(), Settings::default());
    }

    #[test]
    fn concurrent_atomic_writes_never_publish_a_mix() {
        let dir = tmp("atomic");
        let path = dir.join("shared.json");
        let writers: Vec<_> = (0..8u8)
            .map(|n| {
                let path = path.clone();
                std::thread::spawn(move || {
                    for _ in 0..50 {
                        write_atomic(&path, &vec![b'a' + n; 4096]).unwrap();
                    }
                })
            })
            .collect();
        for w in writers {
            w.join().unwrap();
        }
        let bytes = fs::read(&path).unwrap();
        assert_eq!(bytes.len(), 4096);
        assert!(bytes.iter().all(|&b| b == bytes[0]), "file mixes the output of two writers");
    }

    #[test]
    fn corrupt_file_is_quarantined() {
        let dir = tmp("broken");
        let p = dir.join("settings.json");
        fs::write(&p, "{ not json").unwrap();
        let (s, out) = SettingsStore::load(p.clone());
        let LoadOutcome::Recovered(backup) = out else { panic!("expected recovery") };
        assert!(backup.exists());
        assert!(backup.file_name().unwrap().to_string_lossy().starts_with("settings.broken-"));
        assert_eq!(s.get(), Settings::default());
        assert!(p.exists(), "fresh defaults are written back");
    }

    #[test]
    fn cache_roundtrip_and_corruption() {
        let dir = tmp("cache");
        let p = dir.join("cache.json");
        assert_eq!(Cache::load(&p), Cache::default());
        let c = Cache {
            monitors: vec![CachedMonitor {
                id: "A".into(),
                number: 1,
                name: "LG".into(),
                features: vec![CachedFeature { feature: "brightness".into(), max: 100, percent: 65 }],
            }],
        };
        c.save(&p).unwrap();
        assert_eq!(Cache::load(&p), c);
        fs::write(&p, "garbage").unwrap();
        assert_eq!(Cache::load(&p), Cache::default());
    }
}
