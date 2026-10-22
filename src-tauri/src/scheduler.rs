//! Schedule runner (spec §4.4). One thread that sleeps until something can change:
//! the next point, the next step of a running transition (≥ 30 s apart), the end of a pause,
//! or a nudge from settings / monitor / clock events. There is no polling timer.

use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use lumza_core::schedule::{evaluate, format_hhmm, sun_times, Evaluation, ResolvedPoint, DAY};
use lumza_core::store::{Schedule, ScheduleAction, Settings, SettingsStore, Trigger};
use lumza_core::Feature;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::clock::{self, Now};
use crate::controller::{Controller, SchedMonitor};
use crate::i18n;

/// Transition steps are at least this far apart (EEPROM wear, spec §12).
const MIN_STEP: Duration = Duration::from_secs(30);
const FULLSCREEN_RECHECK: Duration = Duration::from_secs(60);
const MAX_SLEEP: Duration = Duration::from_secs(6 * 3600);

#[derive(Serialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct NextPoint {
    pub at: String,
    pub label: String,
}

#[derive(Serialize, Clone, PartialEq, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleState {
    pub enabled: bool,
    pub paused: bool,
    /// Local time the pause ends, `"HH:MM"`.
    pub paused_until: Option<String>,
    pub next: Option<NextPoint>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PreviewPoint {
    pub id: String,
    pub minute: f64,
    pub label: String,
    /// "time" | "sunrise" | "sunset"
    pub kind: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    pub points: Vec<PreviewPoint>,
    /// (minute of day, mean brightness %) every 10 minutes.
    pub curve: Vec<(f64, f64)>,
    pub sunrise: Option<f64>,
    pub sunset: Option<f64>,
    pub now: f64,
}

struct Pause {
    until_epoch: u64,
    label: String,
}

#[derive(Default)]
struct Inner {
    nudge: bool,
    pause: Option<Pause>,
    last_state: ScheduleState,
}

pub struct Scheduler {
    app: AppHandle,
    inner: Mutex<Inner>,
    cv: Condvar,
}

/// Points of one day, resolved against today's sun times and the connected monitors.
struct Plan {
    points: Vec<ResolvedPoint>,
    labels: HashMap<String, (String, String)>, // id -> (label, kind)
    sunrise: Option<f64>,
    sunset: Option<f64>,
}

fn wrap(m: f64) -> f64 {
    m.rem_euclid(DAY)
}

fn sun_local(now: &Now, sc: &Schedule) -> (Option<f64>, Option<f64>) {
    let Some(loc) = &sc.location else { return (None, None) };
    let s = sun_times(now.year, now.month, now.day, loc.lat, loc.lon);
    let off = now.utc_offset_min as f64;
    (s.sunrise_utc.map(|m| wrap(m + off)), s.sunset_utc.map(|m| wrap(m + off)))
}

fn resolve(settings: &Settings, monitors: &[SchedMonitor], now: &Now) -> Plan {
    let sc = &settings.schedule;
    let (sunrise, sunset) = sun_local(now, sc);
    let mut points = Vec::new();
    let mut labels = HashMap::new();

    for p in &sc.points {
        let (at, kind) = match &p.trigger {
            Trigger::Time { at } => match lumza_core::schedule::parse_hhmm(at) {
                Some(m) => (m as f64, "time"),
                None => continue,
            },
            Trigger::Sun { event, offset_min } => {
                let base = if event == "sunset" { sunset } else { sunrise };
                // No location, or polar day/night: the point cannot fire today.
                match base {
                    Some(b) => (wrap(b + *offset_min as f64), if event == "sunset" { "sunset" } else { "sunrise" }),
                    None => continue,
                }
            }
        };

        let mut values: HashMap<(String, Feature), f64> = HashMap::new();
        let label = match &p.action {
            ScheduleAction::Profile { profile_id } => {
                let Some(profile) = settings.profiles.iter().find(|x| &x.id == profile_id) else { continue };
                for (mid, feats) in &profile.values {
                    for (name, v) in feats {
                        if let Ok(f) = name.parse::<Feature>() {
                            values.insert((mid.clone(), f), *v as f64);
                        }
                    }
                }
                i18n::tf(&settings.general.language, "be.sched.profileLabel", &[("name", &profile.name)])
            }
            ScheduleAction::Brightness { value, monitors: only } => {
                for m in monitors.iter().filter(|m| m.features.iter().any(|(f, _)| *f == Feature::Brightness)) {
                    if only.as_ref().is_some_and(|ids| !ids.contains(&m.id)) {
                        continue;
                    }
                    let ms = settings.monitors.get(&m.id);
                    // "All monitors" is a common value: map it through the calibration range,
                    // exactly like the linked slider. Explicit monitors get the raw percent.
                    let v = if only.is_none() && !ms.is_some_and(|s| s.excluded_from_link) {
                        let r = ms.and_then(|s| s.calibration.get("brightness")).copied().unwrap_or_default();
                        r.min as f64 + *value as f64 * (r.max as f64 - r.min as f64) / 100.0
                    } else {
                        *value as f64
                    };
                    values.insert((m.id.clone(), Feature::Brightness), v);
                }
                i18n::tf(&settings.general.language, "be.sched.brightnessLabel", &[("value", &value.to_string())])
            }
        };
        labels.insert(p.id.clone(), (label, kind.to_string()));
        points.push(ResolvedPoint { id: p.id.clone(), at, values });
    }
    Plan { points, labels, sunrise, sunset }
}

