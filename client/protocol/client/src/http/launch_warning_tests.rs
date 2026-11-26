//! Hostile-input tests for the launch-warning parser.
use super::super::launch::parse_launch_reply;
use super::*;

const HEAD: &str = r#"<?xml version="1.0" encoding="utf-8"?><root status_code="200"><gamesession>1</gamesession><sessionUrl0>rtsp://h:48010</sessionUrl0>"#;

fn reply(extra: &str) -> String {
    format!("{HEAD}{extra}</root>")
}

fn warning(extra: &str) -> Option<LaunchWarning> {
    parse_launch_reply(&reply(extra), false)
        .expect("the launch itself succeeds")
        .expect("gamesession 1")
        .warning
}

fn known(message: Option<&str>) -> Option<LaunchWarning> {
    Some(LaunchWarning {
        code: VIRTUAL_DISPLAY_UNAVAILABLE.into(),
        message: message.map(Into::into),
    })
}

#[test]
fn a_code_and_message_are_read_and_escapes_decode() {
    assert_eq!(
        warning(
            "<clientWarning>virtual_display_unavailable</clientWarning><clientWarningMessage>Physical display &amp; more.</clientWarningMessage>"
        ),
        known(Some("Physical display & more."))
    );
    assert_eq!(
        warning("<clientWarning>virtual_display_unavailable</clientWarning>"),
        known(None)
    );
    assert_eq!(
        warning(
            "<clientWarning>some_future_code</clientWarning><clientWarningMessage>Shown as is.</clientWarningMessage>"
        ),
        Some(LaunchWarning {
            code: "some_future_code".into(),
            message: Some("Shown as is.".into())
        })
    );
    assert_eq!(warning(""), None, "no warning on an ordinary reply");
}

#[test]
fn orphan_duplicate_nested_and_malformed_tags_void_the_warning() {
    for extra in [
        "<clientWarningMessage>orphan</clientWarningMessage>",
        "<clientWarning>a</clientWarning><clientWarning>b</clientWarning>",
        "<clientWarning>a</clientWarning><clientWarningMessage>x</clientWarningMessage><clientWarningMessage>y</clientWarningMessage>",
        "<clientWarning><b>a</b></clientWarning>",
        "<clientWarning>a</clientWarning><clientWarningMessage><i>x</i></clientWarningMessage>",
        "<clientWarning>Not_A_Code</clientWarning>",
        "<clientWarning>a b</clientWarning>",
        "<clientWarning></clientWarning>",
        "<nested><clientWarning>a</clientWarning></nested>",
    ] {
        assert_eq!(warning(extra), None, "{extra}");
    }
    let long = format!(
        "<clientWarning>{}</clientWarning>",
        "a".repeat(MAX_CODE_BYTES + 1)
    );
    assert_eq!(warning(&long), None);
}

#[test]
fn an_unusable_message_keeps_the_code_without_text() {
    for message in [
        "&lt;script&gt;alert(1)&lt;/script&gt;",
        "a &gt; b",
        " \t ",
        "&#x200B;",
    ] {
        assert_eq!(
            warning(&format!(
                "<clientWarning>virtual_display_unavailable</clientWarning><clientWarningMessage>{message}</clientWarningMessage>"
            )),
            known(None),
            "{message}"
        );
    }
    let too_long = "x".repeat(MAX_MESSAGE_BYTES + 1);
    assert_eq!(
        warning(&format!(
            "<clientWarning>virtual_display_unavailable</clientWarning><clientWarningMessage>{too_long}</clientWarningMessage>"
        )),
        known(None)
    );
}

#[test]
fn oversized_replies_and_dtds_carry_no_warning_but_still_launch() {
    let tag = "<clientWarning>virtual_display_unavailable</clientWarning>";
    let padded = reply(&format!("{tag}<pad>{}</pad>", "x".repeat(MAX_REPLY_BYTES)));
    let launched = parse_launch_reply(&padded, false).unwrap().unwrap();
    assert_eq!(launched.warning, None);
    assert_eq!(launched.rtsp_session_url.as_deref(), Some("rtsp://h:48010"));
    let dtd = format!(
        r#"<?xml version="1.0"?><!DOCTYPE root [<!ENTITY x "boom">]><root status_code="200"><gamesession>1</gamesession>{tag}</root>"#
    );
    if let Ok(Some(launched)) = parse_launch_reply(&dtd, false) {
        assert_eq!(launched.warning, None);
    }
}

#[test]
fn failed_and_declined_replies_never_warn() {
    let declined = format!(
        r#"<root status_code="200"><gamesession>0</gamesession><clientWarning>virtual_display_unavailable</clientWarning></root>"#
    );
    assert_eq!(parse_launch_reply(&declined, false).unwrap(), None);
    let failed = r#"<root status_code="503" status_message="no"><clientWarning>virtual_display_unavailable</clientWarning></root>"#;
    assert!(parse_launch_reply(failed, false).is_err());
    let resumed = r#"<root status_code="200"><resume>1</resume><clientWarning>display_scale</clientWarning></root>"#;
    assert_eq!(
        parse_launch_reply(resumed, true)
            .unwrap()
            .unwrap()
            .warning
            .unwrap()
            .code,
        "display_scale"
    );
}

#[test]
fn text_cleaning_matches_the_host() {
    assert_eq!(
        message("a\u{0}b\u{7}c\u{1B}[31md").as_deref(),
        Some("abc[31md")
    );
    assert_eq!(
        message("rtl\u{202E}gnp.exe\u{2066}x\u{FEFF}y\u{AD}z").as_deref(),
        Some("rtlgnp.exexyz")
    );
    assert_eq!(
        message("\u{E0041}tag\u{110BD}\u{1D173}").as_deref(),
        Some("tag")
    );
    assert_eq!(
        message("zw\u{200D}j zw\u{200C}nj").as_deref(),
        Some("zw\u{200D}j zw\u{200C}nj")
    );
    assert_eq!(
        message("1\r\n2\r3\u{2028}4\u{2029}5\n6\t7").as_deref(),
        Some("1\n2\n3\n4 5 6 7")
    );
    let limit = "é".repeat(MAX_MESSAGE_BYTES / 2);
    assert_eq!(message(&limit).as_deref(), Some(limit.as_str()));
    assert_eq!(message(&format!("{limit}x")), None);
    assert_eq!(message(&"x".repeat(1 << 20)), None);
    for c in ['a', 'é', 'ア', '😀', '\u{200C}', '\u{200D}'] {
        assert!(!is_control_or_format(c), "{c:?}");
    }
}
