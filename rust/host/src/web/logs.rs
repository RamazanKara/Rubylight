//! Log downloads and support diagnostics API handlers.

use super::error;
use crate::state::Shared;
use axum::{
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};

pub(super) fn handle(h: &Shared, method: &str, path: &str, data: &Value) -> anyhow::Result<Value> {
    Ok(match (method, path) {
        ("GET", "/api/health/crashdump") => crate::maintenance::crash_status(h)?,
        ("POST", "/api/health/crashdump/dismiss") => crate::maintenance::dismiss_crash(h, data)?,
        ("GET", "/api/logs/export_crash/manifest") => crate::maintenance::bundle_manifest(h),
        _ => return Err(anyhow::anyhow!("unknown API endpoint")),
    })
}

pub(super) async fn export_crash(h: &Shared) -> Response {
    let h = h.clone();
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<std::fs::File> {
        use std::os::windows::fs::OpenOptionsExt;
        let path = crate::maintenance::bundle(&h)?;
        Ok(std::fs::OpenOptions::new()
            .access_mode(0x80000000 | 0x00010000)
            .share_mode(1 | 4)
            .custom_flags(0x04000000)
            .open(path)?)
    })
    .await;
    match result {
        Ok(Ok(file)) => (
            [
                (header::CONTENT_TYPE, "application/zip"),
                (
                    header::CONTENT_DISPOSITION,
                    "attachment; filename=butterpollo-support.zip",
                ),
            ],
            axum::body::Body::from_stream(tokio_util::io::ReaderStream::new(
                tokio::fs::File::from_std(file),
            )),
        )
            .into_response(),
        Ok(Err(err)) => error(StatusCode::INTERNAL_SERVER_ERROR, &err.to_string()),
        Err(err) => error(StatusCode::INTERNAL_SERVER_ERROR, &err.to_string()),
    }
}

pub(super) fn tail(h: &Shared, uri: &axum::http::Uri) -> Response {
    use std::io::{Read, Seek};
    let query: std::collections::HashMap<String, String> =
        url::form_urlencoded::parse(uri.query().unwrap_or("").as_bytes())
            .into_owned()
            .collect();
    let offset: i64 = query
        .get("offset")
        .and_then(|v| v.parse().ok())
        .unwrap_or(-1);
    let max: u64 = query
        .get("max")
        .and_then(|v| v.parse().ok())
        .unwrap_or(256 * 1024)
        .clamp(1024, 4 * 1024 * 1024);
    let result = (|| -> std::io::Result<Value> {
        let mut file = match std::fs::File::open(crate::maintenance::log_path(h)) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(
                    json!({"status":true,"offset":0,"size":0,"reset":offset != 0,"text":""}),
                );
            }
            Err(e) => return Err(e),
        };
        let size = file.metadata()?.len();
        // A negative offset asks for the tail; an offset past the end
        // means the log was rotated.
        let reset = offset < 0 || offset as u64 > size;
        let start = if reset {
            size.saturating_sub(max)
        } else {
            offset as u64
        };
        file.seek(std::io::SeekFrom::Start(start))?;
        let mut bytes = vec![];
        file.take(max).read_to_end(&mut bytes)?;
        // End on a line break so no line is split between two reads.
        if start + (bytes.len() as u64) < size
            && let Some(end) = bytes.iter().rposition(|b| *b == b'\n')
        {
            bytes.truncate(end + 1);
        }
        Ok(json!({
            "status": true,
            "offset": start + bytes.len() as u64,
            "size": size,
            "reset": reset,
            "text": String::from_utf8_lossy(&bytes),
        }))
    })();
    match result {
        Ok(value) => axum::Json(value).into_response(),
        Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

/// How much of the log's end the analysis reads, as much as the export holds.
const ANALYSIS_BYTES: u64 = 8 * 1024 * 1024;

/// The log analysis findings: what failed recently, in plain language.
pub(super) async fn analysis(h: &Shared) -> Response {
    let path = crate::maintenance::log_path(h);
    let result = tokio::task::spawn_blocking(move || analyze_file(&path, ANALYSIS_BYTES)).await;
    match result {
        Ok(Ok(value)) => axum::Json(value).into_response(),
        Ok(Err(err)) => error(StatusCode::INTERNAL_SERVER_ERROR, &err.to_string()),
        Err(err) => error(StatusCode::INTERNAL_SERVER_ERROR, &err.to_string()),
    }
}

fn analyze_file(path: &std::path::Path, max: u64) -> std::io::Result<Value> {
    use std::io::{Read, Seek};
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        // A host that has not written its log yet has nothing to report.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(
                json!({"status":true,"findings":[],"since":null,"until":null,"read_bytes":0}),
            );
        }
        Err(e) => return Err(e),
    };
    let start = file.metadata()?.len().saturating_sub(max);
    file.seek(std::io::SeekFrom::Start(start))?;
    let mut bytes = vec![];
    file.take(max).read_to_end(&mut bytes)?;
    // Skip the line the window cuts into.
    let text = String::from_utf8_lossy(&bytes);
    let text = match (start, text.find('\n')) {
        (0, _) => &text[..],
        (_, Some(end)) => &text[end + 1..],
        (_, None) => "",
    };
    let analysis = butterpollo_core::log_analysis::analyze(text);
    Ok(json!({
        "status": true,
        "findings": analysis.findings,
        "since": analysis.since,
        "until": analysis.until,
        "read_bytes": bytes.len(),
    }))
}

