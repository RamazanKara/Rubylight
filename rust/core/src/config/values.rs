//! Reading setting values as text, and reporting the ones the host refuses.

/// Report a setting value the host does not understand and will not use.
/// Each key and value is reported once.
pub fn invalid(key: &str, value: &str) {
    if first_report(key, value) {
        tracing::warn!(key, value, "unsupported setting value; using the default");
    }
}
/// [`invalid`], with the parse error that refused the value.
pub fn refused(key: &str, value: &str, error: &str) {
    if first_report(key, value) {
        tracing::warn!(
            key,
            value,
            error,
            "unsupported setting value; using the default"
        );
    }
}
pub(super) fn first_report(key: &str, value: &str) -> bool {
    static SEEN: std::sync::Mutex<std::collections::BTreeSet<(String, String)>> =
        std::sync::Mutex::new(std::collections::BTreeSet::new());
    SEEN.lock()
        .unwrap()
        .insert((key.to_owned(), value.to_owned()))
}
/// Report `value` unless it is the default the caller falls back to.
pub fn fallback(key: &str, value: &str, default: &str) {
    if value != default {
        invalid(key, value);
    }
}
pub(super) fn unquote(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(value)
}
/// A number setting's text as the host parses it: Vibepollo also writes
/// numbers in quotes (`"30"`), and the console shows them as numbers.
pub fn number_text(value: &str) -> &str {
    unquote(value.trim()).trim()
}
/// A boolean setting's text as the host reads it, `None` when it is neither.
pub fn parse_boolean(value: &str) -> Option<bool> {
    match unquote(value.trim()).to_ascii_lowercase().as_str() {
        "true" | "yes" | "1" | "enable" | "enabled" | "on" => Some(true),
        "false" | "no" | "0" | "disable" | "disabled" | "off" => Some(false),
        _ => None,
    }
}
pub fn parse_integer(value: &str) -> Option<i64> {
    let value = number_text(value);
    if let Some(hex) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        return i64::from_str_radix(hex, 16).ok();
    }
    value.parse().ok()
}
