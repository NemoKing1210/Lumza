//! Flyout window: pre-created hidden, shown near the tray icon, hidden on blur.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, Rect, WebviewWindow};

const MARGIN: f64 = 12.0;
/// A tray click right after blur-hide is the same click that dismissed the flyout.
const REOPEN_GUARD: Duration = Duration::from_millis(250);
const MAX_HEIGHT_FRACTION: f64 = 0.92;
/// The window glides to a new height instead of following the layout frame by frame.
const GLIDE: Duration = Duration::from_millis(260);
const GLIDE_TICK: Duration = Duration::from_millis(8);

/// One running height change; everything is in physical px so the glide thread needs no window queries.
struct Glide {
    hwnd: isize,
    x: i32,
    width: i32,
    from: f64,
    to: f64,
    started: Instant,
    /// Window edge that stays put: its bottom when the tray is below the middle of the screen, else its top.
    edge: i32,
    from_bottom: bool,
}

impl Glide {
    fn height_at(&self, now: Instant) -> (i32, bool) {
        let t = (now.duration_since(self.started).as_secs_f64() / GLIDE.as_secs_f64()).min(1.0);
        let eased = 1.0 - (1.0 - t).powi(4);
        ((self.from + (self.to - self.from) * eased).round() as i32, t >= 1.0)
    }

    fn apply(&self, h: i32) {
        let y = if self.from_bottom { self.edge - h } else { self.edge };
        set_bounds(self.hwnd, self.x, y, self.width, h);
    }
}

pub struct Flyout {
    /// Tray icon center in physical pixels, remembered for re-anchoring on resize.
    anchor: Mutex<Option<(f64, f64)>>,
    last_hide: Mutex<Option<Instant>>,
    /// The running height change, if any; the glide thread lives exactly as long as this is `Some`.
    glide: Arc<Mutex<Option<Glide>>>,
    /// True when no Mica/Acrylic could be applied; the UI then paints an opaque background.
    pub solid: bool,
}

impl Flyout {
    pub fn new(solid: bool) -> Self {
        Self { anchor: Mutex::new(None), last_hide: Mutex::new(None), glide: Arc::new(Mutex::new(None)), solid }
    }

    pub fn window(app: &AppHandle) -> Option<WebviewWindow> {
        app.get_webview_window("flyout")
    }

    pub fn toggle(&self, app: &AppHandle, tray_rect: Option<Rect>) {
        let started = Instant::now();
        let Some(win) = Self::window(app) else { return };
        if win.is_visible().unwrap_or(false) {
            self.hide(&win);
            return;
        }
        if self.last_hide.lock().unwrap().is_some_and(|t| t.elapsed() < REOPEN_GUARD) {
            return;
        }
        if let Some(r) = tray_rect {
            let scale = win.scale_factor().unwrap_or(1.0);
            let p = r.position.to_physical::<f64>(scale);
            let s = r.size.to_physical::<f64>(scale);
            *self.anchor.lock().unwrap() = Some((p.x + s.width / 2.0, p.y + s.height / 2.0));
        }
        self.place(&win);
        let _ = win.show();
        let _ = win.set_focus();
        let _ = win.emit("flyout-shown", ());
        tracing::info!(ms = started.elapsed().as_millis() as u64, "flyout shown");
    }

    pub fn hide(&self, win: &WebviewWindow) {
        if win.is_visible().unwrap_or(false) {
            let _ = win.hide();
            let _ = win.emit("flyout-hidden", ());
        }
        *self.last_hide.lock().unwrap() = Some(Instant::now());
    }

