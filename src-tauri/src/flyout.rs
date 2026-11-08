//! Flyout window: pre-created hidden, shown near the tray icon, hidden on blur.

use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, Rect, WebviewWindow};

const MARGIN: f64 = 12.0;
/// A tray click right after blur-hide is the same click that dismissed the flyout.
const REOPEN_GUARD: Duration = Duration::from_millis(250);
const MAX_HEIGHT_FRACTION: f64 = 0.92;

pub struct Flyout {
    /// Tray icon center in physical pixels, remembered for re-anchoring on resize.
    anchor: Mutex<Option<(f64, f64)>>,
    last_hide: Mutex<Option<Instant>>,
    /// True when no Mica/Acrylic could be applied; the UI then paints an opaque background.
    pub solid: bool,
}

impl Flyout {
    pub fn new(solid: bool) -> Self {
        Self { anchor: Mutex::new(None), last_hide: Mutex::new(None), solid }
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
        if self.last_hide.lock().unwrap_or_else(PoisonError::into_inner).is_some_and(|t| t.elapsed() < REOPEN_GUARD) {
            return;
        }
        if let Some(r) = tray_rect {
            let scale = win.scale_factor().unwrap_or(1.0);
            let p = r.position.to_physical::<f64>(scale);
            let s = r.size.to_physical::<f64>(scale);
            *self.anchor.lock().unwrap_or_else(PoisonError::into_inner) =
                Some((p.x + s.width / 2.0, p.y + s.height / 2.0));
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
        *self.last_hide.lock().unwrap_or_else(PoisonError::into_inner) = Some(Instant::now());
    }

    /// Resize to content height (logical px), capped at 92 % of the work area, then re-anchor.
    pub fn fit(&self, win: &WebviewWindow, logical_height: f64) {
        let scale = win.scale_factor().unwrap_or(1.0);
        let cap = self.work_area(win).map(|(_, _, _, h)| h / scale * MAX_HEIGHT_FRACTION).unwrap_or(900.0);
        let h = logical_height.clamp(80.0, cap.max(80.0));
        let (Ok(inner), Ok(outer)) = (win.inner_size(), win.outer_size()) else { return };
        let width = outer.width as f64;
        let target = (h * scale).round() + (outer.height as f64 - inner.height as f64);
        if target as i32 == outer.height as i32 {
            return;
        }
        let Some((x, y)) = self.position(win, width, target) else { return };
        let Ok(hwnd) = win.hwnd() else { return };
        set_bounds(hwnd.0 as isize, x, y, width as i32, target as i32);
    }

    /// Work area (x, y, w, h) in physical px of the monitor holding the tray icon.
    fn work_area(&self, win: &WebviewWindow) -> Option<(f64, f64, f64, f64)> {
        let anchor = *self.anchor.lock().unwrap_or_else(PoisonError::into_inner);
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
        let (cx, cy) = self.anchor.lock().unwrap_or_else(PoisonError::into_inner).unwrap_or((wx + ww, wy + wh));
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
