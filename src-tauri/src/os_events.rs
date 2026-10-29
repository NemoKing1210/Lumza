//! Hidden top-level window whose message loop turns OS broadcasts into [`OsEvent`]s:
//! display changes, hot-plug, resume from sleep, session unlock and theme changes.
//! (Message-only windows do not receive broadcasts, so this is a real, never-shown window.)

use std::sync::OnceLock;

use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::RemoteDesktop::{WTSRegisterSessionNotification, NOTIFY_FOR_THIS_SESSION};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, RegisterClassW, TranslateMessage, CW_USEDEFAULT,
    MSG, WINDOW_EX_STYLE, WINDOW_STYLE, WNDCLASSW,
};

#[derive(Debug, Clone, Copy)]
pub enum OsEvent {
    /// Resolution/topology change, hot-plug, resume, unlock: monitors must be re-enumerated.
    Rescan,
    /// Resume from sleep or session unlock: like `Rescan`, but monitors may still be waking up.
    Wake,
    /// Light/dark mode or accent colour changed.
    Theme,
    /// The system clock or time zone changed; the schedule must re-evaluate.
    Time,
}

static HANDLER: OnceLock<Box<dyn Fn(OsEvent) + Send + Sync>> = OnceLock::new();

const WM_DISPLAYCHANGE: u32 = 0x007E;
const WM_SETTINGCHANGE: u32 = 0x001A;
const WM_TIMECHANGE: u32 = 0x001E;
const WM_DEVICECHANGE: u32 = 0x0219;
const WM_POWERBROADCAST: u32 = 0x0218;
const WM_WTSSESSION_CHANGE: u32 = 0x02B1;
const PBT_APMRESUMEAUTOMATIC: usize = 0x12;
const DBT_DEVNODES_CHANGED: usize = 0x0007;
const WTS_SESSION_UNLOCK: usize = 0x8;

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let ev = match msg {
        WM_DISPLAYCHANGE => Some(OsEvent::Rescan),
        WM_TIMECHANGE => Some(OsEvent::Time),
        WM_DEVICECHANGE if wp.0 == DBT_DEVNODES_CHANGED => Some(OsEvent::Rescan),
        WM_POWERBROADCAST if wp.0 == PBT_APMRESUMEAUTOMATIC => Some(OsEvent::Wake),
        WM_WTSSESSION_CHANGE if wp.0 == WTS_SESSION_UNLOCK => Some(OsEvent::Wake),
        WM_SETTINGCHANGE if lp.0 != 0 => {
            let p = lp.0 as *const u16;
            let mut len = 0;
            while len < 64 && *p.add(len) != 0 {
                len += 1;
            }
            let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, len));
            (s == "ImmersiveColorSet").then_some(OsEvent::Theme)
        }
        _ => None,
    };
    if let (Some(ev), Some(h)) = (ev, HANDLER.get()) {
        h(ev);
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

pub fn start(handler: impl Fn(OsEvent) + Send + Sync + 'static) {
    let _ = HANDLER.set(Box::new(handler));
    std::thread::Builder::new()
        .name("os-events".into())
        .spawn(|| unsafe {
            let hinst = GetModuleHandleW(None).unwrap_or_default();
            let class = w!("LumzaOsEvents");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wnd_proc),
                hInstance: hinst.into(),
                lpszClassName: class,
                ..Default::default()
            };
            RegisterClassW(&wc);
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class,
                w!("Lumza"),
                WINDOW_STYLE(0),
                0,
                0,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                None,
                None,
                Some(hinst.into()),
                None,
            );
            let Ok(hwnd) = hwnd else {
                tracing::error!("os-events window creation failed");
                return;
            };
            let _ = WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION);
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        })
        .expect("spawn os-events");
}