impl Scheduler {
    pub fn start(app: &AppHandle) -> Arc<Self> {
        let this = Arc::new(Self { app: app.clone(), inner: Mutex::new(Inner::default()), cv: Condvar::new() });
        let runner = this.clone();
        std::thread::Builder::new().name("scheduler".into()).spawn(move || runner.run()).expect("spawn scheduler");
        this
    }

    /// Poison-tolerant: `nudge` and friends are called from the event loop, where a panic would
    /// take the whole app down because the scheduler thread once panicked.
    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn run(&self) {
        loop {
            // A panic in one evaluation must not end the thread for good: the schedule would
            // silently stop until the app is restarted. Try again in a minute instead.
            let wait = catch_unwind(AssertUnwindSafe(|| self.tick())).unwrap_or_else(|_| {
                tracing::error!("schedule evaluation panicked; retrying in a minute");
                Some(Duration::from_secs(60))
            });
            let mut g = self.inner();
            if g.nudge {
                g.nudge = false;
                continue;
            }
            g = match wait {
                Some(d) => self.cv.wait_timeout(g, d.min(MAX_SLEEP)).unwrap_or_else(PoisonError::into_inner).0,
                None => self.cv.wait(g).unwrap_or_else(PoisonError::into_inner),
            };
            g.nudge = false;
        }
    }

    /// Re-evaluate now (settings edited, monitors (re)appeared, clock changed, pause ended).
    pub fn nudge(&self) {
        self.inner().nudge = true;
        self.cv.notify_one();
    }

    pub fn state(&self) -> ScheduleState {
        self.inner().last_state.clone()
    }

    /// Plan for the current moment, or `None` when the schedule has nothing to do.
    fn plan(&self) -> Option<(Settings, Plan, Now, Evaluation)> {
        let store = self.app.try_state::<Arc<SettingsStore>>()?;
        let ctrl = self.app.try_state::<Arc<Controller>>()?;
        let settings = store.get();
        let now = clock::now();
        let mut plan = resolve(&settings, &ctrl.schedule_monitors(), &now);
        let ev = evaluate(&mut plan.points, now.minute, settings.schedule.transition_min as f64)?;
        Some((settings, plan, now, ev))
    }

    fn tick(&self) -> Option<Duration> {
        let Some(store) = self.app.try_state::<Arc<SettingsStore>>() else { return Some(Duration::from_secs(1)) };
        let Some(ctrl) = self.app.try_state::<Arc<Controller>>() else { return Some(Duration::from_secs(1)) };
        let settings = store.get();
        if !settings.schedule.enabled {
            self.inner().pause = None;
            self.publish(ScheduleState::default());
            return None;
        }

        let now = clock::now();
        let monitors = ctrl.schedule_monitors();
        let mut plan = resolve(&settings, &monitors, &now);
        let Some(ev) = evaluate(&mut plan.points, now.minute, settings.schedule.transition_min as f64) else {
            self.publish(ScheduleState { enabled: true, ..Default::default() });
            return None; // no usable points: wait for a settings change
        };
        let next = {
            let id = &plan.points[(ev.active + 1) % plan.points.len()].id;
            NextPoint {
                at: format_hhmm(now.minute + ev.next_trigger_in),
                label: plan.labels.get(id).map(|l| l.0.clone()).unwrap_or_default(),
            }
        };

        // Pause bookkeeping.
        let pause = {
            let mut g = self.inner();
            if g.pause.as_ref().is_some_and(|p| now.epoch_secs >= p.until_epoch) {
                g.pause = None;
            }
            g.pause.as_ref().map(|p| (p.until_epoch - now.epoch_secs, p.label.clone()))
        };
        if let Some((remaining, label)) = pause {
            self.publish(ScheduleState { enabled: true, paused: true, paused_until: Some(label), next: Some(next) });
            return Some(Duration::from_secs(remaining + 1));
        }
        self.publish(ScheduleState { enabled: true, paused: false, paused_until: None, next: Some(next) });

        if settings.schedule.skip_fullscreen && clock::fullscreen_active() {
            return Some(FULLSCREEN_RECHECK);
        }

        if !monitors.is_empty() {
            let values: Vec<(String, Feature, u8)> =
                ev.values.iter().map(|((id, f), v)| (id.clone(), *f, v.round().clamp(0.0, 100.0) as u8)).collect();
            let n = ctrl.apply_scheduled(&values);
            if n > 0 {
                tracing::info!(changed = n, point = %plan.points[ev.active].id, "schedule applied");
            }
        }

        Some(match ev.ramp_remaining {
            // Land exactly on the final value, but never step more often than MIN_STEP.
            Some(rem) => Duration::from_secs_f64(rem * 60.0 + 1.0).min(MIN_STEP),
            None => Duration::from_secs_f64(ev.next_trigger_in * 60.0 + 1.0),
        })
    }

