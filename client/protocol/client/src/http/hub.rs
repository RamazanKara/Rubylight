//! Host Hub Sync HTTP client (`/api/native/v1/hub/*`): conditional `GET /hub/snapshot`,
//! CAS `PUT`/`DELETE /hub/profiles/{id}` with `If-Match` & `X-Hub-Current-Revision`, and
//! conditional SHA-256 `GET /hub/covers/{app_id}` (`304 Not Modified`).

use super::native::error_of;
use super::native_api::PREFIX;
use super::native_api::error::{ApiError, ErrorBody, ErrorCode};
use super::native_api::hub::{
    HEADER_CURRENT_REVISION, HubSnapshot, ProfileDeleteResult, ProfileWrite, SyncProfile,
};
use super::transport::{self, Body, Request, Timeouts};
use super::{Error, HostClient, LONG_CONNECT_TIMEOUT, READ_TIMEOUT};

const SNAPSHOT_BODY_LIMIT: usize = 8 * 1024 * 1024;
const PROFILE_BODY_LIMIT: usize = 256 * 1024;
const COVER_BODY_LIMIT: usize = 16 * 1024 * 1024;

/// Result of `GET /api/native/v1/hub/snapshot`.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(
    clippy::large_enum_variant,
    reason = "keeps snapshot/profile payloads unboxed for callers"
)]
pub enum HubSnapshotResult {
    Modified {
        snapshot: HubSnapshot,
        etag: Option<String>,
    },
    NotModified,
}

/// Result of `PUT /api/native/v1/hub/profiles/{id}`.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(
    clippy::large_enum_variant,
    reason = "keeps snapshot/profile payloads unboxed for callers"
)]
pub enum HubPutResult {
    Saved {
        profile: SyncProfile,
        etag: Option<String>,
    },
    Conflict {
        error: Box<ApiError>,
        current_revision: Option<u64>,
    },
}

/// Result of `DELETE /api/native/v1/hub/profiles/{id}`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HubDeleteResult {
    Deleted(ProfileDeleteResult),
    Conflict {
        error: Box<ApiError>,
        current_revision: Option<u64>,
    },
}

/// Result of `GET /api/native/v1/hub/covers/{app_id}`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HubCoverResult {
    Image {
        etag: Option<String>,
        content_type: Option<String>,
        bytes: Vec<u8>,
    },
    NotModified,
}

fn validate_hub_id(id: &str) -> Result<(), Error> {
    let ok = !id.is_empty()
        && id.len() <= 64
        && id.bytes().enumerate().all(|(i, b)| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || (i > 0 && (b == b'-' || b == b'_'))
        });
    if ok {
        Ok(())
    } else {
        Err(Error::InvalidArgument(format!(
            "invalid hub profile id: {id}"
        )))
    }
}

fn format_if_match(rev: &str) -> String {
    let trimmed = rev.trim();
    if trimmed.starts_with('"') || trimmed.starts_with("W/") {
        trimmed.to_string()
    } else {
        format!("\"{trimmed}\"")
    }
}

fn parse_conflict(response: &transport::Response) -> Option<(Box<ApiError>, Option<u64>)> {
    if response.status != 409 {
        return None;
    }
    let current_revision = response
        .header(HEADER_CURRENT_REVISION)
        .and_then(|v| v.trim().parse::<u64>().ok());
    let body = serde_json::from_slice::<ErrorBody>(&response.body).ok()?;
    if body.error.code == ErrorCode::RevisionConflict || current_revision.is_some() {
        Some((Box::new(body.error), current_revision))
    } else {
        None
    }
}

impl HostClient {
    fn send_hub(
        &self,
        method: &str,
        subpath: &str,
        query: &str,
        extra_headers: &[(&str, &str)],
        body: Option<Body<'_>>,
        body_limit: usize,
    ) -> Result<(String, transport::Response), Error> {
        let path = format!("{}/{subpath}", PREFIX.trim_start_matches('/'));
        let target = self.url_target(&path, (!query.is_empty()).then_some(query));
        let request = Request {
            host: &self.host,
            port: self.https_port(true)?,
            target: &target,
            tls: Some(self.tls()?),
            timeouts: Timeouts {
                connect: LONG_CONNECT_TIMEOUT,
                read: Some(READ_TIMEOUT),
            },
            body_limit,
        };
        let response =
            transport::send_request(method, &request, &self.cancel, extra_headers, body)?;
        Ok((path, response))
    }

