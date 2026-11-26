//! A row's text and what its value draws as: the label left, the value
//! right-aligned in [`COLUMNS`] characters, and the [`Widget`] the renderer
//! draws in the value's place.
use butterpollo_input_policy::couch_menu::layout::COLUMNS;

use super::{Control, Strings};

/// What a row line draws for its value, the last [`super::Line::value`]
/// characters of its text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Widget {
    /// The value's text (also "Working..." and a PC command's "Run").
    #[default]
    Text,
    /// A pill switch.
    Switch(bool),
    /// An inline slider beside the value's text.
    Slider { value: i64, min: i64, max: i64 },
    /// The value between chevrons while focused (`< value >` in the text).
    Choice,
    /// A page link: a chevron for the text's `>`.
    Link,
}

impl Widget {
    pub(super) fn of(control: &Control) -> Self {
        match control {
            Control::Toggle(on) => Self::Switch(*on),
            Control::Integer { value, range, .. } => Self::Slider {
                value: *value,
                min: range.min,
                max: range.max,
            },
            Control::Choice { .. } => Self::Choice,
            Control::Text(_) => Self::Text,
        }
    }
}

pub(super) fn shown(s: &Strings, control: &Control) -> String {
    match control {
        Control::Toggle(true) => s.on.clone(),
        Control::Toggle(false) => s.off.clone(),
        Control::Choice { index, options } => options
            .get(*index)
            .map_or_else(String::new, |o| o.label.clone()),
        Control::Integer { value, unit, .. } => match unit {
            Some(unit) => format!("{value} {unit}"),
            None => value.to_string(),
        },
        Control::Text(text) => text.clone(),
    }
}

/// `text` cut to `width` characters, ending in "..." when cut (and too narrow for that, just
/// cut).
pub(crate) fn fit(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_owned();
    }
    if width < 3 {
        return text.chars().take(width).collect();
    }
    let mut cut: String = text.chars().take(width - 3).collect();
    cut.push_str("...");
    cut
}

/// The label characters a row keeps beside a value too wide for both.
const LABEL_KEEPS: usize = 8;

/// A row line: the label on the left, the value right-aligned. A value that would leave the
/// label fewer than [`LABEL_KEEPS`] characters (or fewer than it has) is cut instead.
#[cfg(test)]
pub(super) fn columns(label: &str, value: &str) -> String {
    row_text(label, value).0
}

/// [`columns`] and how many characters at its end are the value.
pub(super) fn row_text(label: &str, value: &str) -> (String, usize) {
    let width = COLUMNS as usize;
    let keep = label.chars().count().min(LABEL_KEEPS);
    let room = if keep == 0 { width } else { width - keep - 1 };
    let value = fit(value, room);
    let value_len = value.chars().count();
    if value_len == 0 {
        return (fit(label, width), 0);
    }
    let label = fit(label, width.saturating_sub(value_len + 1));
    let gap = width.saturating_sub(value_len + label.chars().count());
    (format!("{label}{}{value}", " ".repeat(gap)), value_len)
}
