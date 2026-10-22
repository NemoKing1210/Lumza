use std::sync::Arc;

use lumza_core::store::SettingsStore;
use tauri::image::Image;
use tauri::menu::{CheckMenuItemBuilder, Menu, MenuBuilder, SubmenuBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};

use crate::controller::Controller;
use crate::display_modes::{menu_entries, mode_from_menu_id};
use crate::flyout::Flyout;
use crate::i18n;
use crate::scheduler::Scheduler;

/// Glyph styles come as a white (for a dark taskbar) and a black (for a light taskbar) PNG; "color" is
/// the app icon and the same on both. Keep in step with `store::TRAY_ICONS`.
struct IconStyle {
    key: &'static str,
    on_dark_taskbar: &'static [u8],
    on_light_taskbar: &'static [u8],
}

macro_rules! glyph {
    ($key:literal) => {
        IconStyle {
            key: $key,
            on_dark_taskbar: include_bytes!(concat!("../icons/tray-", $key, "-light.png")),
            on_light_taskbar: include_bytes!(concat!("../icons/tray-", $key, "-dark.png")),
        }
    };
}

const ICON_STYLES: [IconStyle; 6] = [
    glyph!("contrast"),
    glyph!("sun"),
    glyph!("moon"),
    glyph!("monitor"),
    glyph!("sliders"),
    IconStyle {
        key: "color",
        on_dark_taskbar: include_bytes!("../icons/32x32.png"),
        on_light_taskbar: include_bytes!("../icons/32x32.png"),
    },
];

/// `SystemUsesLightTheme` describes the taskbar/tray theme (not the apps theme).
fn taskbar_is_light() -> bool {
    use winreg::{enums::HKEY_CURRENT_USER, RegKey};
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize")
        .and_then(|k| k.get_value::<u32, _>("SystemUsesLightTheme"))
        .map(|v| v != 0)
        .unwrap_or(false)
}

fn icon(app: &AppHandle) -> Image<'static> {
    let key = app.try_state::<Arc<SettingsStore>>().map(|s| s.get().general.tray_icon).unwrap_or_default();
    let style = ICON_STYLES.iter().find(|s| s.key == key).unwrap_or(&ICON_STYLES[0]);
    let bytes = if taskbar_is_light() { style.on_light_taskbar } else { style.on_dark_taskbar };
    Image::from_bytes(bytes).expect("tray icon")
}

/// Re-applies the icon after a taskbar theme or `general.trayIcon` change.
pub fn update_icon(app: &AppHandle) {
    if let Some(t) = app.tray_by_id("main") {
        let _ = t.set_icon(Some(icon(app)));
    }
}

fn build_menu(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let lang = i18n::lang_of(app);
    let profiles = app.try_state::<Arc<SettingsStore>>().map(|s| s.get().profiles).unwrap_or_default();
    let mut sub = SubmenuBuilder::new(app, i18n::t(&lang, "be.tray.profiles"));
    if profiles.is_empty() {
        sub = sub.text("noprofiles", i18n::t(&lang, "be.tray.noProfiles"));
    }
    for p in &profiles {
        sub = sub.text(format!("profile:{}", p.id), &p.name);
    }
    let sub = sub.build()?;
    if profiles.is_empty() {
        if let Some(item) = sub.get("noprofiles") {
            if let Some(i) = item.as_menuitem() {
                let _ = i.set_enabled(false);
            }
        }
    }
    let sched = app.try_state::<Arc<Scheduler>>().map(|s| s.state()).unwrap_or_default();
    let toggle = i18n::t(&lang, if sched.paused { "be.tray.resume" } else { "be.tray.pause" });
    let toggle_item =
        tauri::menu::MenuItemBuilder::with_id("schedule-toggle", toggle).enabled(sched.enabled).build(app)?;
    let modes = app
        .try_state::<Arc<Controller>>()
        .and_then(|c| c.display_modes_raw())
        .unwrap_or_else(|| lumza_core::DisplayModes { displays: Vec::new(), current: None });
    let entries = menu_entries(&lang, &modes);
    let mut builder = MenuBuilder::new(app).text("refresh", i18n::t(&lang, "be.tray.refresh")).item(&sub);
    if !entries.is_empty() {
        let mut display = SubmenuBuilder::new(app, i18n::t(&lang, "be.tray.displayMode"));
        for e in &entries {
            display = display.item(&CheckMenuItemBuilder::with_id(&e.id, &e.label).checked(e.checked).build(app)?);
        }
        builder = builder.item(&display.build()?);
    }
    builder
        .item(&toggle_item)
        .text("settings", i18n::t(&lang, "be.tray.settings"))
        .separator()
        .text("quit", i18n::t(&lang, "be.tray.quit"))
        .build()
}

pub fn rebuild_menu(app: &AppHandle) {
    if let (Some(t), Ok(menu)) = (app.tray_by_id("main"), build_menu(app)) {
        let _ = t.set_menu(Some(menu));
    }
}

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let menu = build_menu(app)?;

    TrayIconBuilder::with_id("main")
        .icon(icon(app))
        .tooltip("Lumza")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, ev| match ev.id().as_ref() {
            "refresh" => {
                if let Some(c) = app.try_state::<Arc<Controller>>() {
                    c.rescan();
                }
            }
            "settings" => crate::open_settings_window(app),
            "schedule-toggle" => {
                if let Some(s) = app.try_state::<Arc<Scheduler>>() {
                    s.toggle();
                }
            }
            id if id.starts_with("display:") => {
                if let (Some(mode), Some(c)) = (mode_from_menu_id(id), app.try_state::<Arc<Controller>>()) {
                    c.set_display_mode(mode);
                }
                // A check item flips itself on click; put the check back where the real layout says.
                rebuild_menu(app);
            }
            id if id.starts_with("profile:") => {
                crate::scheduler::manual(app);
                if let Some(c) = app.try_state::<Arc<Controller>>() {
                    c.apply_profile(&id["profile:".len()..]);
                }
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, ev| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left, button_state: MouseButtonState::Up, rect, ..
            } = ev
            {
                let app = tray.app_handle();
                if let Some(f) = app.try_state::<Flyout>() {
                    f.toggle(app, Some(rect));
                }
            }
        })
        .build(app)?;
    Ok(())
}
