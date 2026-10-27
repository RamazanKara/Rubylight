//! `GET /bandwidth-probe`: paired clients only, bounded in size and rate, and never
//! while a stream runs or starts.
use super::*;
use crate::state::test_support::Fixture;
use axum::{
    body::{Body, to_bytes},
    http::{HeaderMap, Request},
};
use butterpollo_core::{bandwidth_probe as probe, session::Role};
use tower::ServiceExt;

async fn get(f: &Fixture, paired: bool, query: &str) -> (StatusCode, HeaderMap, Vec<u8>) {
    let response = router(f.host.clone(), true)
        .layer(Extension(f.connection(paired)))
        .oneshot(
            Request::builder()
                .uri(format!("{}{query}", probe::ROUTE))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let (parts, body) = response.into_parts();
    let body = to_bytes(body, 2 * probe::MAX_BYTES as usize).await.unwrap();
    (parts.status, parts.headers, body.to_vec())
}

#[tokio::test]
async fn a_paired_client_gets_the_bytes_it_asked_for_and_waits_between_probes() {
    let f = Fixture::new();
    let (status, ..) = get(&f, false, "?bytes=65536").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "an unpaired client");
    let client = f.client(u32::MAX);

    for query in ["", "?bytes=65535", "?bytes=4194305", "?bytes=lots"] {
        let (status, ..) = get(&f, true, query).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{query}");
    }

    let (status, headers, body) = get(&f, true, "?bytes=1048576").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.len(), 1 << 20);
    assert_eq!(headers[header::CONTENT_TYPE], "application/octet-stream");
    assert_eq!(headers[header::CACHE_CONTROL], "no-store");
    assert!(headers.get(header::CONTENT_LENGTH).is_none(), "streamed");
    // Random bytes: no compression on the way shrinks them.
    let zeros = body.iter().filter(|&&b| b == 0).count();
    assert!(zeros < body.len() / 128, "{zeros} zero bytes");

    let (status, headers, _) = get(&f, true, "?bytes=65536").await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "within the cooldown");
    let wait: u64 = headers[header::RETRY_AFTER]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!((1..=probe::COOLDOWN.as_secs()).contains(&wait), "{wait}");

    // A stream that runs or starts takes precedence, before the cooldown is checked.
    let session = f.session(client, Role::Stream);
    let (status, ..) = get(&f, true, "?bytes=65536").await;
    assert_eq!(status, StatusCode::CONFLICT);
    drop(session);
}

#[tokio::test]
async fn the_probe_is_gone_when_it_is_off() {
    let f = Fixture::new();
    f.client(u32::MAX);
    f.host
        .config
        .write()
        .unwrap()
        .values
        .insert("bandwidth_probe".into(), "false".into());
    let (status, ..) = get(&f, true, "?bytes=65536").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[test]
fn the_body_ends_at_its_size_or_its_deadline() {
    use tokio::io::AsyncReadExt;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    runtime.block_on(async {
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut full = vec![];
        let n = ProbeBody::new(probe::MAX_BYTES as usize, deadline, || false)
            .read_to_end(&mut full)
            .await
            .unwrap();
        assert_eq!(n, probe::MAX_BYTES as usize);
        let mut late = vec![];
        let n = ProbeBody::new(1 << 20, Instant::now(), || false)
            .read_to_end(&mut late)
            .await
            .unwrap();
        assert_eq!(n, 0, "past its deadline the body ends");
    });
}

#[test]
fn the_body_stops_when_a_stream_runs_or_starts() {
    use tokio::io::AsyncReadExt;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    runtime.block_on(async {
        let f = Fixture::new();
        let client = f.client(u32::MAX);
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut chunk = vec![0; 64 * 1024];

        // A launch that is still pending ends the body.
        let mut body = ProbeBody::new(probe::MAX_BYTES as usize, deadline, stream_starts(&f.host));
        assert_eq!(body.read(&mut chunk).await.unwrap(), chunk.len());
        let launch = f.launch(client.clone(), Role::Stream);
        f.host
            .sessions
            .lock()
            .unwrap()
            .pending
            .insert(launch.id.clone(), launch.clone());
        let mut rest = vec![];
        assert_eq!(body.read_to_end(&mut rest).await.unwrap(), 0);
        f.host.sessions.lock().unwrap().pending.remove(&launch.id);

        // So does a stream that is running.
        let mut body = ProbeBody::new(probe::MAX_BYTES as usize, deadline, stream_starts(&f.host));
        assert_eq!(body.read(&mut chunk).await.unwrap(), chunk.len());
        let session = f.session(client, Role::Stream);
        assert_eq!(body.read_to_end(&mut rest).await.unwrap(), 0);
        drop(session);
    });
}