    /// `GET /api/native/v1/hub/snapshot` with optional `If-None-Match` and `?all_devices=1`.
    pub fn hub_snapshot(
        &self,
        if_none_match: Option<&str>,
        all_devices: bool,
    ) -> Result<HubSnapshotResult, Error> {
        let mut headers = Vec::with_capacity(1);
        if let Some(etag) = if_none_match {
            headers.push(("If-None-Match", etag));
        }
        let query = if all_devices { "all_devices=1" } else { "" };
        let (path, response) = self.send_hub(
            "GET",
            "hub/snapshot",
            query,
            &headers,
            None,
            SNAPSHOT_BODY_LIMIT,
        )?;
        if response.status == 304 {
            return Ok(HubSnapshotResult::NotModified);
        }
        if (200..=299).contains(&response.status) {
            let etag = response.header("ETag").map(str::to_owned);
            let snapshot: HubSnapshot = serde_json::from_slice(&response.body)
                .map_err(|e| Error::Malformed(format!("native API {path}: {e}")))?;
            return Ok(HubSnapshotResult::Modified { snapshot, etag });
        }
        Err(error_of(response, &path))
    }

    /// `PUT /api/native/v1/hub/profiles/{id}` with required `If-Match`.
    pub fn put_hub_profile(
        &self,
        id: &str,
        if_match: &str,
        write: &ProfileWrite,
    ) -> Result<HubPutResult, Error> {
        validate_hub_id(id)?;
        let if_match_val = format_if_match(if_match);
        let json_bytes = serde_json::to_vec(write)
            .map_err(|e| Error::InvalidArgument(format!("hub profile: {e}")))?;
        let subpath = format!("hub/profiles/{id}");
        let (path, response) = self.send_hub(
            "PUT",
            &subpath,
            "",
            &[("If-Match", &if_match_val)],
            Some(Body::json(&json_bytes)),
            PROFILE_BODY_LIMIT,
        )?;
        if (200..=299).contains(&response.status) {
            let etag = response.header("ETag").map(str::to_owned);
            let profile: SyncProfile = serde_json::from_slice(&response.body)
                .map_err(|e| Error::Malformed(format!("native API {path}: {e}")))?;
            return Ok(HubPutResult::Saved { profile, etag });
        }
        if let Some((error, current_revision)) = parse_conflict(&response) {
            return Ok(HubPutResult::Conflict {
                error,
                current_revision,
            });
        }
        Err(error_of(response, &path))
    }

    /// `DELETE /api/native/v1/hub/profiles/{id}` with required `If-Match`.
    pub fn delete_hub_profile(&self, id: &str, if_match: &str) -> Result<HubDeleteResult, Error> {
        validate_hub_id(id)?;
        let if_match_val = format_if_match(if_match);
        let subpath = format!("hub/profiles/{id}");
        let (path, response) = self.send_hub(
            "DELETE",
            &subpath,
            "",
            &[("If-Match", &if_match_val)],
            None,
            PROFILE_BODY_LIMIT,
        )?;
        if (200..=299).contains(&response.status) {
            let deleted: ProfileDeleteResult = serde_json::from_slice(&response.body)
                .map_err(|e| Error::Malformed(format!("native API {path}: {e}")))?;
            return Ok(HubDeleteResult::Deleted(deleted));
        }
        if let Some((error, current_revision)) = parse_conflict(&response) {
            return Ok(HubDeleteResult::Conflict {
                error,
                current_revision,
            });
        }
        Err(error_of(response, &path))
    }

