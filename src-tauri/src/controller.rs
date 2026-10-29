//! The single place that owns monitor state, calibration and write queues (spec §7).
//!
//! Tray, hotkeys, schedule and the UI never touch monitors directly; they call
//! into [`Controller`]. Every monitor has its own worker thread, so slow DDC/CI
//! on one display never blocks another or the UI.

use std::collections::{BTreeMap, HashMap};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};

use lumza_core::link;
use lumza_core::scale::percent_to_raw;
use lumza_core::store::{Cache, CachedFeature, CachedMonitor, Profile, Range, Settings, SettingsStore};
use lumza_core::{DisplayMode, DisplayModes, Feature, HdrState, MonitorInfo, MonitorPort, PortSource, VcpValue};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::display_modes::{Busy, DisplayModesDto};

type Probe = Vec<(Feature, Option<VcpValue>)>;

enum Event {
    /// Result of the first read after a scan: which features answer at all. The flag is false when a
    /// write was queued meanwhile, so the values read may already be out of date.
    Probed(Probe, bool),
    Value(Feature, VcpValue),
    WriteFailed(Feature),
}

type Callback = Arc<dyn Fn(Event) + Send + Sync>;

// ---------------------------------------------------------------- worker

#[derive(Default)]
struct Queue {
    /// Coalesced: at most one pending value per feature (spec §3.2).
    writes: Vec<(Feature, u32)>,
    probe: bool,
    read: bool,
    stop: bool,
}

struct Shared {
    q: Mutex<Queue>,
    cv: Condvar,
    /// Counts queued writes. A read that began before a write must not report its value afterwards:
    /// it would put the old value back over what the user just set.
    writes: AtomicU64,
}

struct Worker {
    shared: Arc<Shared>,
}

impl Worker {
    fn spawn(mut port: Box<dyn MonitorPort>, name: String, cb: Callback) -> Self {
        let shared =
            Arc::new(Shared { q: Mutex::new(Queue::default()), cv: Condvar::new(), writes: AtomicU64::new(0) });
        let s = shared.clone();
        std::thread::Builder::new()
            .name(format!("ddc-{name}"))
            .spawn(move || loop {
                let job = {
                    let mut q = lock(&s.q);
                    while q.writes.is_empty() && !q.probe && !q.read && !q.stop {
                        q = s.cv.wait(q).unwrap_or_else(PoisonError::into_inner);
                    }
                    if q.writes.is_empty() && !q.probe && !q.read {
                        return; // stopped and drained
                    }
                    // `stop` stays set: taking the whole queue would clear it, and the worker (with
                    // its monitor handle) would then wait forever for a wake-up that never comes.
                    Queue {
                        writes: std::mem::take(&mut q.writes),
                        probe: std::mem::take(&mut q.probe),
                        read: std::mem::take(&mut q.read),
                        stop: false,
                    }
                };
                // A panic in one monitor's DDC code must not take the app down.
                let r = catch_unwind(AssertUnwindSafe(|| run_job(port.as_mut(), job, &cb, &s)));
                if r.is_err() {
                    tracing::error!(monitor = %name, "DDC worker panicked; continuing");
                }
            })
            .expect("spawn worker");
        Self { shared }
    }

    fn write(&self, f: Feature, raw: u32) {
        let mut q = lock(&self.shared.q);
        match q.writes.iter_mut().find(|(g, _)| *g == f) {
            Some(e) => e.1 = raw,
            None => q.writes.push((f, raw)),
        }
        self.shared.writes.fetch_add(1, Ordering::SeqCst);
        self.shared.cv.notify_one();
    }

    fn probe(&self) {
        lock(&self.shared.q).probe = true;
        self.shared.cv.notify_one();
    }

    fn read(&self) {
        lock(&self.shared.q).read = true;
        self.shared.cv.notify_one();
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        lock(&self.shared.q).stop = true;
        self.shared.cv.notify_one();
    }
}

