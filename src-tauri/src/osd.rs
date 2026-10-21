//! On-screen display: a small, click-through window that never takes focus.
//! Created on first use and kept hidden afterwards so later shows are instant.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

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
    generation: AtomicU64,
    /// Payload for a page that finished loading after the event was emitted.
    pending: Mutex<Option<OsdPayload>>,
}

impl Osd {
    pub fn take_pending(&self) -> Option<OsdPayload> {
        self.pending.lock().unwrap().take()
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

    *state.pending.lock().unwrap() = Some(payload.clone());
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
    let _ = win.show();

    let generation = state.generation.fetch_add(1, Ordering::SeqCst) + 1;
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(duration_ms as u64));
        // A newer OSD may have been shown meanwhile; only the latest one hides the window.
        if let Some(state) = app.try_state::<Osd>() {
            if state.generation.load(Ordering::SeqCst) == generation {
                if let Some(w) = app.get_webview_window("osd") {
                    let _ = w.hide();
                }
            }
        }
    });
}
