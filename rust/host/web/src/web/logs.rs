//! Log downloads and support diagnostics API handlers.

use super::error;
use axum::{
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use butterpollo_stream::state::Shared;
use serde_json::{Value, json};

/// Whether a support bundle request asks for the crash memory dump (`include_dump=1`): left out
/// unless asked for, since it holds whatever the host had in memory.
pub(super) fn include_dump(uri: &axum::http::Uri) -> bool {
    url::form_urlencoded::parse(uri.query().unwrap_or("").as_bytes())
        .any(|(key, value)| key == "include_dump" && matches!(value.as_ref(), "1" | "true"))
}

pub(super) fn handle(
    h: &Shared,
    method: &str,
    path: &str,
    data: &Value,
    include_dump: bool,
) -> anyhow::Result<Value> {
    Ok(match (method, path) {
        ("GET", "/api/health/crashdump") => butterpollo_stream::maintenance::crash_status(h)?,
        ("POST", "/api/health/crashdump/dismiss") => {
            butterpollo_stream::maintenance::dismiss_crash(h, data)?
        }
        ("GET", "/api/logs/export_crash/manifest") => {
            butterpollo_stream::maintenance::bundle_manifest(h, include_dump)
        }
        _ => return Err(anyhow::anyhow!("unknown API endpoint")),
    })
}

pub(super) async fn export_crash(h: &Shared, include_dump: bool) -> Response {
    let h = h.clone();
    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<std::fs::File> {
        use std::os::windows::fs::OpenOptionsExt;
        let path = butterpollo_stream::maintenance::bundle(&h, include_dump)?;
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
        let mut file = match std::fs::File::open(butterpollo_stream::maintenance::log_path(h)) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(
                    json!({"status":true,"offset":0,"size":0,"reset":offset != 0,"text":""}),
                );
            }
            Err(e) => return Err(e),
        };
        let size = file.metadata()?.len();
        let head = if size >= 64 {
            use sha2::Digest;
            let mut prefix = [0u8; 64];
            file.read_exact(&mut prefix)?;
            hex::encode(&sha2::Sha256::digest(prefix)[..16])
        } else {
            String::new()
        };
        // A negative offset asks for the tail; an offset past the end or a
        // changed file prefix (`head`) means the log was rotated.
        let rotated = !head.is_empty()
            && query
                .get("head")
                .is_some_and(|expected| !expected.is_empty() && expected != &head);
        let reset = offset < 0 || offset as u64 > size || rotated;
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
            "head": head,
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
    let path = butterpollo_stream::maintenance::log_path(h);
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
    let analysis = butterpollo_session::log_analysis::analyze(text);
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
        let mut file = std::fs::File::open(butterpollo_stream::maintenance::log_path(h))?;
        let length = file.metadata()?.len();
        file.seek(std::io::SeekFrom::Start(
            length.saturating_sub(8 * 1024 * 1024),
        ))?;
        let mut bytes = vec![];
        file.take(8 * 1024 * 1024).read_to_end(&mut bytes)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    })();
    let text = match result {
        Ok(text) => text,
        // No log yet reads as an empty one.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        // A log that cannot be read is an error, not an empty download.
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("the log could not be read: {e}"),
            )
                .into_response();
        }
    };
    let mut result = ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], text).into_response();
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
    fn only_an_explicit_request_includes_the_crash_memory_dump() {
        let asks = |uri: &str| super::include_dump(&uri.parse().unwrap());
        assert!(asks("/api/logs/export_crash?include_dump=1"));
        assert!(asks("/api/logs/export_crash?x=y&include_dump=true"));
        for uri in [
            "/api/logs/export_crash",
            "/api/logs/export_crash?include_dump=0",
            "/api/logs/export_crash?include_dump=",
            "/api/logs/export_crash?include_dumps=1",
        ] {
            assert!(!asks(uri), "{uri}");
        }
    }

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
        let hang = "2026-10-20T12:40:20.129623Z  WARN butterpollo_win_graphics::amf: slow encoder call call=\"AMF SubmitInput\" elapsed_ms=34787\n";
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

    #[test]
    fn tail_detects_log_rotation_even_when_new_file_grows_past_old_offset() {
        let f = butterpollo_stream::state::test_support::Fixture::new();
        std::fs::create_dir_all(&f.host.directory).unwrap();
        let path = butterpollo_stream::maintenance::log_path(&f.host);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let first_line = "2026-10-20T12:40:20.000001Z  INFO old_log: started session alpha with enough padding bytes\n";
        std::fs::write(&path, first_line).unwrap();
        let read_tail = |uri: &str| {
            let resp = super::tail(&f.host, &uri.parse().unwrap());
            let body = futures_block(resp.into_body());
            serde_json::from_slice::<serde_json::Value>(&body).unwrap()
        };
        let initial = read_tail("/api/logs/tail?offset=-1");
        let offset = initial["offset"].as_u64().unwrap();
        let head = initial["head"].as_str().unwrap().to_owned();
        assert!(!head.is_empty());

        // Rotate: new file has a different first line and already grew past `offset`.
        let rotated = format!(
            "2026-10-20T12:45:00.000002Z  INFO new_log: rotated file header line padded cleanly\n{}",
            "2026-10-20T12:45:01.000003Z  INFO new_log: extra line\n".repeat(4)
        );
        assert!(rotated.len() as u64 > offset);
        std::fs::write(&path, &rotated).unwrap();
        let after = read_tail(&format!("/api/logs/tail?offset={offset}&head={head}"));
        assert_eq!(after["reset"], true);
        assert_ne!(after["head"].as_str().unwrap(), head);
        assert!(after["text"].as_str().unwrap().contains("new_log"));
    }

    fn futures_block(body: axum::body::Body) -> Vec<u8> {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(axum::body::to_bytes(body, 1024 * 1024))
            .unwrap()
            .to_vec()
    }
}
