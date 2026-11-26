//! The optional `title`, `next_step` and `technical_detail` of an error body:
//! a host that sends them and a client that reads them keep working with the
//! hosts and clients from before them, in both directions.
use rubylight_native_api::error::{ApiError, ErrorBody, ErrorCode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// `ApiError` as v1 clients before the three fields declare it.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct OldApiError {
    code: ErrorCode,
    message: String,
    setting: Option<String>,
    retry_after_ms: Option<u64>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct OldErrorBody {
    error: OldApiError,
}

fn explained() -> ErrorBody {
    let mut error = ApiError::new(
        ErrorCode::DriverRejected,
        "The driver refused anti_lag.enabled.",
    )
    .for_setting("anti_lag.enabled");
    error.title = Some("The graphics driver refused the change".into());
    error.next_step = Some("Choose another value, or update the graphics driver on the PC.".into());
    error.technical_detail = Some("driver_rejected (anti_lag.enabled): ADLX_FAIL".into());
    ErrorBody { error }
}

/// The example of `docs/native-api.md`, "Errors", parsed and written back unchanged.
#[test]
fn the_documented_example_round_trips() {
    let path = format!(
        "{}/tests/fixtures/error_explained.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains('\r'), "{path} must use LF");
    let expected: Value = serde_json::from_str(&text).unwrap();
    let body: ErrorBody = serde_json::from_str(&text).unwrap();
    assert_eq!(serde_json::to_value(&body).unwrap(), expected);
    assert_eq!(
        body.error.title.as_deref(),
        Some("The graphics driver refused the change")
    );
    assert!(body.error.next_step.is_some() && body.error.technical_detail.is_some());
}

#[test]
fn an_explained_error_round_trips_with_its_fields() {
    let body = explained();
    let wire = serde_json::to_value(&body).unwrap();
    assert_eq!(
        wire,
        json!({ "error": {
            "code": "driver_rejected",
            "message": "The driver refused anti_lag.enabled.",
            "setting": "anti_lag.enabled",
            "retry_after_ms": null,
            "title": "The graphics driver refused the change",
            "next_step": "Choose another value, or update the graphics driver on the PC.",
            "technical_detail": "driver_rejected (anti_lag.enabled): ADLX_FAIL",
        } })
    );
    assert_eq!(serde_json::from_value::<ErrorBody>(wire).unwrap(), body);
}

#[test]
fn an_error_without_them_leaves_them_off_the_wire() {
    let body = ErrorBody {
        error: ApiError::new(ErrorCode::SessionBusy, "A launch is in progress.").retry_after(1000),
    };
    let wire = serde_json::to_value(&body).unwrap();
    let mut fields: Vec<&str> = wire["error"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    fields.sort_unstable();
    assert_eq!(fields, ["code", "message", "retry_after_ms", "setting"]);
    assert_eq!(
        wire["error"]["setting"],
        Value::Null,
        "null on purpose, as before"
    );
    assert_eq!(wire["error"]["retry_after_ms"], 1000);
}

#[test]
fn an_old_client_reads_a_new_error() {
    let text = serde_json::to_string(&explained()).unwrap();
    let old: OldErrorBody = serde_json::from_str(&text).unwrap();
    assert_eq!(
        old,
        OldErrorBody {
            error: OldApiError {
                code: ErrorCode::DriverRejected,
                message: "The driver refused anti_lag.enabled.".into(),
                setting: Some("anti_lag.enabled".into()),
                retry_after_ms: None,
            }
        }
    );
}

#[test]
fn a_new_client_reads_an_old_error() {
    let old = OldErrorBody {
        error: OldApiError {
            code: ErrorCode::HostInUse,
            message: "TV is streaming from this PC.".into(),
            setting: None,
            retry_after_ms: None,
        },
    };
    let body: ErrorBody = serde_json::from_str(&serde_json::to_string(&old).unwrap()).unwrap();
    assert_eq!(
        body.error,
        ApiError::new(ErrorCode::HostInUse, "TV is streaming from this PC.")
    );
    assert_eq!(body.error.title, None);
    assert_eq!(body.error.next_step, None);
    assert_eq!(body.error.technical_detail, None);
    // The fallback body the host writes by hand when serialising fails.
    let fallback = r#"{"error":{"code":"internal","message":"The host failed to build the answer.","setting":null,"retry_after_ms":null}}"#;
    let body: ErrorBody = serde_json::from_str(fallback).unwrap();
    assert_eq!(
        body.error,
        ApiError::new(ErrorCode::Internal, "The host failed to build the answer.")
    );
}
