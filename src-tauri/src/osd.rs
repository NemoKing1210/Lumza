//! On-screen display: a small, click-through window that never takes focus.
//! Created on first use and kept hidden afterwards so later shows are instant.

use std::sync::{Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, WebviewUrl, WebviewWindowBuilder};

const WIDTH: f64 = 300.0;
const HEIGHT: f64 = 84.0;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OsdPayload {
    /// "brightness" | "contrast" | "volume" or a profile icon key.
    pub icon: String,
    pub label: String,
    /// `None` for events without a level, such as "profile applied".
    pub percent: Option<u8>,
}

#[derive(Default)]
pub struct Osd {
    /// Payload for a page that finished loading after the event was emitted.
    pending: Mutex<Option<OsdPayload>>,
    /// When the window should disappear. Pushed back by every new OSD, so a held hotkey keeps it up.
    hide_at: Mutex<Option<Instant>>,
    wake: Condvar,
    hider: OnceLock<()>,
}

impl Osd {
    pub fn take_pending(&self) -> Option<OsdPayload> {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner).take()
    }

    fn deadline(&self) -> MutexGuard<'_, Option<Instant>> {
        self.hide_at.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Hide the window `after` from now; one thread serves every OSD (a held hotkey fires dozens a second).
    fn hide_later(&self, app: &AppHandle, after: Duration) {
        *self.deadline() = Some(Instant::now() + after);
        self.wake.notify_one();
        self.hider.get_or_init(|| {
            let app = app.clone();
            std::thread::Builder::new().name("osd-hide".into()).spawn(move || hide_loop(&app)).expect("spawn osd-hide");
        });
    }
}

fn hide_loop(app: &AppHandle) {
    let Some(state) = app.try_state::<Osd>() else { return };
    let mut at = state.deadline();
    loop {
        at = match *at {
            None => state.wake.wait(at).unwrap_or_else(PoisonError::into_inner),
            Some(t) if t > Instant::now() => {
                state.wake.wait_timeout(at, t - Instant::now()).unwrap_or_else(PoisonError::into_inner).0
            }
            Some(_) => {
                *at = None;
                // Hidden while holding the lock: a newer OSD pushes the deadline before it shows the
                // window, so it either lands before this check or waits for the hide and shows after it.
                if let Some(w) = app.get_webview_window("osd") {
                    let _ = w.hide();
                }
                at
            }
        };
    }
}

/// Show the OSD for `duration_ms` at the top or bottom of the monitor under the cursor.
/// Blocking (may create the window): call from a worker thread, never the event loop.
pub fn show(app: &AppHandle, payload: OsdPayload, position: &str, duration_ms: u32) {
    let Some(state) = app.try_state::<Osd>() else { return };
    let win = match app.get_webview_window("osd") {
        Some(w) => w,
        None => {
            let built = WebviewWindowBuilder::new(app, "osd", WebviewUrl::App("index.html".into()))
                .title("Lumza OSD")
                .inner_size(WIDTH, HEIGHT)
                .visible(false)
                .decorations(false)
                .transparent(true)
                .resizable(false)
                .skip_taskbar(true)
                .always_on_top(true)
                .shadow(false)
                .focused(false)
                .focusable(false)
                .additional_browser_args(crate::BROWSER_ARGS)
                .build();
            match built {
                Ok(w) => {
                    let _ = w.set_ignore_cursor_events(true);
                    w
                }
                Err(e) => {
                    tracing::warn!(error = %e, "osd window failed");
                    return;
                }
            }
        }
    };

    *state.pending.lock().unwrap_or_else(PoisonError::into_inner) = Some(payload.clone());
    let _ = win.emit("osd-show", payload);

    let scale = win.scale_factor().unwrap_or(1.0);
    let cursor = app.cursor_position().ok();
    let monitor = cursor
        .and_then(|c| app.monitor_from_point(c.x, c.y).ok().flatten())
        .or_else(|| app.primary_monitor().ok().flatten());
    if let Some(m) = monitor {
        let wa = m.work_area();
        let (w, h) = ((WIDTH * scale) as i32, (HEIGHT * scale) as i32);
        let x = wa.position.x + (wa.size.width as i32 - w) / 2;
        let y = if position == "top" {
            wa.position.y + (48.0 * scale) as i32
        } else {
            wa.position.y + wa.size.height as i32 - h - (96.0 * scale) as i32
        };
        let _ = win.set_position(PhysicalPosition::new(x, y));
    }
    state.hide_later(app, Duration::from_millis(duration_ms as u64));
    let _ = win.show();
}
