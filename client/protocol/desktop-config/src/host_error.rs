//! The PC's plain-English parts of a native API error (`title`, `next_step`,
//! `technical_detail`) around the words an error dialog already shows.
//!
//! A host that sends none of them (an older Rubylight, Sunshine, Apollo, GameStream) leaves the
//! dialog's words exactly as they were: the parts are the gate, never the host's brand.
use crate::strings::trf;
use rubylight_native_api::error::ApiError;

/// `lead` with the host's `title` above it and its `next_step` and `technical_detail` below,
/// each its own paragraph; `lead` alone when the host sent none of them.
pub fn compose(title: Option<&str>, lead: &str, next_step: Option<&str>, technical_detail: Option<&str>) -> String {
    fn given(part: Option<&str>) -> Option<&str> {
        part.map(str::trim).filter(|p| !p.is_empty())
    }
    let mut parts: Vec<String> = Vec::with_capacity(4);
    if let Some(title) = given(title).filter(|t| *t != lead.trim()) {
        parts.push(title.to_owned());
    }
    if !lead.is_empty() {
        parts.push(lead.to_owned());
    }
    if let Some(next_step) = given(next_step) {
        parts.push(trf("What to do: {}", &[&next_step]));
    }
    if let Some(detail) = given(technical_detail) {
        parts.push(trf("Technical details: {}", &[&detail]));
    }
    parts.join("\n\n")
}

/// The host's own message with its title, next step and technical detail: for a dialog that
/// shows the PC's words as they came.
pub fn host_text(error: &ApiError) -> String {
    compose(error.title.as_deref(), &error.message, error.next_step.as_deref(), error.technical_detail.as_deref())
}

/// Nightjar's own words for an error (chosen by its code) with the host's next step and
/// technical detail; the host's title would only repeat them.
pub fn with_host_help(lead: String, error: &ApiError) -> String {
    match (&error.next_step, &error.technical_detail) {
        (None, None) => lead,
        _ => compose(None, &lead, error.next_step.as_deref(), error.technical_detail.as_deref()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strings::{Lang, set};
    use rubylight_native_api::error::ErrorCode;

    fn explained() -> ApiError {
        let mut error = ApiError::new(ErrorCode::DriverRejected, "wrote 3 and the driver reads back 1");
        error.title = Some("The graphics driver refused the change".into());
        error.next_step = Some("Choose another value.".into());
        error.technical_detail = Some("driver_rejected (anti_lag.level): ADLX_FAIL".into());
        error
    }

    #[test]
    fn an_error_without_the_parts_reads_as_before() {
        set(Lang::En);
        let old = ApiError::new(ErrorCode::HostInUse, "TV is streaming from this PC.");
        assert_eq!(host_text(&old), "TV is streaming from this PC.");
        assert_eq!(with_host_help("The PC is busy.".into(), &old), "The PC is busy.");
        assert_eq!(compose(Some(" "), "words", Some(""), None), "words");
    }

    #[test]
    fn the_parts_come_in_order_each_on_its_own() {
        set(Lang::En);
        assert_eq!(
            host_text(&explained()),
            "The graphics driver refused the change\n\nwrote 3 and the driver reads back 1\n\n\
             What to do: Choose another value.\n\nTechnical details: driver_rejected (anti_lag.level): ADLX_FAIL"
        );
        assert_eq!(
            with_host_help("The graphics driver rejected this change.".into(), &explained()),
            "The graphics driver rejected this change.\n\nWhat to do: Choose another value.\n\n\
             Technical details: driver_rejected (anti_lag.level): ADLX_FAIL"
        );
    }

    #[test]
    fn a_title_that_is_the_message_or_an_empty_message_is_not_doubled() {
        set(Lang::En);
        assert_eq!(compose(Some("The PC is in use"), "The PC is in use", None, None), "The PC is in use");
        assert_eq!(compose(Some("Too many wrong PINs"), "", Some("Pair it on the PC."), None), "Too many wrong PINs\n\nWhat to do: Pair it on the PC.");
    }

    #[test]
    fn the_labels_are_translated() {
        set(Lang::De);
        let text = host_text(&explained());
        assert!(!text.contains("What to do"), "{text}");
        assert!(!text.contains("Technical details"), "{text}");
        assert!(text.contains("Choose another value.") && text.contains("ADLX_FAIL"), "{text}");
    }
}
