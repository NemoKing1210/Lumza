//! Backend strings (tray, OSD, notices). Same JSON files as the UI: `locales/*.json`.

use std::collections::HashMap;
use std::sync::OnceLock;

type Dict = HashMap<String, String>;

fn parse(src: &str) -> Dict {
    serde_json::from_str(src).expect("locale file is valid JSON")
}

/// Every UI language, English first (it is also the fallback).
fn dicts() -> &'static Vec<(&'static str, Dict)> {
    static D: OnceLock<Vec<(&'static str, Dict)>> = OnceLock::new();
    D.get_or_init(|| {
        vec![
            ("en", parse(include_str!("../../locales/en.json"))),
            ("ru", parse(include_str!("../../locales/ru.json"))),
            ("es", parse(include_str!("../../locales/es.json"))),
            ("de", parse(include_str!("../../locales/de.json"))),
            ("fr", parse(include_str!("../../locales/fr.json"))),
            ("pt", parse(include_str!("../../locales/pt.json"))),
            ("zh", parse(include_str!("../../locales/zh.json"))),
        ]
    })
}

/// Text for `key` in `lang` ("auto" follows the system language); falls back to English, then to the key itself.
pub fn t(lang: &str, key: &str) -> String {
    let lang = if lang == "auto" { system_language() } else { lang };
    let d = dicts();
    d.iter()
        .find(|(l, _)| *l == lang)
        .and_then(|(_, m)| m.get(key))
        .or_else(|| d[0].1.get(key))
        .cloned()
        .unwrap_or_else(|| key.to_string())
}

/// Like [`t`], substituting `{name}` placeholders.
pub fn tf(lang: &str, key: &str, params: &[(&str, &str)]) -> String {
    let mut s = t(lang, key);
    for (k, v) in params {
        s = s.replace(&format!("{{{k}}}"), v);
    }
    s
}

/// Current UI language from the settings in app state.
pub fn lang_of(app: &tauri::AppHandle) -> String {
    use tauri::Manager;
    app.try_state::<std::sync::Arc<lumza_core::store::SettingsStore>>()
        .map(|s| s.read(|s| s.general.language.clone()))
        .unwrap_or_else(|| "en".into())
}

/// The supported UI language that matches the Windows display language, "en" otherwise.
pub fn system_language() -> &'static str {
    use windows::Win32::Globalization::GetUserDefaultLocaleName;
    let mut buf = [0u16; 85];
    let n = unsafe { GetUserDefaultLocaleName(&mut buf) };
    let name = String::from_utf16_lossy(&buf[..(n.max(1) as usize - 1).min(buf.len())]).to_ascii_lowercase();
    dicts().iter().map(|(l, _)| *l).find(|l| name.starts_with(l)).unwrap_or("en")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locales_have_identical_keys_and_placeholders() {
        let d = dicts();
        let (_, en) = &d[0];
        let mut en_keys: Vec<_> = en.keys().collect();
        en_keys.sort();
        let ph = |s: &str| {
            let mut v: Vec<&str> =
                s.match_indices('{').filter_map(|(i, _)| s[i..].find('}').map(|j| &s[i..=i + j])).collect();
            v.sort();
            v.into_iter().map(String::from).collect::<Vec<_>>()
        };
        for (lang, m) in d {
            let mut keys: Vec<_> = m.keys().collect();
            keys.sort();
            assert_eq!(keys, en_keys, "{lang}.json and en.json must define the same keys");
            for (k, v) in m {
                assert_eq!(ph(v), ph(&en[k]), "placeholders differ for {k} in {lang}");
            }
        }
    }

    #[test]
    fn formatting_and_fallback() {
        assert_eq!(tf("en", "be.sched.brightnessLabel", &[("value", "40")]), "Brightness 40%");
        assert_eq!(t("en", "no.such.key"), "no.such.key");
    }
}