    fn publish(&self, state: ScheduleState) {
        let changed = {
            let mut g = self.inner();
            let changed = g.last_state != state;
            g.last_state = state.clone();
            changed
        };
        if changed {
            let _ = self.app.emit("schedule-state", state);
        }
    }

    /// Pause for `minutes`, or until the next point when `None`.
    pub fn pause(&self, minutes: Option<u32>) {
        let Some((_, _, now, ev)) = self.plan() else { return };
        let delta_min = match minutes {
            Some(m) => m as f64,
            None => ev.next_trigger_in,
        };
        let label = format_hhmm(now.minute + delta_min);
        self.inner().pause = Some(Pause { until_epoch: now.epoch_secs + (delta_min * 60.0).ceil() as u64, label });
        self.nudge();
    }

    /// Resume now; the value that is right for this moment is applied immediately.
    pub fn resume(&self) {
        self.inner().pause = None;
        self.nudge();
    }

    /// Toggle for the hotkey / tray; returns (paused now, text for the OSD).
    pub fn toggle(&self) -> (bool, String) {
        let st = self.state();
        let lang = i18n::lang_of(&self.app);
        if !st.enabled {
            return (false, i18n::t(&lang, "be.sched.off"));
        }
        if st.paused {
            self.resume();
            (false, i18n::t(&lang, "be.sched.resumed"))
        } else {
            self.pause(None);
            // `pause` publishes through the nudge; compute the label for immediate feedback.
            let until = self.plan().map(|(_, _, now, ev)| format_hhmm(now.minute + ev.next_trigger_in));
            let text = match until {
                Some(u) => i18n::tf(&lang, "be.sched.pausedUntil", &[("time", &u)]),
                None => i18n::t(&lang, "be.sched.paused"),
            };
            (true, text)
        }
    }

    /// The user changed a value by hand: pause according to the schedule's setting (spec §4.4).
    pub fn note_manual(&self) {
        let Some(store) = self.app.try_state::<Arc<SettingsStore>>() else { return };
        // Runs on every slider tick: read the few fields needed instead of cloning all settings.
        let (active, minutes, pause_minutes) = store.read(|s| {
            let sc = &s.schedule;
            let active = sc.enabled && !sc.points.is_empty() && sc.pause_on_manual != "off";
            (active, sc.pause_on_manual == "minutes", sc.pause_minutes)
        });
        if !active {
            return;
        }
        if self.inner().pause.is_some() {
            return; // already paused: do not slide the end time on every slider tick
        }
        if minutes {
            self.pause(Some(pause_minutes));
        } else {
            self.pause(None);
        }
    }

    pub fn preview(&self) -> Preview {
        let Some(store) = self.app.try_state::<Arc<SettingsStore>>() else {
            return Preview { points: vec![], curve: vec![], sunrise: None, sunset: None, now: 0.0 };
        };
        let Some(ctrl) = self.app.try_state::<Arc<Controller>>() else {
            return Preview { points: vec![], curve: vec![], sunrise: None, sunset: None, now: 0.0 };
        };
        let settings = store.get();
        let now = clock::now();
        let plan = resolve(&settings, &ctrl.schedule_monitors(), &now);
        let transition = settings.schedule.transition_min as f64;

        let mut curve = Vec::new();
        let mut m = 0.0;
        while m < DAY {
            let mut pts = plan.points.clone();
            if let Some(ev) = evaluate(&mut pts, m, transition) {
                let b: Vec<f64> =
                    ev.values.iter().filter(|((_, f), _)| *f == Feature::Brightness).map(|(_, v)| *v).collect();
                if !b.is_empty() {
                    curve.push((m, b.iter().sum::<f64>() / b.len() as f64));
                }
            }
            m += 10.0;
        }
        let points = plan
            .points
            .iter()
            .map(|p| {
                let (label, kind) = plan.labels.get(&p.id).cloned().unwrap_or_default();
                PreviewPoint { id: p.id.clone(), minute: p.at, label, kind }
            })
            .collect();
        Preview { points, curve, sunrise: plan.sunrise, sunset: plan.sunset, now: now.minute }
    }
}

/// Call after any change the user made by hand (slider, hotkey, tray profile).
pub fn manual(app: &AppHandle) {
    if let Some(s) = app.try_state::<Arc<Scheduler>>() {
        s.note_manual();
    }
}
