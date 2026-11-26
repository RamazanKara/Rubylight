//! The catalog's words stay plain English, and the cause a caller knows wins.
use super::*;

/// Words a player can't act on: hex codes, HRESULT and Windows error names, error numbers.
fn technical(text: &str) -> Option<&'static str> {
    let lower = text.to_ascii_lowercase();
    [
        "0x",
        "hresult",
        "os error",
        "error code",
        "amf_",
        "dxgi_",
        "adlx",
        "e_fail",
        "nvapi",
    ]
    .into_iter()
    .find(|token| lower.contains(token))
}

const EVERY_CODE: &[ErrorCode] = &[
    ErrorCode::NotPaired,
    ErrorCode::PermissionRequired,
    ErrorCode::HostControlRequired,
    ErrorCode::NotFound,
    ErrorCode::InvalidRequest,
    ErrorCode::InvalidValue,
    ErrorCode::NotWritable,
    ErrorCode::NoSession,
    ErrorCode::NotSessionOwner,
    ErrorCode::SessionBusy,
    ErrorCode::DriverRejected,
    ErrorCode::TemporarilyUnavailable,
    ErrorCode::Internal,
    ErrorCode::NotStreaming,
    ErrorCode::Forbidden,
    ErrorCode::FileTransferDisabled,
    ErrorCode::InvalidName,
    ErrorCode::IncompleteUpload,
    ErrorCode::LengthRequired,
    ErrorCode::TooLarge,
    ErrorCode::InsufficientStorage,
    ErrorCode::NoConsoleUser,
    ErrorCode::TransferBusy,
    ErrorCode::UsbDisabled,
    ErrorCode::UsbLimit,
    ErrorCode::FoldersDisabled,
    ErrorCode::FolderUnreadable,
    ErrorCode::CoopRefused,
    ErrorCode::HostInUse,
    ErrorCode::TokenExpired,
    ErrorCode::UpdateUnavailable,
    ErrorCode::WrongPin,
    ErrorCode::PairingUnconfirmed,
    ErrorCode::PairingEnded,
    ErrorCode::PairingCooldown,
    ErrorCode::Unknown,
];

fn assert_plain(what: &str, explanation: Explanation) {
    for text in [Some(explanation.title), explanation.next_step]
        .into_iter()
        .flatten()
    {
        assert!(!text.trim().is_empty(), "{what}: empty text");
        assert_eq!(technical(text), None, "{what}: {text:?}");
    }
}

#[test]
fn every_known_cause_reads_as_plain_english() {
    for code in EVERY_CODE {
        if let Some(explanation) = for_code(*code) {
            assert_plain(&format!("{code:?}"), explanation);
        }
    }
    for rule in log_analysis::RULES {
        assert_plain(
            rule.id,
            Explanation {
                title: rule.title,
                next_step: Some(rule.fix),
            },
        );
    }
}

#[test]
fn the_causes_a_player_meets_have_a_title_and_client_bugs_none() {
    for code in [
        ErrorCode::DriverRejected,
        ErrorCode::HostInUse,
        ErrorCode::NoConsoleUser,
        ErrorCode::PairingCooldown,
        ErrorCode::InsufficientStorage,
        ErrorCode::TemporarilyUnavailable,
        ErrorCode::Internal,
    ] {
        assert!(for_code(code).is_some(), "{code:?}");
    }
    for code in [
        ErrorCode::NotFound,
        ErrorCode::InvalidRequest,
        ErrorCode::LengthRequired,
        ErrorCode::Unknown,
    ] {
        assert_eq!(
            explain(ApiError::new(code, "words")).title,
            None,
            "{code:?}"
        );
    }
}

#[test]
fn a_driver_refusal_keeps_its_codes_out_of_the_words() {
    let mut error = ApiError::new(
        ErrorCode::DriverRejected,
        "wrote 3 and the driver reads back 1",
    )
    .for_setting("anti_lag.level");
    error.technical_detail = Some(detail(
        "driver_rejected (anti_lag.level): ADLX_FAIL 0x80004005",
    ));
    let error = explain(error);
    assert_eq!(
        error.title.as_deref(),
        Some("The graphics driver refused the change")
    );
    let next_step = error.next_step.as_deref().unwrap();
    assert_eq!(technical(error.title.as_deref().unwrap()), None);
    assert_eq!(technical(next_step), None, "{next_step}");
    assert_eq!(
        error.technical_detail.as_deref(),
        Some("driver_rejected (anti_lag.level): ADLX_FAIL 0x80004005")
    );
    assert_eq!(error.message, "wrote 3 and the driver reads back 1");
}

#[test]
fn a_detail_with_a_known_failure_takes_the_rules_words() {
    let mut error = ApiError::new(ErrorCode::Internal, "The host failed to start the stream.");
    error.technical_detail = Some("GPU device lost: DXGI_ERROR_DEVICE_RESET (0x887A0007)".into());
    let error = explain(error);
    let rule = log_analysis::RULES
        .iter()
        .find(|rule| rule.id == "gpu_reset")
        .unwrap();
    assert_eq!(error.title.as_deref(), Some(rule.title));
    assert_eq!(error.next_step.as_deref(), Some(rule.fix));
    assert_eq!(technical(rule.title), None);
}

#[test]
fn the_callers_own_words_win() {
    let mut error = ApiError::new(ErrorCode::Internal, "Rubylight can't reach your files.");
    error.title = Some("Rubylight can't reach your files".into());
    let error = explain(error);
    assert_eq!(
        error.title.as_deref(),
        Some("Rubylight can't reach your files")
    );
    assert_eq!(
        error.next_step, None,
        "no generic next step under a caller's title"
    );
}

#[test]
fn a_detail_is_redacted_and_cut() {
    let text = detail(r"open C:\Users\Alice\AppData\Local\x.json failed from 192.168.4.20");
    assert!(!text.contains("Alice"), "{text}");
    assert!(!text.contains("192.168.4.20"), "{text}");
    let long = detail(&"x".repeat(DETAIL_CHARS * 2));
    assert_eq!(long.chars().count(), DETAIL_CHARS + 1);
    assert!(long.ends_with('…'));
    assert_eq!(detail("  short  "), "short");
}
