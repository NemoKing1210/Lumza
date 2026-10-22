//! The display-mode feature seen from the app: what crosses IPC, what the tray shows, and the
//! guard that keeps two layout switches from running at once.

use std::sync::atomic::{AtomicBool, Ordering};

use lumza_core::{DisplayEntry, DisplayMode, DisplayModes};
use serde::{Deserialize, Serialize};

use crate::i18n;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum DisplayModeDto {
    Duplicate,
    Extend,
    Only { key: String },
}

impl From<DisplayMode> for DisplayModeDto {
    fn from(m: DisplayMode) -> Self {
        match m {
            DisplayMode::Duplicate => Self::Duplicate,
            DisplayMode::Extend => Self::Extend,
            DisplayMode::Only(key) => Self::Only { key },
        }
    }
}

impl From<DisplayModeDto> for DisplayMode {
    fn from(m: DisplayModeDto) -> Self {
        match m {
            DisplayModeDto::Duplicate => Self::Duplicate,
            DisplayModeDto::Extend => Self::Extend,
            DisplayModeDto::Only { key } => Self::Only(key),
        }
    }
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DisplayEntryDto {
    key: String,
    number: u32,
    name: String,
    active: bool,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DisplayModesDto {
    displays: Vec<DisplayEntryDto>,
    current: Option<DisplayModeDto>,
}

impl From<DisplayModes> for DisplayModesDto {
    fn from(m: DisplayModes) -> Self {
        Self {
            displays: m
                .displays
                .into_iter()
                .map(|d: DisplayEntry| DisplayEntryDto { key: d.key, number: d.number, name: d.name, active: d.active })
                .collect(),
            current: m.current.map(Into::into),
        }
    }
}

pub struct MenuEntry {
    pub id: String,
    pub label: String,
    pub checked: bool,
}

/// Tray entries for the current displays; empty when there is nothing to switch between.
pub fn menu_entries(lang: &str, m: &DisplayModes) -> Vec<MenuEntry> {
    if m.displays.len() < 2 {
        return Vec::new();
    }
    let entry = |id: String, label: String, mode: DisplayMode| MenuEntry {
        id,
        label,
        checked: m.current.as_ref() == Some(&mode),
    };
    let mut out = vec![
        entry("display:duplicate".into(), i18n::t(lang, "be.tray.display.duplicate"), DisplayMode::Duplicate),
        entry("display:extend".into(), i18n::t(lang, "be.tray.display.extend"), DisplayMode::Extend),
    ];
    for d in &m.displays {
        let label = i18n::tf(lang, "be.tray.display.only", &[("number", &d.number.to_string()), ("name", &d.name)]);
        out.push(entry(format!("display:only:{}", d.key), label, DisplayMode::Only(d.key.clone())));
    }
    out
}

/// The mode behind a tray menu id built by [`menu_entries`].
pub fn mode_from_menu_id(id: &str) -> Option<DisplayMode> {
    match id.strip_prefix("display:")? {
        "duplicate" => Some(DisplayMode::Duplicate),
        "extend" => Some(DisplayMode::Extend),
        other => other.strip_prefix("only:").map(|key| DisplayMode::Only(key.to_string())),
    }
}

/// One layout switch at a time. The guard releases on drop, so a panic in the switching thread
/// cannot leave the feature stuck.
pub struct Busy(AtomicBool);

pub struct BusyGuard<'a>(&'a AtomicBool);

impl Busy {
    pub fn new() -> Self {
        Self(AtomicBool::new(false))
    }

    pub fn try_acquire(&self) -> Option<BusyGuard<'_>> {
        (!self.0.swap(true, Ordering::SeqCst)).then_some(BusyGuard(&self.0))
    }
}

impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(key: &str, number: u32, name: &str, active: bool) -> DisplayEntry {
        DisplayEntry { key: key.into(), number, name: name.into(), active }
    }

    fn two(current: Option<DisplayMode>) -> DisplayModes {
        DisplayModes { displays: vec![entry("t1", 1, "LG", true), entry("t2", 2, "AOC", false)], current }
    }

    #[test]
    fn mode_dto_json_shape() {
        assert_eq!(serde_json::to_string(&DisplayModeDto::Duplicate).unwrap(), r#"{"type":"duplicate"}"#);
        assert_eq!(
            serde_json::to_string(&DisplayModeDto::Only { key: "k".into() }).unwrap(),
            r#"{"type":"only","key":"k"}"#
        );
        let back: DisplayModeDto = serde_json::from_str(r#"{"type":"extend"}"#).unwrap();
        assert_eq!(DisplayMode::from(back), DisplayMode::Extend);
    }

    #[test]
    fn modes_dto_is_camel_case_and_keeps_the_current_mode() {
        let dto = DisplayModesDto::from(two(Some(DisplayMode::Only("t2".into()))));
        let json = serde_json::to_value(&dto).unwrap();
        assert_eq!(json["displays"][1]["key"], "t2");
        assert_eq!(json["displays"][1]["active"], false);
        assert_eq!(json["current"]["type"], "only");
        assert_eq!(json["current"]["key"], "t2");
    }

    #[test]
    fn menu_entries_mark_the_current_mode() {
        let e = menu_entries("en", &two(Some(DisplayMode::Only("t2".into()))));
        assert_eq!(
            e.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(),
            ["display:duplicate", "display:extend", "display:only:t1", "display:only:t2"]
        );
        assert_eq!(e.iter().map(|x| x.checked).collect::<Vec<_>>(), [false, false, false, true]);
        assert_eq!(e[2].label, "Only on 1 (LG)");
    }

    #[test]
    fn menu_entries_are_empty_below_two_displays() {
        let one = DisplayModes { displays: vec![entry("t1", 1, "LG", true)], current: Some(DisplayMode::Extend) };
        assert!(menu_entries("en", &one).is_empty());
    }

    #[test]
    fn menu_entries_mark_nothing_when_the_mode_is_unknown() {
        assert!(menu_entries("en", &two(None)).iter().all(|e| !e.checked));
    }

    #[test]
    fn menu_id_roundtrip() {
        assert_eq!(mode_from_menu_id("display:only:a-1-2"), Some(DisplayMode::Only("a-1-2".into())));
        assert_eq!(mode_from_menu_id("display:extend"), Some(DisplayMode::Extend));
        assert_eq!(mode_from_menu_id("display:duplicate"), Some(DisplayMode::Duplicate));
        assert_eq!(mode_from_menu_id("profile:x"), None);
    }

    #[test]
    fn busy_allows_one_holder_and_releases_on_drop() {
        let b = Busy::new();
        let g = b.try_acquire().expect("first");
        assert!(b.try_acquire().is_none());
        drop(g);
        assert!(b.try_acquire().is_some());
    }

    #[test]
    fn busy_is_released_when_the_holder_panics() {
        let b = Busy::new();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _g = b.try_acquire().unwrap();
            panic!("boom");
        }));
        assert!(b.try_acquire().is_some());
    }
}
