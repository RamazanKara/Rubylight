//! Active display ordering and target selection policy for live mid-stream monitor switching.

/// Maximum 1-based display index reachable via `Ctrl + Alt + Shift + F1`–`F12`.
pub const MAX_DISPLAYS: usize = 12;

const WINDOWS_DISPLAY_PREFIX: &str = r"\\.\DISPLAY";

/// Filters empty/whitespace names, deduplicates case-insensitively keeping the
/// first casing, and sorts naturally with `\\.\DISPLAY<N>` ordered numerically
/// first (`DISPLAY1`, `DISPLAY2`, ..., `DISPLAY10`) followed by all other
/// display names ordered case-insensitively.
pub fn order_active_displays<S: AsRef<str>>(names: &[S]) -> Vec<String> {
    let mut unique: Vec<String> = Vec::with_capacity(names.len());
    for raw in names {
        let name = raw.as_ref();
        if name.trim().is_empty() {
            continue;
        }
        if unique.iter().any(|seen| seen.eq_ignore_ascii_case(name)) {
            continue;
        }
        unique.push(name.to_owned());
    }
    unique.sort_by(|a, b| sort_key(a).cmp(&sort_key(b)));
    unique
}

/// Selects the display at `index_1based` (`1..=MAX_DISPLAYS`) from the ordered
/// active display list, returning `None` when out of range.
pub fn select_display<S: AsRef<str>>(active: &[S], index_1based: u8) -> Option<String> {
    if index_1based == 0 || usize::from(index_1based) > MAX_DISPLAYS {
        return None;
    }
    order_active_displays(active)
        .into_iter()
        .nth(usize::from(index_1based - 1))
}

/// Computes the new `monitor_override` for `selected`: returns `None` when
/// `selected` matches the stream's `prepared_display` (restoring the prepared
/// display/app-window target cleanly) or `Some(selected)` otherwise.
pub fn override_for_selection(selected: &str, prepared_display: &str) -> Option<String> {
    if selected.eq_ignore_ascii_case(prepared_display) {
        None
    } else {
        Some(selected.to_owned())
    }
}

/// Returns true while `current_override` matches one of the currently active
/// display names (case-insensitively).
pub fn override_still_active<S: AsRef<str>>(current_override: &str, active: &[S]) -> bool {
    active
        .iter()
        .any(|name| name.as_ref().eq_ignore_ascii_case(current_override))
}

fn sort_key(name: &str) -> (u8, u32, String, &str) {
    if let Some(num) = parse_windows_display_number(name) {
        (0, num, name.to_ascii_lowercase(), name)
    } else {
        (1, 0, name.to_ascii_lowercase(), name)
    }
}

fn parse_windows_display_number(name: &str) -> Option<u32> {
    let prefix_len = WINDOWS_DISPLAY_PREFIX.len();
    let head = name.get(..prefix_len)?;
    if !head.eq_ignore_ascii_case(WINDOWS_DISPLAY_PREFIX) {
        return None;
    }
    let tail = &name[prefix_len..];
    if tail.is_empty() || !tail.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    tail.parse::<u32>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orders_windows_displays_naturally_before_other_names() {
        let input = [
            "VirtualExtra",
            "",
            "   ",
            r"\\.\DISPLAY10",
            r"\\.\DISPLAY2",
            r"\\.\display2",
            r"\\.\DISPLAY1",
            "alpha",
            r"\\.\DISPLAYabc",
        ];
        assert_eq!(
            order_active_displays(&input),
            vec![
                r"\\.\DISPLAY1".to_owned(),
                r"\\.\DISPLAY2".to_owned(),
                r"\\.\DISPLAY10".to_owned(),
                "alpha".to_owned(),
                "VirtualExtra".to_owned(),
                r"\\.\DISPLAYabc".to_owned(),
            ]
        );
    }

    #[test]
    fn select_display_respects_1based_bounds_and_exclusive_single_display() {
        let displays = [r"\\.\DISPLAY2", r"\\.\DISPLAY1", r"\\.\DISPLAY10"];
        assert_eq!(select_display(&displays, 0), None);
        assert_eq!(
            select_display(&displays, 1).as_deref(),
            Some(r"\\.\DISPLAY1")
        );
        assert_eq!(
            select_display(&displays, 2).as_deref(),
            Some(r"\\.\DISPLAY2")
        );
        assert_eq!(
            select_display(&displays, 3).as_deref(),
            Some(r"\\.\DISPLAY10")
        );
        assert_eq!(select_display(&displays, 4), None);
        assert_eq!(select_display(&displays, 13), None);

        // Exclusive virtual-display layout yields only the single active VDD.
        let exclusive = [r"\\.\DISPLAY5"];
        assert_eq!(
            select_display(&exclusive, 1).as_deref(),
            Some(r"\\.\DISPLAY5")
        );
        assert_eq!(select_display(&exclusive, 2), None);
    }

    #[test]
    fn override_helpers_clear_on_prepared_display_and_check_presence() {
        assert_eq!(
            override_for_selection(r"\\.\DISPLAY2", r"\\.\DISPLAY1"),
            Some(r"\\.\DISPLAY2".to_owned())
        );
        assert_eq!(
            override_for_selection(r"\\.\display1", r"\\.\DISPLAY1"),
            None
        );

        let active = [r"\\.\DISPLAY1", r"\\.\DISPLAY3"];
        assert!(override_still_active(r"\\.\display3", &active));
        assert!(!override_still_active(r"\\.\DISPLAY2", &active));
    }
}