    /// `GET /api/native/v1/hub/covers/{app_id}` with optional `If-None-Match`.
    pub fn hub_cover(
        &self,
        app_id: u32,
        if_none_match: Option<&str>,
    ) -> Result<HubCoverResult, Error> {
        let mut headers = Vec::with_capacity(1);
        if let Some(etag) = if_none_match {
            headers.push(("If-None-Match", etag));
        }
        let subpath = format!("hub/covers/{app_id}");
        let (path, response) =
            self.send_hub("GET", &subpath, "", &headers, None, COVER_BODY_LIMIT)?;
        if response.status == 304 {
            return Ok(HubCoverResult::NotModified);
        }
        if (200..=299).contains(&response.status) {
            let etag = response.header("ETag").map(str::to_owned);
            return Ok(HubCoverResult::Image {
                etag,
                content_type: response.content_type,
                bytes: response.body,
            });
        }
        Err(error_of(response, &path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::ClientIdentity;
    use crate::http::native_api::hub::{HubSnapshot, ProfileScope, SyncProfile};
    use rustls::pki_types::pem::PemObject;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Arc;

    fn loopback_https(
        replies: Vec<(&'static str, &'static str, Vec<u8>)>,
    ) -> (HostClient, std::thread::JoinHandle<Vec<String>>) {
        let host_id = ClientIdentity::generate().unwrap();
        let client_id = Arc::new(ClientIdentity::generate().unwrap());
        let cert = CertificateDer::from_pem_slice(host_id.certificate_pem().as_bytes()).unwrap();
        let key = PrivateKeyDer::from_pem_slice(host_id.private_key_pem().as_bytes()).unwrap();
        let tls_cfg = Arc::new(
            rustls::ServerConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![cert.clone()], key)
            .unwrap(),
        );
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            let mut seen_heads = Vec::new();
            for (status, extra_headers, body) in replies {
                let (sock, _) = listener.accept().unwrap();
                let conn = rustls::ServerConnection::new(tls_cfg.clone()).unwrap();
                let mut tls = rustls::StreamOwned::new(conn, sock);
                let mut req = Vec::new();
                while !req.ends_with(b"\r\n\r\n") {
                    let mut b = [0u8; 1];
                    tls.read_exact(&mut b).unwrap();
                    req.push(b[0]);
                }
                let head = String::from_utf8_lossy(&req).into_owned();
                if let Some(cl) = head
                    .lines()
                    .find_map(|l| {
                        l.split_once(':')
                            .filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                    })
                    .and_then(|(_, v)| v.trim().parse::<usize>().ok())
                    && cl > 0
                {
                    let mut buf = vec![0u8; cl];
                    tls.read_exact(&mut buf).unwrap();
                }
                seen_heads.push(head);
                let mut header = format!(
                    "HTTP/1.1 {status}\r\n{extra_headers}Content-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = tls.write_all(header.as_bytes());
                if !body.is_empty() {
                    let _ = tls.write_all(&body);
                }
                tls.conn.send_close_notify();
                let _ = tls.flush();
                header.clear();
            }
            seen_heads
        });
        let client = HostClient::new("127.0.0.1", 47989, client_id)
            .unwrap()
            .with_https_port(port);
        client.pin_server_certificate_der(Some(cert.as_ref().to_vec()));
        (client, handle)
    }

    #[test]
    fn hub_client_handles_304_put_409_delete_and_cover() {
        let snap = HubSnapshot {
            schema_version: 1,
            snapshot_revision: 5,
            ..Default::default()
        };
        let saved = SyncProfile {
            id: "couch".into(),
            revision: 6,
            scope: ProfileScope::Device,
            name: "Couch".into(),
            ..Default::default()
        };
        let conflict_json = br#"{"error":{"code":"revision_conflict","message":"Changed","setting":"couch","retry_after_ms":null}}"#.to_vec();
        let del_json = br#"{"deleted":true,"snapshot_revision":7}"#.to_vec();
        let (client, server) = loopback_https(vec![
            (
                "200 OK",
                "ETag: \"rev:5\"\r\nContent-Type: application/json\r\n",
                serde_json::to_vec(&snap).unwrap(),
            ),
            ("304 Not Modified", "ETag: \"rev:5\"\r\n", Vec::new()),
            (
                "409 Conflict",
                "Content-Type: application/json\r\nX-Hub-Current-Revision: 6\r\n",
                conflict_json,
            ),
            (
                "200 OK",
                "ETag: \"6\"\r\nContent-Type: application/json\r\n",
                serde_json::to_vec(&saved).unwrap(),
            ),
            ("200 OK", "Content-Type: application/json\r\n", del_json),
            (
                "200 OK",
                "ETag: \"sha256:abc\"\r\nContent-Type: image/png\r\n",
                b"png!".to_vec(),
            ),
            ("304 Not Modified", "ETag: \"sha256:abc\"\r\n", Vec::new()),
        ]);

        let r1 = client.hub_snapshot(None, false).unwrap();
        assert!(
            matches!(r1, HubSnapshotResult::Modified { snapshot, etag } if snapshot.snapshot_revision == 5 && etag.as_deref() == Some("\"rev:5\""))
        );

        let r2 = client.hub_snapshot(Some("\"rev:5\""), false).unwrap();
        assert_eq!(r2, HubSnapshotResult::NotModified);

        let write = ProfileWrite {
            scope: ProfileScope::Device,
            name: "Couch".into(),
            ..Default::default()
        };
        let r3 = client.put_hub_profile("couch", "0", &write).unwrap();
        assert!(matches!(
            r3,
            HubPutResult::Conflict {
                current_revision: Some(6),
                ..
            }
        ));

        let r4 = client.put_hub_profile("couch", "5", &write).unwrap();
        assert!(matches!(r4, HubPutResult::Saved { profile, .. } if profile.revision == 6));

        let r5 = client.delete_hub_profile("couch", "6").unwrap();
        assert!(
            matches!(r5, HubDeleteResult::Deleted(res) if res.deleted && res.snapshot_revision == 7)
        );

        let r6 = client.hub_cover(42, None).unwrap();
        assert!(
            matches!(r6, HubCoverResult::Image { etag, bytes, .. } if etag.as_deref() == Some("\"sha256:abc\"") && bytes == b"png!")
        );

        let r7 = client.hub_cover(42, Some("\"sha256:abc\"")).unwrap();
        assert_eq!(r7, HubCoverResult::NotModified);

        let seen = server.join().unwrap();
        assert!(seen[1].contains("If-None-Match: \"rev:5\""));
        assert!(seen[2].starts_with("PUT /api/native/v1/hub/profiles/couch?"));
        assert!(seen[2].contains("If-Match: \"0\""));
        assert!(seen[4].starts_with("DELETE /api/native/v1/hub/profiles/couch?"));
        assert!(seen[6].contains("If-None-Match: \"sha256:abc\""));
    }
}
