//! Display-mode logic that does not need Windows: naming the current layout and checking a request.
//! The calls that read and change the real topology are in `display_config.rs`.

use crate::error::{Error, Result};
use crate::types::{DisplayEntry, DisplayMode};

/// Display numbers for the connected displays, in the given order. `windows` holds the number
/// Windows shows for a display that is in use (its GDI name, `\\.\DISPLAY2` -> 2), `None` for one
/// that is switched off. In-use displays keep their number so "only on 1" means what Windows'
/// own "Identify" shows; the rest take the lowest numbers still free. Never repeats a number.
pub fn assign_numbers(windows: &[Option<u32>]) -> Vec<u32> {
    let mut taken: Vec<u32> = Vec::new();
    let mut numbers: Vec<Option<u32>> = windows
        .iter()
        .map(|w| match w {
            Some(n) if *n > 0 && !taken.contains(n) => {
                taken.push(*n);
                Some(*n)
            }
            _ => None,
        })
        .collect();
    let mut free = (1..).filter(|n| !taken.contains(n));
    for n in numbers.iter_mut().filter(|n| n.is_none()) {
        *n = free.next();
    }
    numbers.into_iter().flatten().collect()
}

/// Names the current layout from the active display paths, one `(target key, source key)` each.
/// Paths that share a source show the same desktop area, which is what duplicate means.
pub fn classify(active: &[(String, String)]) -> Option<DisplayMode> {
    match active {
        [] => None,
        [(target, _)] => Some(DisplayMode::Only(target.clone())),
        [(_, first), rest @ ..] if rest.iter().all(|(_, s)| s == first) => Some(DisplayMode::Duplicate),
        _ => Some(DisplayMode::Extend),
    }
}

/// Rejects requests that cannot be applied: nothing to switch between, or a display that is gone.
pub fn validate(mode: &DisplayMode, displays: &[DisplayEntry]) -> Result<()> {
    if displays.len() < 2 {
        return Err(Error::Invalid("fewer than two displays are connected".into()));
    }
    match mode {
        DisplayMode::Only(key) if !displays.iter().any(|d| &d.key == key) => Err(Error::NotFound(key.clone())),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(key: &str, number: u32, active: bool) -> DisplayEntry {
        DisplayEntry { key: key.into(), number, name: format!("Display {number}"), active }
    }

    fn p(target: &str, source: &str) -> (String, String) {
        (target.into(), source.into())
    }

    #[test]
    fn numbers_fall_back_to_order_when_windows_gives_none() {
        assert_eq!(assign_numbers(&[None, None, None]), [1, 2, 3]);
    }

    #[test]
    fn active_displays_keep_their_windows_number_and_the_rest_take_free_ones() {
        assert_eq!(assign_numbers(&[Some(2), None]), [2, 1]);
        assert_eq!(assign_numbers(&[None, Some(1)]), [2, 1]);
        assert_eq!(assign_numbers(&[None, Some(1), None, Some(3)]), [2, 1, 4, 3]);
    }

    #[test]
    fn a_number_is_never_handed_out_twice() {
        assert_eq!(assign_numbers(&[Some(1), Some(1), None]), [1, 2, 3]);
    }

    #[test]
    fn no_active_paths_is_unknown() {
        assert_eq!(classify(&[]), None);
    }

    #[test]
    fn one_active_path_is_only_that_display() {
        assert_eq!(classify(&[p("t1", "s1")]), Some(DisplayMode::Only("t1".into())));
    }

    #[test]
    fn shared_source_is_duplicate() {
        assert_eq!(classify(&[p("t1", "s1"), p("t2", "s1")]), Some(DisplayMode::Duplicate));
    }

    #[test]
    fn distinct_sources_are_extend() {
        assert_eq!(classify(&[p("t1", "s1"), p("t2", "s2")]), Some(DisplayMode::Extend));
    }

    #[test]
    fn partial_clone_of_three_is_extend() {
        assert_eq!(classify(&[p("t1", "s1"), p("t2", "s1"), p("t3", "s2")]), Some(DisplayMode::Extend));
    }

    #[test]
    fn one_display_cannot_switch() {
        let d = [entry("t1", 1, true)];
        for m in [DisplayMode::Duplicate, DisplayMode::Extend, DisplayMode::Only("t1".into())] {
            assert!(matches!(validate(&m, &d), Err(Error::Invalid(_))));
        }
    }

    #[test]
    fn stale_key_is_not_found() {
        let d = [entry("t1", 1, true), entry("t2", 2, false)];
        assert!(matches!(validate(&DisplayMode::Only("gone".into()), &d), Err(Error::NotFound(_))));
    }

    #[test]
    fn known_key_and_plain_modes_pass_with_two_displays() {
        let d = [entry("t1", 1, true), entry("t2", 2, false)];
        for m in [DisplayMode::Duplicate, DisplayMode::Extend, DisplayMode::Only("t2".into())] {
            assert!(validate(&m, &d).is_ok());
        }
    }
}