/// Locks `m`, carrying on with the data if a panicking holder poisoned it: the state here is plain
/// values, and one panic must not turn every later call into another panic in a tray app that runs for weeks.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn run_job(port: &mut dyn MonitorPort, job: Queue, cb: &Callback, shared: &Shared) {
    // Taken after the job left the queue, so its own writes are already counted.
    let seen = shared.writes.load(Ordering::SeqCst);
    let fresh = || shared.writes.load(Ordering::SeqCst) == seen;
    for (f, raw) in job.writes {
        if let Err(e) = port.set_vcp(f.vcp(), raw) {
            tracing::warn!(feature = %f, error = %e, "write failed");
            cb(Event::WriteFailed(f));
            // Roll the slider back to what the monitor actually has.
            if let Ok(v) = port.get_vcp(f.vcp()) {
                cb(Event::Value(f, v));
            }
        }
    }
    if job.probe {
        let res = Feature::ALL.iter().map(|&f| (f, port.get_vcp(f.vcp()).ok())).collect();
        cb(Event::Probed(res, fresh()));
    } else if job.read {
        for f in Feature::ALL {
            // A value read before a newer write is dropped: the write is the truth now.
            if let Ok(v) = port.get_vcp(f.vcp()) {
                if fresh() {
                    cb(Event::Value(f, v));
                }
            }
        }
    }
}

// ------------------------------------------------------------ controller

struct FeatureState {
    feature: Feature,
    max: u32,
    percent: u8,
}

struct Mon {
    info: MonitorInfo,
    features: Vec<FeatureState>,
    probed: bool,
    worker: Worker,
}

#[derive(Default)]
struct State {
    generation: u64,
    scanning: bool,
    pending_probes: usize,
    scan_started: Option<Instant>,
    monitors: Vec<Mon>,
    /// Last known monitors from `cache.json`, shown (read-only) until the first scan lands.
    placeholder: Vec<CachedMonitor>,
    /// HDR state by monitor id, only for monitors Windows could read it for. Not DDC/CI, so
    /// it lives outside `Mon` and the workers.
    hdr: HashMap<String, HdrState>,
    /// Connected displays and the current layout as of the last scan or switch. Not DDC/CI either,
    /// and cached so the tray and the flyout never wait on the source lock.
    displays: Option<DisplayModes>,
}

/// Cache writes are delayed so a slider drag produces one disk write, not hundreds.
const CACHE_SAVE_DELAY: Duration = Duration::from_secs(2);