pub(super) fn export(h: &Shared, path: &str) -> Response {
    use std::io::{Read, Seek};
    let result = (|| -> std::io::Result<String> {
        let mut file = std::fs::File::open(crate::maintenance::log_path(h))?;
        let length = file.metadata()?.len();
        file.seek(std::io::SeekFrom::Start(
            length.saturating_sub(8 * 1024 * 1024),
        ))?;
        let mut bytes = vec![];
        file.take(8 * 1024 * 1024).read_to_end(&mut bytes)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    })();
    let mut result = (
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        result.unwrap_or_default(),
    )
        .into_response();
    if path.ends_with("/export") {
        result.headers_mut().insert(
            header::CONTENT_DISPOSITION,
            "attachment; filename=butterpollo.log".parse().unwrap(),
        );
    }
    result
}

#[cfg(test)]
mod tests {
    use super::analyze_file;

    #[test]
    fn the_analysis_explains_the_logs_end_and_skips_the_cut_line() {
        let dir =
            std::env::temp_dir().join(format!("butterpollo-analysis-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("butterpollo.log");
        assert_eq!(
            analyze_file(&path, 1 << 20).unwrap()["findings"],
            serde_json::json!([])
        );
        let hang = "2026-10-20T12:40:20.129623Z  WARN butterpollo_windows::amf: slow encoder call call=\"AMF SubmitInput\" elapsed_ms=34787\n";
        let timeout = "2026-10-20T12:41:00.000000Z  WARN butterpollo::stream: client control stream timed out client=SM-F946B\n";
        std::fs::write(&path, format!("{hang}{hang}{timeout}")).unwrap();
        let all = analyze_file(&path, 1 << 20).unwrap();
        let ids: Vec<_> = all["findings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| {
                (
                    f["id"].as_str().unwrap().to_owned(),
                    f["count"].as_u64().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            ids,
            [
                ("client_timeout".to_owned(), 1),
                ("encoder_hang".to_owned(), 2)
            ]
        );
        assert_eq!(all["since"], "2026-10-20T12:40:20.129623Z");
        let link = all["findings"][1]["link"].as_str().unwrap();
        assert!(
            link.ends_with(
                "troubleshooting.md#the-picture-freezes-and-the-gpu-resets-during-a-stream"
            ),
            "{link}"
        );
        // A window that starts inside the second hang line counts only the whole lines after it.
        let cut = analyze_file(&path, (hang.len() + timeout.len() - 10) as u64).unwrap();
        assert_eq!(cut["findings"].as_array().unwrap().len(), 1);
        assert_eq!(cut["findings"][0]["id"], "client_timeout");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