    /// Resize to content height (logical px), capped at 92 % of the work area, then re-anchor.
    pub fn fit(&self, win: &WebviewWindow, logical_height: f64) {
        let scale = win.scale_factor().unwrap_or(1.0);
        let cap = self.work_area(win).map(|(_, _, _, h)| h / scale * MAX_HEIGHT_FRACTION).unwrap_or(900.0);
        let h = logical_height.clamp(80.0, cap.max(80.0));
        let (Ok(inner), Ok(outer)) = (win.inner_size(), win.outer_size()) else { return };
        let width = outer.width as f64;
        let target = (h * scale).round() + (outer.height as f64 - inner.height as f64);
        let mut slot = self.glide.lock().unwrap();
        // Where the window is right now, which is mid-way when a change is still running.
        let current = slot.as_ref().map_or(outer.height as f64, |g| g.height_at(Instant::now()).0 as f64);
        if slot.as_ref().map_or(current == target, |g| g.to == target) {
            return;
        }
        let (Some((x, y)), Some((_, wy, _, wh))) = (self.position(win, width, current), self.work_area(win)) else {
            return;
        };
        let from_bottom = self.anchor.lock().unwrap().is_none_or(|(_, cy)| cy > wy + wh / 2.0);
        let edge = if from_bottom { y + current as i32 } else { y };
        let Ok(hwnd) = win.hwnd() else { return };
        let glide = Glide {
            hwnd: hwnd.0 as isize,
            x,
            width: width as i32,
            from: current,
            to: target,
            started: Instant::now(),
            edge,
            from_bottom,
        };
        if !win.is_visible().unwrap_or(false) {
            // Nobody sees a hidden window change: jump.
            glide.apply(target as i32);
            *slot = None;
            return;
        }
        let spawn = slot.is_none();
        *slot = Some(glide);
        drop(slot);
        if spawn {
            let slot = self.glide.clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(GLIDE_TICK);
                let mut g = slot.lock().unwrap();
                let Some(glide) = g.as_ref() else { return };
                let (h, done) = glide.height_at(Instant::now());
                glide.apply(h);
                if done {
                    *g = None;
                    return;
                }
            });
        }
    }

    /// Work area (x, y, w, h) in physical px of the monitor holding the tray icon.
    fn work_area(&self, win: &WebviewWindow) -> Option<(f64, f64, f64, f64)> {
        let anchor = *self.anchor.lock().unwrap();
        let mon = match anchor {
            Some((x, y)) => win.app_handle().monitor_from_point(x, y).ok().flatten(),
            None => None,
        }
        .or_else(|| win.primary_monitor().ok().flatten())?;
        let wa = mon.work_area();
        Some((wa.position.x as f64, wa.position.y as f64, wa.size.width as f64, wa.size.height as f64))
    }

    /// Anchor to the work-area edge nearest the tray icon so it respects taskbar
    /// position (bottom/top/side) and DPI.
    fn place(&self, win: &WebviewWindow) {
        let Ok(size) = win.outer_size() else { return };
        let Some((x, y)) = self.position(win, size.width as f64, size.height as f64) else { return };
        let _ = win.set_position(PhysicalPosition::new(x, y));
    }

    /// Top-left corner (physical px) for a window of `w` x `h` physical px.
    fn position(&self, win: &WebviewWindow, w: f64, h: f64) -> Option<(i32, i32)> {
        let (wx, wy, ww, wh) = self.work_area(win)?;
        let scale = win.scale_factor().unwrap_or(1.0);
        let m = MARGIN * scale;
        let (cx, cy) = self.anchor.lock().unwrap().unwrap_or((wx + ww, wy + wh));
        let x = (cx - w / 2.0).clamp(wx + m, (wx + ww - w - m).max(wx + m));
        let y = if cy > wy + wh / 2.0 { wy + wh - h - m } else { wy + m };
        Some((x.round() as i32, y.round() as i32))
    }
}

/// Moves and resizes in one `SetWindowPos` (two calls would show the window between them), without stealing focus or touching the z-order.
fn set_bounds(hwnd: isize, x: i32, y: i32, w: i32, h: i32) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{SetWindowPos, SWP_NOACTIVATE, SWP_NOZORDER};
    // Rebuilt from the raw pointer: tauri may link another `windows` version than this crate.
    let hwnd = HWND(hwnd as _);
    let _ = unsafe { SetWindowPos(hwnd, None, x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE) };
}