pub struct Controller {
    app: AppHandle,
    source: Mutex<Box<dyn PortSource>>,
    state: Mutex<State>,
    settings: Arc<SettingsStore>,
    cache_path: PathBuf,
    cache_save_pending: AtomicBool,
    rescanning: AtomicBool,
    rescan_again: AtomicBool,
    /// One display-layout switch at a time.
    switching: Busy,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FeatureDto {
    feature: &'static str,
    percent: u8,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MonitorDto {
    id: String,
    number: u32,
    /// Display name: the user's custom name if set, else the detected one.
    name: String,
    original_name: String,
    /// Badge colour (`#rrggbb`) and icon key chosen by the user, to tell monitors apart.
    color: Option<String>,
    icon: Option<String>,
    features: Vec<FeatureDto>,
    /// True once the probe finished; a probed monitor with no features has no DDC/CI.
    probed: bool,
    /// Values come from the disk cache; the monitor has not answered yet, so it is read-only.
    stale: bool,
    hidden: bool,
    hidden_features: Vec<String>,
    excluded_from_link: bool,
    /// Calibration ranges for linked sliders, only for features the user changed.
    calibration: Vec<CalibrationDto>,
    /// Present only when the monitor supports HDR.
    hdr: Option<HdrDto>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct HdrDto {
    enabled: bool,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CalibrationDto {
    feature: String,
    min: u8,
    max: u8,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    monitors: Vec<MonitorDto>,
    scanning: bool,
}

/// A monitor as the scheduler sees it: id and (feature, current percent).
pub struct SchedMonitor {
    pub id: String,
    pub features: Vec<(Feature, u8)>,
}

/// Outcome of a relative change, for the OSD.
pub struct AdjustResult {
    pub feature: Feature,
    pub label: String,
    pub percent: u8,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ValueChanged {
    id: String,
    feature: &'static str,
    percent: u8,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct WriteFailed {
    id: String,
    feature: &'static str,
}

impl Controller {
    pub fn new(
        app: AppHandle,
        source: Box<dyn PortSource>,
        settings: Arc<SettingsStore>,
        cache_path: PathBuf,
    ) -> Arc<Self> {
        let state = State { placeholder: Cache::load(&cache_path).monitors, scanning: true, ..State::default() };
        Arc::new(Self {
            app,
            source: Mutex::new(source),
            state: Mutex::new(state),
            settings,
            cache_path,
            cache_save_pending: AtomicBool::new(false),
            rescanning: AtomicBool::new(false),
            rescan_again: AtomicBool::new(false),
            switching: Busy::new(),
        })
    }

    pub fn snapshot(&self) -> Snapshot {
        let st = lock(&self.state);
        self.settings.read(|s| snapshot_of(&st, s))
    }

    /// Settings changed (names, order, visibility): push a fresh snapshot to the windows.
    pub fn settings_changed(&self) {
        let _ = self.app.emit("monitors-changed", self.snapshot());
    }

    fn schedule_cache_save(self: &Arc<Self>) {
        if self.cache_save_pending.swap(true, Ordering::SeqCst) {
            return;
        }
        let this = self.clone();
        std::thread::spawn(move || {
            std::thread::sleep(CACHE_SAVE_DELAY);
            this.cache_save_pending.store(false, Ordering::SeqCst);
            let cache = {
                let st = lock(&this.state);
                Cache {
                    monitors: st
                        .monitors
                        .iter()
                        .filter(|m| !m.features.is_empty())
                        .map(|m| CachedMonitor {
                            id: m.info.id.clone(),
                            number: m.info.number,
                            name: m.info.name.clone(),
                            features: m
                                .features
                                .iter()
                                .map(|f| CachedFeature {
                                    feature: f.feature.name().into(),
                                    max: f.max,
                                    percent: f.percent,
                                })
                                .collect(),
                        })
                        .collect(),
                }
            };
            if let Err(e) = cache.save(&this.cache_path) {
                tracing::warn!(error = %e, "cache save failed");
            }
        });
    }

    /// Re-enumerate monitors off-thread. Requests arriving mid-scan coalesce into one more pass.
    pub fn rescan(self: &Arc<Self>) {
        if self.rescanning.swap(true, Ordering::SeqCst) {
            self.rescan_again.store(true, Ordering::SeqCst);
            return;
        }
        let this = self.clone();
        std::thread::Builder::new()
            .name("scan".into())
            .spawn(move || {
                // A panic inside a scan must not leave `rescanning` set, which would turn every
                // later rescan request into a silent no-op.
                struct OnPanic<'a>(&'a AtomicBool);
                impl Drop for OnPanic<'_> {
                    fn drop(&mut self) {
                        if std::thread::panicking() {
                            self.0.store(false, Ordering::SeqCst);
                        }
                    }
                }
                let _on_panic = OnPanic(&this.rescanning);
                loop {
                    this.rescan_again.store(false, Ordering::SeqCst);
                    this.do_scan();
                    if this.rescan_again.load(Ordering::SeqCst) {
                        continue;
                    }
                    this.rescanning.store(false, Ordering::SeqCst);
                    // A request that slipped in between the check and the store would be lost:
                    // take the flag back and run once more.
                    if this.rescan_again.load(Ordering::SeqCst) && !this.rescanning.swap(true, Ordering::SeqCst) {
                        continue;
                    }
                    break;
                }
            })
            .expect("spawn scan");
    }

    fn do_scan(self: &Arc<Self>) {
        let started = Instant::now();
        let (scanned, displays) = {
            let ddc = self.settings.get().ddc;
            let mut source = lock(&self.source);
            source.set_timing(ddc.write_delay_ms, ddc.read_delay_ms, ddc.retries);
            (source.scan(), source.display_modes())
        };
        let ports = match scanned {
            Ok(p) => p,
            Err(e) => {
                tracing::error!(error = %e, "scan failed");
                Vec::new()
            }
        };
        // Even when no monitor answers DDC/CI: a layout that left no display is still a layout.
        self.publish_display_modes(displays);
        let mut st = lock(&self.state);
        st.generation += 1;
        let generation = st.generation;
        st.scan_started = Some(started);
        st.scanning = !ports.is_empty();
        st.pending_probes = ports.len();
        let cached: HashMap<String, CachedMonitor> =
            std::mem::take(&mut st.placeholder).into_iter().map(|m| (m.id.clone(), m)).collect();
        st.monitors = ports
            .into_iter()
            .map(|(info, port)| {
                let weak: Weak<Controller> = Arc::downgrade(self);
                let id = info.id.clone();
                let cb: Callback = Arc::new(move |ev| {
                    if let Some(c) = weak.upgrade() {
                        c.on_event(generation, &id, ev);
                    }
                });
                let worker = Worker::spawn(port, info.name.clone(), cb);
                worker.probe();
                let features = cached
                    .get(&info.id)
                    .map(|c| {
                        c.features
                            .iter()
                            .filter_map(|f| {
                                Some(FeatureState { feature: f.feature.parse().ok()?, max: f.max, percent: f.percent })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                Mon { info, features, probed: false, worker }
            })
            .collect();
        tracing::info!(monitors = st.monitors.len(), ms = started.elapsed().as_millis() as u64, "enumerated");
        let snap = self.settings.read(|s| snapshot_of(&st, s));
        drop(st);
        let _ = self.app.emit("monitors-changed", snap);
        self.refresh_hdr(false);
    }

    fn on_event(self: &Arc<Self>, generation: u64, id: &str, ev: Event) {
        let mut st = lock(&self.state);
        if st.generation != generation {
            return; // event from a worker of a previous scan
        }
        let started = st.scan_started;
        let Some(idx) = st.monitors.iter().position(|m| m.info.id == id) else { return };
        match ev {
            Event::Probed(res, fresh) => {
                let m = &mut st.monitors[idx];
                let old = std::mem::take(&mut m.features);
                m.features = res
                    .into_iter()
                    .filter_map(|(f, v)| {
                        let v = v?;
                        // Written to while probing: keep what the user set, take only the range.
                        let kept = if fresh { None } else { old.iter().find(|o| o.feature == f).map(|o| o.percent) };
                        Some(FeatureState { feature: f, max: v.max, percent: kept.unwrap_or_else(|| v.percent()) })
                    })
                    .collect();
                m.probed = true;
                st.pending_probes = st.pending_probes.saturating_sub(1);
                if st.pending_probes == 0 {
                    st.scanning = false;
                    if let Some(t) = started {
                        tracing::info!(ms = t.elapsed().as_millis() as u64, "all monitors probed");
                    }
                }
                let all_probed = !st.scanning;
                let snap = self.settings.read(|s| snapshot_of(&st, s));
                drop(st);
                let _ = self.app.emit("monitors-changed", snap);
                self.schedule_cache_save();
                // Only once every monitor answered, so the templates cover all of them.
                if all_probed {
                    self.seed_templates();
                    // Lets the schedule apply the value that is right for this moment.
                    let _ = self.app.emit("scan-finished", ());
                }
            }
            Event::Value(f, v) => {
                let m = &mut st.monitors[idx];
                if let Some(fs) = m.features.iter_mut().find(|x| x.feature == f) {
                    fs.max = v.max;
                    fs.percent = v.percent();
                    let payload = ValueChanged { id: id.to_string(), feature: f.name(), percent: fs.percent };
                    drop(st);
                    let _ = self.app.emit("values-changed", payload);
                    self.schedule_cache_save();
                }
            }
            Event::WriteFailed(f) => {
                drop(st);
                let _ = self.app.emit("write-failed", WriteFailed { id: id.to_string(), feature: f.name() });
            }
        }
    }

    /// Re-read current values in the background (monitor buttons may have changed them).
    pub fn reload_values(self: &Arc<Self>) {
        {
            let st = lock(&self.state);
            for m in st.monitors.iter().filter(|m| m.probed) {
                m.worker.read();
            }
        }
        // HDR may have been toggled in Windows (Win+Alt+B); DisplayConfig stays off the UI thread.
        let this = self.clone();
        std::thread::Builder::new().name("hdr".into()).spawn(move || this.refresh_hdr(false)).expect("spawn hdr");
    }

    /// Re-read every monitor's HDR state from Windows and push a snapshot when it changed.
    /// `force` pushes it anyway, so a switch whose change was refused snaps back.
    fn refresh_hdr(&self, force: bool) {
        let devices: Vec<(String, String)> = {
            let st = lock(&self.state);
            st.monitors.iter().map(|m| (m.info.id.clone(), m.info.device.clone())).collect()
        };
        let results: Vec<(String, Option<HdrState>)> = {
            let mut source = lock(&self.source);
            devices.into_iter().map(|(id, device)| (id, source.hdr_state(&device))).collect()
        };
        let mut st = lock(&self.state);
        let changed = merge_hdr(&mut st.hdr, results);
        if changed || force {
            let snap = self.settings.read(|s| snapshot_of(&st, s));
            drop(st);
            let _ = self.app.emit("monitors-changed", snap);
        }
    }

    /// Switch HDR on or off for one monitor. Unknown monitors are ignored (hot-unplug).
    /// A refused change raises `hdr-failed`; either way the real state is pushed back.
    pub fn set_hdr(self: &Arc<Self>, id: &str, on: bool) {
        let Some(device) = lock(&self.state).monitors.iter().find(|m| m.info.id == id).map(|m| m.info.device.clone())
        else {
            return;
        };
        let this = self.clone();
        std::thread::Builder::new()
            .name("hdr".into())
            .spawn(move || {
                let res = lock(&this.source).set_hdr(&device, on);
                if let Err(e) = res {
                    tracing::warn!(error = %e, "HDR change failed");
                    let _ = this.app.emit("hdr-failed", ());
                }
                this.refresh_hdr(true);
            })
            .expect("spawn hdr");
    }

    /// Displays as of the last scan or switch; `None` when Windows could not be asked.
    pub fn display_modes_raw(&self) -> Option<DisplayModes> {
        lock(&self.state).displays.clone()
    }

    pub fn display_modes(&self) -> Option<DisplayModesDto> {
        self.display_modes_raw().map(Into::into)
    }

    /// Store `new` and tell the windows, but only when it differs from what they already have.
    fn publish_display_modes(&self, new: Option<DisplayModes>) {
        {
            let mut st = lock(&self.state);
            if st.displays == new {
                return;
            }
            st.displays = new.clone();
        }
        let _ = self.app.emit("display-modes-changed", new.map(DisplayModesDto::from));
    }

    /// Switch the display layout. Runs on a one-shot thread: Windows can take seconds to
    /// reconfigure the GPU outputs. A second request while one is running is ignored. On success
    /// the rescan (which `WM_DISPLAYCHANGE` would also trigger) refreshes monitors and layout;
    /// a refusal raises `display-mode-failed` and pushes the real layout back.
    pub fn set_display_mode(self: &Arc<Self>, mode: DisplayMode) {
        let this = self.clone();
        std::thread::Builder::new()
            .name("display-mode".into())
            .spawn(move || {
                let Some(_busy) = this.switching.try_acquire() else { return };
                let res = lock(&this.source).set_display_mode(&mode);
                match res {
                    Ok(()) => this.rescan(),
                    Err(e) => {
                        tracing::warn!(error = %e, "display mode change failed");
                        let _ = this.app.emit("display-mode-failed", ());
                        let real = lock(&this.source).display_modes();
                        this.publish_display_modes(real);
                    }
                }
            })
            .expect("spawn display-mode");
    }

    /// Set one feature of one monitor (0–100 %). Unknown monitors are ignored: a
    /// hot-unplug during a drag must not surface as an error (spec §9).
    pub fn set_value(self: &Arc<Self>, id: &str, feature: Feature, percent: u8) {
        let applied = queue_write(&mut lock(&self.state), id, feature, percent);
        if applied.is_some() {
            self.schedule_cache_save();
        }
    }

    /// Linked sliders: one common value for every participating monitor, each mapped
    /// through its own calibration range. Emits `values-changed` per monitor so the
    /// individual sliders follow.
    pub fn set_linked_value(self: &Arc<Self>, feature: Feature, percent: u8) {
        let settings = self.settings.get();
        let mut changed = Vec::new();
        {
            let mut st = lock(&self.state);
            let ids: Vec<String> = st
                .monitors
                .iter()
                .filter(|m| {
                    !settings.monitors.get(&m.info.id).is_some_and(|s| s.excluded_from_link)
                        && m.features.iter().any(|f| f.feature == feature)
                })
                .map(|m| m.info.id.clone())
                .collect();
            let parts: Vec<(&str, Range)> =
                ids.iter().map(|id| (id.as_str(), range_of(&settings, id, feature))).collect();
            for (id, p) in link::targets(percent.min(100), &parts) {
                if let Some(applied) = queue_write(&mut st, id, feature, p) {
                    changed.push((id.to_string(), applied));
                }
            }
        }
        for (id, applied) in changed {
            let _ = self.app.emit("values-changed", ValueChanged { id, feature: feature.name(), percent: applied });
        }
        self.schedule_cache_save();
    }

    /// Apply a profile. Missing monitors or features are skipped, never touched (spec §4.2).
    pub fn apply_profile(self: &Arc<Self>, profile_id: &str) -> bool {
        let Some(profile) = self.settings.get().profiles.into_iter().find(|p| p.id == profile_id) else {
            return false;
        };
        let mut changed = Vec::new();
        {
            let mut st = lock(&self.state);
            for (id, feats) in &profile.values {
                for (name, value) in feats {
                    let Ok(f) = name.parse::<Feature>() else { continue };
                    if let Some(applied) = queue_write(&mut st, id, f, *value) {
                        changed.push((id.clone(), f, applied));
                    }
                }
            }
        }
        for (id, f, applied) in changed {
            let _ = self.app.emit("values-changed", ValueChanged { id, feature: f.name(), percent: applied });
        }
        self.schedule_cache_save();
        true
    }

    /// Change a feature by `delta` percent (hotkeys). `target` is one monitor id, or `None` for
    /// all monitors that have the feature. Each monitor moves relative to its own value and
    /// is clamped to 0–100. Returns what the OSD should show.
    pub fn adjust(self: &Arc<Self>, target: Option<&str>, feature: Feature, delta: i32) -> Option<AdjustResult> {
        let settings = self.settings.get();
        let mut changed: Vec<(String, u8)> = Vec::new();
        let label;
        {
            let mut st = lock(&self.state);
            let ids: Vec<String> = st
                .monitors
                .iter()
                .filter(|m| target.is_none_or(|t| t == m.info.id) && m.features.iter().any(|f| f.feature == feature))
                .map(|m| m.info.id.clone())
                .collect();
            label = match target {
                Some(t) => {
                    let m = st.monitors.iter().find(|m| m.info.id == t)?;
                    settings
                        .monitors
                        .get(t)
                        .and_then(|s| s.custom_name.clone())
                        .filter(|n| !n.trim().is_empty())
                        .unwrap_or_else(|| m.info.name.clone())
                }
                None => crate::i18n::t(&settings.general.language, "be.allMonitors"),
            };
            for id in ids {
                let cur = st
                    .monitors
                    .iter()
                    .find(|m| m.info.id == id)
                    .and_then(|m| m.features.iter().find(|f| f.feature == feature))
                    .map(|f| f.percent as i32)?;
                let next = (cur + delta).clamp(0, 100) as u8;
                if let Some(applied) = queue_write(&mut st, &id, feature, next) {
                    changed.push((id, applied));
                }
            }
        }
        if changed.is_empty() {
            return None;
        }
        let mean = (changed.iter().map(|(_, p)| *p as u32).sum::<u32>() as f32 / changed.len() as f32).round() as u8;
        for (id, percent) in changed {
            let _ = self.app.emit("values-changed", ValueChanged { id, feature: feature.name(), percent });
        }
        self.schedule_cache_save();
        Some(AdjustResult { feature, label, percent: mean })
    }

    /// Connected, answering monitors and the features they have, for the scheduler.
    pub fn schedule_monitors(&self) -> Vec<SchedMonitor> {
        let st = lock(&self.state);
        st.monitors
            .iter()
            .filter(|m| m.probed)
            .map(|m| SchedMonitor {
                id: m.info.id.clone(),
                features: m.features.iter().map(|f| (f.feature, f.percent)).collect(),
            })
            .collect()
    }

    /// Apply values prescribed by the schedule. Monitors/features that are gone are skipped and
    /// values already in place cause no write (EEPROM wear, spec §12).
    pub fn apply_scheduled(self: &Arc<Self>, values: &[(String, Feature, u8)]) -> usize {
        let mut changed = Vec::new();
        {
            let mut st = lock(&self.state);
            for (id, f, v) in values {
                let same = st
                    .monitors
                    .iter()
                    .find(|m| &m.info.id == id)
                    .and_then(|m| m.features.iter().find(|x| x.feature == *f))
                    .map(|x| x.percent == (*v).min(100));
                if same == Some(false) {
                    if let Some(applied) = queue_write(&mut st, id, *f, *v) {
                        changed.push((id.clone(), *f, applied));
                    }
                }
            }
        }
        let n = changed.len();
        for (id, f, applied) in changed {
            let _ = self.app.emit("values-changed", ValueChanged { id, feature: f.name(), percent: applied });
        }
        if n > 0 {
            self.schedule_cache_save();
        }
        n
    }

    /// Current value of every feature of every connected monitor, for "save as profile".
    pub fn current_values(&self) -> BTreeMap<String, BTreeMap<String, u8>> {
        let st = lock(&self.state);
        st.monitors
            .iter()
            .filter(|m| m.probed && !m.features.is_empty())
            .map(|m| {
                (m.info.id.clone(), m.features.iter().map(|f| (f.feature.name().to_string(), f.percent)).collect())
            })
            .collect()
    }

    /// First run: create the built-in Day / Night / Games profiles from the monitors found.
    fn seed_templates(&self) {
        let settings = self.settings.get();
        if settings.templates_seeded {
            return;
        }
        let have_brightness: Vec<String> =
            self.current_values().into_iter().filter(|(_, f)| f.contains_key("brightness")).map(|(id, _)| id).collect();
        if have_brightness.is_empty() {
            return; // nothing to build them from yet; try again after the next scan
        }
        let mk = |id: &str, name: &str, icon: &str, brightness: u8| Profile {
            id: id.into(),
            name: name.into(),
            icon: icon.into(),
            values: have_brightness
                .iter()
                .map(|m| (m.clone(), BTreeMap::from([("brightness".to_string(), brightness)])))
                .collect(),
        };
        let mut profiles = settings.profiles;
        profiles.push(mk("tpl-day", "Day", "sun", 80));
        profiles.push(mk("tpl-night", "Night", "moon", 25));
        profiles.push(mk("tpl-games", "Games", "game", 100));
        match self.settings.update(&serde_json::json!({ "profiles": profiles, "templatesSeeded": true })) {
            Ok(new) => {
                let _ = self.app.emit("settings-changed", &new);
            }
            Err(e) => tracing::warn!(error = %e, "seeding profile templates failed"),
        }
    }
}

/// Replace the HDR map with fresh readings (monitors that gave none are dropped).
/// Returns whether anything changed.
fn merge_hdr(map: &mut HashMap<String, HdrState>, results: Vec<(String, Option<HdrState>)>) -> bool {
    let fresh: HashMap<String, HdrState> = results.into_iter().filter_map(|(id, s)| Some((id, s?))).collect();
    let changed = *map != fresh;
    *map = fresh;
    changed
}

fn range_of(settings: &Settings, id: &str, f: Feature) -> Range {
    settings.monitors.get(id).and_then(|m| m.calibration.get(f.name())).copied().unwrap_or_default()
}

/// Update state and queue the DDC write. Returns the percent actually applied.
fn queue_write(st: &mut State, id: &str, feature: Feature, percent: u8) -> Option<u8> {
    let m = st.monitors.iter_mut().find(|m| m.info.id == id)?;
    let fs = m.features.iter_mut().find(|x| x.feature == feature)?;
    fs.percent = percent.min(100);
    m.worker.write(feature, percent_to_raw(fs.percent, fs.max));
    Some(fs.percent)
}

fn snapshot_of(st: &State, settings: &Settings) -> Snapshot {
    let dto = |id: &str, number: u32, name: &str, features: Vec<FeatureDto>, probed: bool, stale: bool| {
        let ms = settings.monitors.get(id);
        MonitorDto {
            id: id.to_string(),
            number,
            name: ms
                .and_then(|m| m.custom_name.clone())
                .filter(|n| !n.trim().is_empty())
                .unwrap_or_else(|| name.to_string()),
            original_name: name.to_string(),
            color: ms.and_then(|m| m.color.clone()),
            icon: ms.and_then(|m| m.icon.clone()),
            features,
            probed,
            stale,
            hidden: ms.is_some_and(|m| m.hidden),
            hidden_features: ms.map(|m| m.hidden_features.clone()).unwrap_or_default(),
            excluded_from_link: ms.is_some_and(|m| m.excluded_from_link),
            calibration: ms
                .map(|m| {
                    m.calibration
                        .iter()
                        .map(|(f, r)| CalibrationDto { feature: f.clone(), min: r.min, max: r.max })
                        .collect()
                })
                .unwrap_or_default(),
            hdr: st.hdr.get(id).filter(|h| h.supported).map(|h| HdrDto { enabled: h.enabled }),
        }
    };
    let mut monitors: Vec<MonitorDto> = if st.monitors.is_empty() && !st.placeholder.is_empty() {
        st.placeholder
            .iter()
            .map(|c| {
                let f = c
                    .features
                    .iter()
                    .filter_map(|f| {
                        Some(FeatureDto { feature: f.feature.parse::<Feature>().ok()?.name(), percent: f.percent })
                    })
                    .collect();
                dto(&c.id, c.number, &c.name, f, false, true)
            })
            .collect()
    } else {
        st.monitors
            .iter()
            .map(|m| {
                let f =
                    m.features.iter().map(|f| FeatureDto { feature: f.feature.name(), percent: f.percent }).collect();
                dto(&m.info.id, m.info.number, &m.info.name, f, m.probed, !m.probed && !m.features.is_empty())
            })
            .collect()
    };
    monitors.sort_by_key(|m| (settings.monitors.get(&m.id).map_or(0, |s| s.order), m.number));
    Snapshot { scanning: st.scanning, monitors }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A port whose first write blocks until released, and which reports when it is dropped
    /// (a real port drops its monitor handle there).
    struct GatedPort {
        entered: std::sync::mpsc::Sender<()>,
        gate: Arc<(Mutex<bool>, Condvar)>,
        dropped: Arc<AtomicBool>,
    }

    impl MonitorPort for GatedPort {
        fn capabilities_string(&mut self) -> lumza_core::Result<String> {
            Ok(String::new())
        }
        fn get_vcp(&mut self, _: u8) -> lumza_core::Result<VcpValue> {
            Ok(VcpValue { current: 1, max: 100 })
        }
        fn set_vcp(&mut self, _: u8, _: u32) -> lumza_core::Result<()> {
            let _ = self.entered.send(());
            let (open, cv) = &*self.gate;
            let mut open = open.lock().unwrap();
            while !*open {
                open = cv.wait(open).unwrap();
            }
            Ok(())
        }
    }

    impl Drop for GatedPort {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::SeqCst);
        }
    }

    #[test]
    fn worker_stopped_with_jobs_pending_still_exits_and_releases_its_port() {
        let (entered_tx, entered) = std::sync::mpsc::channel();
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let dropped = Arc::new(AtomicBool::new(false));
        let port = GatedPort { entered: entered_tx, gate: gate.clone(), dropped: dropped.clone() };
        let worker = Worker::spawn(Box::new(port), "test".into(), Arc::new(|_| {}));

        worker.write(Feature::Brightness, 10); // the worker is now stuck inside this write
        entered.recv_timeout(Duration::from_secs(5)).expect("worker started the first write");
        worker.write(Feature::Contrast, 20); // pending when the worker is dropped (a rescan)
        drop(worker);

        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !dropped.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(dropped.load(Ordering::SeqCst), "the worker thread leaked its port");
    }

    /// One shared register. The first read reports it, then blocks until released: a read that
    /// started before a later write and finishes after it.
    struct SlowReadPort {
        register: Arc<std::sync::atomic::AtomicU32>,
        entered: std::sync::mpsc::Sender<()>,
        gate: Arc<(Mutex<bool>, Condvar)>,
        blocked: AtomicBool,
    }

    impl MonitorPort for SlowReadPort {
        fn capabilities_string(&mut self) -> lumza_core::Result<String> {
            Ok(String::new())
        }
        fn get_vcp(&mut self, _: u8) -> lumza_core::Result<VcpValue> {
            let current = self.register.load(Ordering::SeqCst);
            if !self.blocked.swap(true, Ordering::SeqCst) {
                let _ = self.entered.send(());
                let (open, cv) = &*self.gate;
                let mut open = open.lock().unwrap();
                while !*open {
                    open = cv.wait(open).unwrap();
                }
            }
            Ok(VcpValue { current, max: 100 })
        }
        fn set_vcp(&mut self, _: u8, value: u32) -> lumza_core::Result<()> {
            self.register.store(value, Ordering::SeqCst);
            Ok(())
        }
    }

    #[test]
    fn read_started_before_a_write_does_not_report_the_old_value() {
        let (entered_tx, entered) = std::sync::mpsc::channel();
        let (ev_tx, ev_rx) = std::sync::mpsc::channel();
        let ev_tx = Mutex::new(ev_tx);
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let port = SlowReadPort {
            register: Arc::new(std::sync::atomic::AtomicU32::new(1)),
            entered: entered_tx,
            gate: gate.clone(),
            blocked: AtomicBool::new(false),
        };
        let cb: Callback = Arc::new(move |ev| {
            if let Event::Value(_, v) = ev {
                let _ = ev_tx.lock().unwrap().send(v.current);
            }
        });
        let worker = Worker::spawn(Box::new(port), "test".into(), cb);

        worker.read();
        entered.recv_timeout(Duration::from_secs(5)).expect("worker started the read");
        worker.write(Feature::Brightness, 50); // the user moves the slider while the read is in flight
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();

        worker.read(); // runs after the write
        let first = ev_rx.recv_timeout(Duration::from_secs(5)).expect("a value after the write");
        assert_eq!(first, 50, "the stale read leaked through");
    }

    const ON: HdrState = HdrState { supported: true, enabled: true };
    const OFF: HdrState = HdrState { supported: true, enabled: false };

    fn hdr_of(entries: &[(&str, HdrState)]) -> HashMap<String, HdrState> {
        entries.iter().map(|(id, s)| (id.to_string(), *s)).collect()
    }

    #[test]
    fn merge_hdr_reports_change_then_no_change() {
        let mut map = HashMap::new();
        assert!(merge_hdr(&mut map, vec![("a".into(), Some(OFF))]));
        assert!(!merge_hdr(&mut map, vec![("a".into(), Some(OFF))]));
    }

    #[test]
    fn merge_hdr_drops_none_and_missing() {
        let mut map = hdr_of(&[("a", OFF), ("b", OFF)]);
        assert!(merge_hdr(&mut map, vec![("a".into(), None)]));
        assert!(map.is_empty());
    }

    #[test]
    fn merge_hdr_detects_enabled_flip() {
        let mut map = hdr_of(&[("a", OFF)]);
        assert!(merge_hdr(&mut map, vec![("a".into(), Some(ON))]));
        assert_eq!(map["a"], ON);
    }
}
