//! `GET /usb/tunnel`: the setting, the device's permission and the token decide who opens it.
use super::*;
use crate::usb_forward::Vhci;
use butterpollo_core::usb_forward::{UsbMessage, UsbReason, UsbState};
use std::time::{Duration, Instant};

const TOKEN_HEADER: &str = "X-Rubylight-Usb-Token";
const UPGRADE: &str = "rubylight-usbip/1";
const SESSION: &str = "usb-session";

/// An installed driver; these requests never get as far as attaching.
struct Installed;
impl Vhci for Installed {
    fn ready(&self) -> bool {
        true
    }
    fn is_driver(
        &self,
        _peer: std::net::SocketAddr,
        _relay: std::net::SocketAddr,
    ) -> anyhow::Result<bool> {
        anyhow::bail!("the router tests relay nothing")
    }
    fn attach(&self, _tcp_port: u16, _busid: &str) -> anyhow::Result<u32> {
        anyhow::bail!("the router tests attach nothing")
    }
    fn detach(&self, _port: u32, _tcp_port: u16, _busid: &str) -> anyhow::Result<()> {
        Ok(())
    }
    fn stop(&self, _tcp_port: u16, _busid: &str) -> anyhow::Result<()> {
        Ok(())
    }
}

fn set(s: &Setup, key: &str, value: &str) {
    s.host()
        .config
        .write()
        .unwrap()
        .values
        .insert(key.into(), value.into());
}

/// The console grants or revokes `allow_usb` for paired device `index`.
fn allow_usb(s: &Setup, index: usize, allowed: bool) -> Client {
    let mut paired = s.host().paired.write().unwrap();
    let client = &mut paired.clients[index];
    client.extra.insert("allow_usb".into(), allowed.into());
    client.clone()
}

/// The token the host hands `client` for a flash drive, as the client sends it.
fn token(s: &Setup, client: &Client) -> String {
    s.host().usb.on_message(
        SESSION,
        client,
        true,
        4,
        UsbMessage::AttachRequest {
            request_id: 1,
            vid: 0x0781,
            pid: 0x5581,
            class: 8,
            speed: 3,
            busid: "1-1".into(),
            name: "Flash drive".into(),
        },
    );
    match &s.host().usb.take(SESSION, Instant::now())[..] {
        [UsbMessage::TunnelReady { token, .. }] => {
            token.iter().map(|byte| format!("{byte:02x}")).collect()
        }
        other => panic!("expected a TunnelReady, got {other:?}"),
    }
}

async fn open(s: &Setup, connection: &Connection, upgrade: &str, token: Option<&str>) -> Reply {
    let mut request = Request::builder()
        .method("GET")
        .uri(format!("{PREFIX}/usb/tunnel"))
        .header(header::CONNECTION, "upgrade")
        .header(header::UPGRADE, upgrade);
    if let Some(token) = token {
        request = request.header(TOKEN_HEADER, token);
    }
    let response = router(s.host().clone(), true)
        .layer(Extension(connection.clone()))
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let (parts, body) = response.into_parts();
    Reply {
        status: parts.status,
        headers: parts.headers,
        body: to_bytes(body, 1 << 20).await.unwrap().to_vec(),
    }
}

#[tokio::test]
async fn the_usb_tunnel_needs_the_setting_and_allow_usb_never_full_control() {
    let s = Setup::with(u32::MAX);
    s.host().usb.set_vhci(Arc::new(Installed));
    let zero = "0".repeat(32);
    let reply = open(&s, &s.connection, UPGRADE, Some(&zero)).await;
    reply.assert_error(403, "usb_disabled");
    assert_eq!(reply.json()["error"]["setting"], "usb_forwarding");

    set(&s, "usb_forwarding", "enabled");
    open(&s, &s.connection, UPGRADE, Some(&zero))
        .await
        .assert_error(403, "usb_disabled");
    allow_usb(&s, 0, true);
    open(&s, &s.connection, UPGRADE, Some(&zero))
        .await
        .assert_error(404, "not_found");
}

#[tokio::test]
async fn a_usb_tunnel_token_opens_once_for_the_device_that_asked() {
    let s = Setup::new();
    s.host().usb.set_vhci(Arc::new(Installed));
    set(&s, "usb_forwarding", "enabled");
    let client = allow_usb(&s, 0, true);
    let token = token(&s, &client);

    // `from_str_radix` alone would take a sign.
    let signed = "+f".repeat(16);
    for (upgrade, token) in [
        ("websocket", Some(token.as_str())),
        (UPGRADE, None),
        (UPGRADE, Some("not-a-token")),
        (UPGRADE, Some(signed.as_str())),
    ] {
        open(&s, &s.connection, upgrade, token)
            .await
            .assert_error(400, "invalid_request");
    }
    // Another paired device with USB forwarding can't use this device's token.
    let other = other_device(&s.f, ALL, false);
    allow_usb(&s, 1, true);
    open(&s, &other, UPGRADE, Some(&token))
        .await
        .assert_error(404, "not_found");

    let reply = open(
        &s,
        &s.connection,
        UPGRADE,
        Some(&token.to_ascii_uppercase()),
    )
    .await;
    assert_eq!(
        reply.status,
        StatusCode::SWITCHING_PROTOCOLS,
        "{}",
        reply.text()
    );
    assert_eq!(reply.headers[header::UPGRADE], UPGRADE);
    // A request outside a real connection has nothing to upgrade: the client hears it failed.
    let mut heard = Vec::new();
    for _ in 0..200 {
        heard = s.host().usb.take(SESSION, Instant::now());
        if !heard.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        matches!(
            heard[..],
            [UsbMessage::Status {
                request_id: 1,
                state: UsbState::Failed,
                reason: UsbReason::DeviceError
            }]
        ),
        "{heard:?}"
    );
    assert!(s.host().usb.list().is_empty());
    open(&s, &s.connection, UPGRADE, Some(&token))
        .await
        .assert_error(404, "not_found");
}

#[tokio::test]
async fn a_used_usb_tunnel_token_is_not_found_rather_than_over_the_limit() {
    let s = Setup::new();
    s.host().usb.set_vhci(Arc::new(Installed));
    set(&s, "usb_forwarding", "enabled");
    let client = allow_usb(&s, 0, true);
    let token = token(&s, &client);
    let bytes: [u8; 16] =
        std::array::from_fn(|at| u8::from_str_radix(&token[at * 2..at * 2 + 2], 16).unwrap());
    // Another tunnel already carries it.
    let _opened = s
        .host()
        .usb
        .claim(&bytes, &client.cert, Instant::now())
        .unwrap();
    let reply = open(&s, &s.connection, UPGRADE, Some(&token)).await;
    reply.assert_error(404, "not_found");
    assert!(reply.text().contains("already used"), "{}", reply.text());
}
