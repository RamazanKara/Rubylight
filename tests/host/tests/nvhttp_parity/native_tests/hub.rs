//! Host Hub Sync native API route tests (`GET /hub/snapshot`, `PUT`/`DELETE /hub/profiles/{id}`,
//! `GET /hub/covers/{app_id}`): cross-device isolation, `host_control` `?all_devices=1`,
//! `#screen2` normalization, 16-64 KiB body pass, >64 KiB & >1 MiB JSON `413 too_large`,
//! `If-Match` `409 revision_conflict` + `X-Hub-Current-Revision`, `304 Not Modified`, and restart
//! persistence.
use super::*;
use rubylight_native_api::hub::{
    HEADER_CURRENT_REVISION, HubSnapshot, ProfileDeleteResult, ProfileScope, SyncProfile,
};

async fn send_with_headers(
    f: &Fixture,
    connection: &Connection,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
) -> Reply {
    let mut request = Request::builder()
        .method(method)
        .uri(format!("{PREFIX}{path}"));
    for (k, v) in headers {
        request = request.header(*k, *v);
    }
    let response = nvhttp::router_with(f.host.clone(), true, Some(native_api::router()))
        .layer(Extension(connection.clone()))
        .layer(Extension(butterpollo_stream::tls::ConnectionEnd::default()))
        .oneshot(request.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let (parts, body) = response.into_parts();
    Reply {
        status: parts.status,
        headers: parts.headers,
        body: to_bytes(body, 2 << 20).await.unwrap().to_vec(),
    }
}

fn png_bytes(w: u32, h: u32) -> Vec<u8> {
    let mut png = vec![0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
    png.extend_from_slice(&13u32.to_be_bytes());
    png.extend_from_slice(b"IHDR");
    png.extend_from_slice(&w.to_be_bytes());
    png.extend_from_slice(&h.to_be_bytes());
    png.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
    png
}

fn add_device(f: &Fixture, uuid: &str, perm: u32, host_control: bool) -> Connection {
    let identity = Identity::generate().unwrap();
    let mut client = Client {
        name: format!("Device {uuid}"),
        cert: identity.certificate.clone(),
        uuid: uuid.into(),
        perm,
        enabled: true,
        extra: Default::default(),
    };
    if host_control {
        client.extra.insert("host_control".into(), true.into());
    }
    f.host
        .paired
        .write()
        .unwrap()
        .add(&f.host.paired_path, client)
        .unwrap();
    Connection {
        certificate: Some(identity.der.clone()),
        ..f.connection(true)
    }
}

#[tokio::test]
async fn hub_capabilities_scoping_and_all_devices_isolation() {
    let s = Setup::new();
    let caps: Capabilities = serde_json::from_slice(&s.get("/capabilities").await.body).unwrap();
    assert!(caps.features.iter().any(|f| f == "hub.sync"));

    let dev_b = add_device(&s.f, "device-b", ALL, false);
    let dev_admin = add_device(&s.f, "device-admin", ALL, true);

    // Device A creates a private device profile.
    let body_a = serde_json::to_vec(&json!({
        "scope": "device",
        "name": "Device A Couch",
        "stream": { "width": 1280, "height": 800, "fps": 90, "resolution_scale_pct": 85 }
    }))
    .unwrap();
    let created = send_with_headers(
        &s.f,
        &s.connection,
        "PUT",
        "/hub/profiles/couch-a",
        &[("Content-Type", "application/json"), ("If-Match", "\"0\"")],
        body_a.clone(),
    )
    .await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());
    let profile_a: SyncProfile = serde_json::from_slice(&created.body).unwrap();
    assert_eq!(profile_a.revision, 1);
    assert_eq!(profile_a.stream.resolution_scale_pct, Some(85));

    // Device B cannot mutate or delete Device A's profile (403 host_control_required).
    send_with_headers(
        &s.f,
        &dev_b,
        "PUT",
        "/hub/profiles/couch-a",
        &[("Content-Type", "application/json"), ("If-Match", "\"1\"")],
        body_a.clone(),
    )
    .await
    .assert_error(403, "host_control_required");

    send_with_headers(
        &s.f,
        &dev_b,
        "DELETE",
        "/hub/profiles/couch-a",
        &[("If-Match", "\"1\"")],
        vec![],
    )
    .await
    .assert_error(403, "host_control_required");

    // Device B cannot see Device A's profile or request ?all_devices=1.
    let snap_b: HubSnapshot = serde_json::from_slice(
        &send_with_headers(&s.f, &dev_b, "GET", "/hub/snapshot", &[], vec![])
            .await
            .body,
    )
    .unwrap();
    assert!(snap_b.profiles.is_empty());

    send_with_headers(
        &s.f,
        &dev_b,
        "GET",
        "/hub/snapshot?all_devices=1",
        &[],
        vec![],
    )
    .await
    .assert_error(403, "host_control_required");

    // Host control can request ?all_devices=1 and receives a distinct ETag vs ?all_devices=0.
    let admin_all = send_with_headers(
        &s.f,
        &dev_admin,
        "GET",
        "/hub/snapshot?all_devices=1",
        &[],
        vec![],
    )
    .await;
    let admin_all_etag = admin_all.headers[header::ETAG].to_str().unwrap().to_owned();
    let snap_admin: HubSnapshot = serde_json::from_slice(&admin_all.body).unwrap();
    assert_eq!(snap_admin.profiles.len(), 1);

    let admin_own = send_with_headers(
        &s.f,
        &dev_admin,
        "GET",
        "/hub/snapshot?all_devices=0",
        &[("If-None-Match", &admin_all_etag)],
        vec![],
    )
    .await;
    assert_eq!(admin_own.status, StatusCode::OK);
    assert_ne!(
        admin_own.headers[header::ETAG].to_str().unwrap(),
        admin_all_etag
    );

    let shared_body = serde_json::to_vec(&json!({
        "scope": "shared",
        "name": "Living Room TV"
    }))
    .unwrap();
    let shared_put = send_with_headers(
        &s.f,
        &dev_admin,
        "PUT",
        "/hub/profiles/tv-shared",
        &[("Content-Type", "application/json"), ("If-Match", "\"0\"")],
        shared_body,
    )
    .await;
    assert_eq!(shared_put.status, StatusCode::CREATED);

    let snap_b_after: HubSnapshot = serde_json::from_slice(
        &send_with_headers(&s.f, &dev_b, "GET", "/hub/snapshot", &[], vec![])
            .await
            .body,
    )
    .unwrap();
    assert_eq!(snap_b_after.profiles.len(), 1);
    assert_eq!(snap_b_after.profiles[0].scope, ProfileScope::Shared);
}

#[tokio::test]
async fn hub_body_limits_pass_between_16k_and_64k_and_reject_above_with_json_413() {
    let s = Setup::new();

    // 32 KiB valid profile body exceeds the router-wide 16 KiB limit and must succeed.
    let deck_id = "d".repeat(64);
    let pad = "x".repeat(32 * 1024);
    let mid_body = serde_json::to_vec(&json!({
        "scope": "device",
        "name": "Mid Payload",
        "network_matcher": { "kind": "lan", "ssid": pad },
        "input_deck_id": deck_id
    }))
    .unwrap();
    assert!(mid_body.len() > 16 * 1024 && mid_body.len() < 64 * 1024);
    let ok = send_with_headers(
        &s.f,
        &s.connection,
        "PUT",
        "/hub/profiles/mid-body",
        &[("Content-Type", "application/json"), ("If-Match", "\"0\"")],
        mid_body,
    )
    .await;
    assert_eq!(ok.status, StatusCode::CREATED, "{}", ok.text());

    // > 64 KiB must return structured JSON ErrorCode::TooLarge (HTTP 413), not Axum text.
    let big_pad = "y".repeat(65 * 1024);
    let over_64k = serde_json::to_vec(&json!({
        "scope": "device",
        "name": "Over 64K",
        "network_matcher": { "kind": "lan", "ssid": big_pad }
    }))
    .unwrap();
    let r64 = send_with_headers(
        &s.f,
        &s.connection,
        "PUT",
        "/hub/profiles/over-64k",
        &[("Content-Type", "application/json"), ("If-Match", "\"0\"")],
        over_64k,
    )
    .await;
    r64.assert_error(413, "too_large");

    // > 1 MiB (exceeding nvhttp.rs's outer 1 MiB DefaultBodyLimit unless disabled on route)
    // must ALSO return structured JSON ErrorCode::TooLarge (HTTP 413), never plain text.
    let huge_pad = "z".repeat(1100 * 1024);
    let over_1m = serde_json::to_vec(&json!({
        "scope": "device",
        "name": "Over 1M",
        "network_matcher": { "kind": "lan", "ssid": huge_pad }
    }))
    .unwrap();
    let r1m = send_with_headers(
        &s.f,
        &s.connection,
        "PUT",
        "/hub/profiles/over-1m",
        &[("Content-Type", "application/json"), ("If-Match", "\"0\"")],
        over_1m,
    )
    .await;
    r1m.assert_error(413, "too_large");
}

#[tokio::test]
async fn hub_cas_conflicts_etags_covers_and_restart_persistence() {
    let s = Setup::new();
    // Point an app's artwork at a real PNG on disk.
    let cover_file = s.host().directory.join("custom-cover.png");
    std::fs::write(&cover_file, png_bytes(600, 900)).unwrap();
    let app_id = {
        let mut apps = s.host().apps.write().unwrap();
        apps[0].extra.insert(
            "image-path".into(),
            Value::String(cover_file.to_string_lossy().into_owned()),
        );
        apps[0].id()
    };

    // Snapshot returns ETag + CoverEntry, and matching If-None-Match returns 304 zero-body.
    let snap_res =
        send_with_headers(&s.f, &s.connection, "GET", "/hub/snapshot", &[], vec![]).await;
    assert_eq!(snap_res.status, StatusCode::OK);
    let snap_etag = snap_res.headers[header::ETAG].to_str().unwrap().to_owned();
    let snap: HubSnapshot = serde_json::from_slice(&snap_res.body).unwrap();
    assert_eq!(snap.covers.len(), 1);
    let cover_etag = snap.covers[0].etag.clone();

    let not_mod_snap = send_with_headers(
        &s.f,
        &s.connection,
        "GET",
        "/hub/snapshot",
        &[("If-None-Match", &snap_etag)],
        vec![],
    )
    .await;
    assert_eq!(not_mod_snap.status, StatusCode::NOT_MODIFIED);
    assert!(not_mod_snap.body.is_empty());

    // Cover fetch + 304 zero-body + ETag invalidation on cover edit.
    let cov_res = send_with_headers(
        &s.f,
        &s.connection,
        "GET",
        &format!("/hub/covers/{app_id}"),
        &[],
        vec![],
    )
    .await;
    assert_eq!(cov_res.status, StatusCode::OK);
    assert_eq!(cov_res.headers[header::CONTENT_TYPE], "image/png");
    let wire_cover_etag = cov_res.headers[header::ETAG].to_str().unwrap().to_owned();
    assert!(wire_cover_etag.contains(&cover_etag));

    let not_mod_cov = send_with_headers(
        &s.f,
        &s.connection,
        "GET",
        &format!("/hub/covers/{app_id}"),
        &[("If-None-Match", &wire_cover_etag)],
        vec![],
    )
    .await;
    assert_eq!(not_mod_cov.status, StatusCode::NOT_MODIFIED);
    assert!(not_mod_cov.body.is_empty());

    // Update cover on disk: ETag changes and snapshot revision bumps.
    std::fs::write(&cover_file, png_bytes(800, 1200)).unwrap();
    let snap2_res =
        send_with_headers(&s.f, &s.connection, "GET", "/hub/snapshot", &[], vec![]).await;
    let snap2: HubSnapshot = serde_json::from_slice(&snap2_res.body).unwrap();
    assert!(snap2.snapshot_revision > snap.snapshot_revision);
    assert_ne!(snap2.covers[0].etag, cover_etag);

    // CAS conflict returns 409 revision_conflict + X-Hub-Current-Revision.
    let body = serde_json::to_vec(&json!({ "scope": "device", "name": "Mine" })).unwrap();
    send_with_headers(
        &s.f,
        &s.connection,
        "PUT",
        "/hub/profiles/p1",
        &[("Content-Type", "application/json"), ("If-Match", "\"0\"")],
        body.clone(),
    )
    .await;
    let conflict = send_with_headers(
        &s.f,
        &s.connection,
        "PUT",
        "/hub/profiles/p1",
        &[("Content-Type", "application/json"), ("If-Match", "\"0\"")],
        body,
    )
    .await;
    conflict.assert_error(409, "revision_conflict");
    assert_eq!(
        conflict.headers[HEADER_CURRENT_REVISION].to_str().unwrap(),
        "1"
    );

    // Simulated restart: reloading HubStore from the same directory preserves state.
    let reloaded =
        butterpollo_stream::hub_store::HubStore::load(&s.host().directory, &s.host().warnings);
    let proj = reloaded
        .project(
            &s.host().apps.read().unwrap(),
            &s.host().directory,
            &s.client,
            false,
            false,
        )
        .unwrap();
    assert_eq!(proj.profiles.len(), 1);
    assert_eq!(proj.profiles[0].id, "p1");

    // Clean delete requires matching If-Match.
    let del_res = send_with_headers(
        &s.f,
        &s.connection,
        "DELETE",
        "/hub/profiles/p1",
        &[("If-Match", "\"1\"")],
        vec![],
    )
    .await;
    assert_eq!(del_res.status, StatusCode::OK);
    let deleted: ProfileDeleteResult = serde_json::from_slice(&del_res.body).unwrap();
    assert!(deleted.deleted);
}
