//! USB forwarding against a fake driver and a scripted client device.

use super::*;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    sync::mpsc,
};
use tokio::io::DuplexStream;

const SESSION: &str = "session-1";
const BUSID: &str = "1-1";
const VID: u16 = 0x0781;
const PID: u16 = 0x5581;

pub(crate) fn client(allow_usb: bool) -> Client {
    let mut extra = BTreeMap::new();
    if allow_usb {
        extra.insert("allow_usb".to_owned(), Value::Bool(true));
    }
    Client {
        name: "phone".into(),
        cert: "cert-phone".into(),
        uuid: "uuid-phone".into(),
        perm: 0,
        enabled: true,
        extra,
    }
}

fn request(request_id: u32, busid: &str) -> UsbMessage {
    UsbMessage::AttachRequest {
        request_id,
        vid: VID,
        pid: PID,
        class: 8,
        speed: 3,
        busid: busid.into(),
        name: "Flash drive".into(),
    }
}

/// A driver that imports the device like usbip-win2 does and hands its connection to the test.
struct FakeVhci {
    ready: bool,
    connections: Mutex<mpsc::Sender<std::net::TcpStream>>,
    detached: Mutex<Vec<(u32, String)>>,
}
impl FakeVhci {
    fn new(ready: bool) -> (Arc<Self>, mpsc::Receiver<std::net::TcpStream>) {
        let (sender, receiver) = mpsc::channel();
        let vhci = Arc::new(Self {
            ready,
            connections: Mutex::new(sender),
            detached: Mutex::default(),
        });
        (vhci, receiver)
    }
    fn detached(&self) -> Vec<(u32, String)> {
        self.detached.lock().unwrap().clone()
    }
}
impl Vhci for FakeVhci {
    fn ready(&self) -> bool {
        self.ready
    }
    fn attach(&self, tcp_port: u16, busid: &str) -> Result<u32> {
        let mut driver = std::net::TcpStream::connect((Ipv4Addr::LOCALHOST, tcp_port))?;
        driver.write_all(&wire::import_request(busid).context("busid")?)?;
        let mut head = [0; wire::OP_HEADER_BYTES];
        driver.read_exact(&mut head)?;
        let header = wire::OpHeader::decode(&head);
        if header.status != 0 {
            bail!("import refused: {}", header.status);
        }
        let mut device = [0; wire::DEVICE_BYTES];
        driver.read_exact(&mut device)?;
        self.connections.lock().unwrap().send(driver)?;
        Ok(3)
    }
    fn detach(&self, port: u32, _tcp_port: u16, busid: &str) -> Result<()> {
        self.detached.lock().unwrap().push((port, busid.to_owned()));
        Ok(())
    }
}

/// An installed driver for tests that never attach.
pub(crate) fn installed() -> Arc<dyn Vhci> {
    FakeVhci::new(true).0
}

fn forwarding(vhci: Arc<dyn Vhci>) -> Arc<UsbForwarding> {
    let forwarding = Arc::new(UsbForwarding::default());
    forwarding.set_vhci(vhci);
    forwarding
}

fn refusal(messages: &[UsbMessage]) -> Option<UsbReason> {
    match messages {
        [
            UsbMessage::Status {
                state: UsbState::Refused,
                reason,
                ..
            },
        ] => Some(*reason),
        _ => None,
    }
}

fn token(messages: &[UsbMessage]) -> [u8; TOKEN_BYTES] {
    match messages {
        [UsbMessage::TunnelReady { token, .. }] => *token,
        other => panic!("expected a TunnelReady, got {other:?}"),
    }
}

/// Asks for the device and returns its token.
fn asked(forwarding: &UsbForwarding, request_id: u32) -> [u8; TOKEN_BYTES] {
    forwarding.on_message(SESSION, &client(true), true, 4, request(request_id, BUSID));
    token(&forwarding.take(SESSION, Instant::now()))
}

#[test]
fn attach_requests_are_refused_by_setting_permission_busid_driver_and_limit() {
    let (ready, _) = FakeVhci::new(true);
    let forwarding = forwarding(ready);
    let now = Instant::now();
    let ask = |client: &Client, enabled: bool, limit: u8, busid: &str| {
        forwarding.on_message(SESSION, client, enabled, limit, request(7, busid));
        refusal(&forwarding.take(SESSION, now))
    };
    assert_eq!(
        ask(&client(true), false, 4, BUSID),
        Some(UsbReason::Disabled)
    );
    assert_eq!(
        ask(&client(false), true, 4, BUSID),
        Some(UsbReason::Permission)
    );
    // Full control never implies USB.
    let mut full_control = client(false);
    full_control.perm = u32::MAX;
    assert_eq!(
        ask(&full_control, true, 4, BUSID),
        Some(UsbReason::Permission)
    );
    let mut disabled = client(true);
    disabled.enabled = false;
    assert_eq!(ask(&disabled, true, 4, BUSID), Some(UsbReason::Permission));
    assert_eq!(
        ask(&client(true), true, 4, "-1 --remote evil"),
        Some(UsbReason::DeviceError)
    );
    assert_eq!(ask(&client(true), true, 0, BUSID), Some(UsbReason::Limit));
    assert!(forwarding.list().is_empty());

    let (missing, _) = FakeVhci::new(false);
    let forwarding = self::forwarding(missing);
    forwarding.on_message(SESSION, &client(true), true, 4, request(7, BUSID));
    assert_eq!(
        refusal(&forwarding.take(SESSION, now)),
        Some(UsbReason::Driver)
    );
}

#[test]
fn the_device_limit_counts_every_client() {
    let (vhci, _) = FakeVhci::new(true);
    let forwarding = forwarding(vhci);
    forwarding.on_message("session-a", &client(true), true, 1, request(1, BUSID));
    token(&forwarding.take("session-a", Instant::now()));
    forwarding.on_message("session-b", &client(true), true, 1, request(1, BUSID));
    assert_eq!(
        refusal(&forwarding.take("session-b", Instant::now())),
        Some(UsbReason::Limit)
    );
    // A repeated request id is ignored, not attached twice.
    forwarding.on_message("session-a", &client(true), true, 4, request(1, BUSID));
    assert!(forwarding.take("session-a", Instant::now()).is_empty());
    assert_eq!(forwarding.list().len(), 1);
}

#[test]
fn a_token_opens_once_within_its_time_and_only_for_its_device() {
    let (vhci, _) = FakeVhci::new(true);
    let forwarding = forwarding(vhci);
    let token = asked(&forwarding, 1);
    let now = Instant::now();
    assert_eq!(
        forwarding
            .claim(&[0; TOKEN_BYTES], "cert-phone", now)
            .unwrap_err(),
        Refused::Unknown
    );
    assert_eq!(
        forwarding.claim(&token, "cert-other", now).unwrap_err(),
        Refused::Unknown
    );
    let late = now + Duration::from_secs(TOKEN_TTL_SECS + 1);
    assert_eq!(
        forwarding.claim(&token, "cert-phone", late).unwrap_err(),
        Refused::Unknown
    );
    let opened = forwarding.claim(&token, "cert-phone", now).unwrap();
    assert_eq!(opened.request_id, 1);
    assert_eq!(forwarding.list()[0].state, "opening");
    assert_eq!(
        forwarding.claim(&token, "cert-phone", now).unwrap_err(),
        Refused::Used
    );
    forwarding.abandon(opened, "upgrade failed");
    assert!(forwarding.list().is_empty());
    assert!(matches!(
        forwarding.take(SESSION, now)[..],
        [UsbMessage::Status {
            request_id: 1,
            state: UsbState::Failed,
            reason: UsbReason::DeviceError
        }]
    ));
}

#[test]
fn an_unopened_tunnel_times_out() {
    let (vhci, _) = FakeVhci::new(true);
    let forwarding = forwarding(vhci);
    let token = asked(&forwarding, 2);
    let late = Instant::now() + Duration::from_secs(TOKEN_TTL_SECS + 1);
    assert!(matches!(
        forwarding.take(SESSION, late)[..],
        [UsbMessage::Status {
            request_id: 2,
            state: UsbState::Failed,
            reason: UsbReason::TunnelTimeout
        }]
    ));
    assert!(forwarding.list().is_empty());
    assert!(
        forwarding
            .claim(&token, "cert-phone", Instant::now())
            .is_err()
    );
}

#[test]
fn waiting_attachments_end_with_the_client_the_console_and_the_session() {
    let (vhci, _) = FakeVhci::new(true);
    let forwarding = forwarding(vhci);
    let now = Instant::now();

    asked(&forwarding, 1);
    forwarding.on_message(
        SESSION,
        &client(true),
        true,
        4,
        UsbMessage::Detach { request_id: 1 },
    );
    assert!(matches!(
        forwarding.take(SESSION, now)[..],
        [UsbMessage::Status {
            request_id: 1,
            state: UsbState::Detached,
            reason: UsbReason::None
        }]
    ));
    assert!(forwarding.list().is_empty());

    let token = asked(&forwarding, 2);
    let id = forwarding.list()[0].id;
    assert_eq!(forwarding.list()[0].state, "waiting");
    assert!(forwarding.detach(id));
    assert!(!forwarding.detach(id));
    assert!(matches!(
        forwarding.take(SESSION, now)[..],
        [
            UsbMessage::Detach { request_id: 2 },
            UsbMessage::Status {
                request_id: 2,
                state: UsbState::Detached,
                reason: UsbReason::None
            }
        ]
    ));
    assert!(forwarding.claim(&token, "cert-phone", now).is_err());

    asked(&forwarding, 3);
    forwarding.on_message("other", &client(true), true, 4, request(1, BUSID));
    assert!(matches!(
        forwarding.end_session(SESSION)[..],
        [
            UsbMessage::Detach { request_id: 3 },
            UsbMessage::Status {
                request_id: 3,
                state: UsbState::Detached,
                reason: UsbReason::SessionEnd
            }
        ]
    ));
    assert_eq!(forwarding.list().len(), 1, "another session's device stays");
    assert!(forwarding.end_session(SESSION).is_empty());
}

/// A client device: answers the driver's import with the requested device.
async fn serve_import(client: &mut DuplexStream, vendor: u16) {
    let mut request = [0; wire::IMPORT_REQUEST_BYTES];
    client.read_exact(&mut request).await.unwrap();
    assert_eq!(
        wire::decode_import_request(&request).as_deref(),
        Some(BUSID)
    );
    let device = wire::Device {
        path: "/android/usb/1".into(),
        busid: BUSID.into(),
        busnum: 1,
        devnum: 1,
        speed: 3,
        vendor,
        product: PID,
        bcd_device: 0x0100,
        class: 0,
        subclass: 0,
        protocol: 0,
        configuration: 1,
        configurations: 1,
        interfaces: 1,
    };
    client.write_all(&wire::import_reply(0)).await.unwrap();
    client.write_all(&device.encode().unwrap()).await.unwrap();
}

async fn read_header<R: AsyncRead + Unpin>(from: &mut R) -> Header {
    let mut head = [0; wire::HEADER_BYTES];
    from.read_exact(&mut head).await.unwrap();
    Header::decode(&head).unwrap()
}

fn base(seqnum: u32, direction: u32, ep: u32) -> wire::Base {
    wire::Base {
        seqnum,
        devid: 0x10002,
        direction,
        ep,
    }
}

fn submit(seqnum: u32, direction: u32, length: u32) -> Header {
    Header::Submit {
        base: base(seqnum, direction, 1),
        flags: 0,
        length,
        start_frame: 0,
        packets: wire::NOT_ISO,
        interval: 0,
        setup: [0; 8],
    }
}

fn ret_submit(seqnum: u32, length: u32) -> Header {
    Header::RetSubmit {
        base: base(seqnum, 0, 0),
        status: 0,
        length,
        start_frame: 0,
        packets: wire::NOT_ISO,
        errors: 0,
    }
}

/// Polls the session's outbox until it holds something.
async fn next(forwarding: &UsbForwarding) -> Vec<UsbMessage> {
    for _ in 0..500 {
        let messages = forwarding.take(SESSION, Instant::now());
        if !messages.is_empty() {
            return messages;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("no USB message for the client");
}

struct Relay {
    forwarding: Arc<UsbForwarding>,
    vhci: Arc<FakeVhci>,
    driver: TcpStream,
    client: DuplexStream,
    served: tokio::task::JoinHandle<()>,
}

/// A device attached through the relay, with the driver's and the client's ends.
async fn attached() -> Relay {
    let (vhci, connections) = FakeVhci::new(true);
    let forwarding = forwarding(vhci.clone());
    let token = asked(&forwarding, 5);
    let opened = forwarding
        .claim(&token, "cert-phone", Instant::now())
        .unwrap();
    let (tunnel, mut client) = tokio::io::duplex(1 << 16);
    let served = tokio::spawn({
        let forwarding = forwarding.clone();
        async move { forwarding.serve(opened, tunnel).await }
    });
    serve_import(&mut client, VID).await;
    assert!(matches!(
        next(&forwarding).await[..],
        [UsbMessage::Status {
            request_id: 5,
            state: UsbState::Attached,
            reason: UsbReason::None
        }]
    ));
    let listed = &forwarding.list()[0];
    assert_eq!((listed.state, listed.port), ("attached", Some(3)));
    let driver = connections.recv_timeout(Duration::from_secs(5)).unwrap();
    driver.set_nonblocking(true).unwrap();
    Relay {
        forwarding,
        vhci,
        driver: TcpStream::from_std(driver).unwrap(),
        client,
        served,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_relay_carries_transfers_and_unlinks_and_the_console_detaches() {
    let Relay {
        forwarding,
        vhci,
        mut driver,
        mut client,
        served,
    } = attached().await;

    // An IN transfer: the client answers with less than the driver's buffer.
    driver
        .write_all(&submit(1, wire::DIR_IN, 512).encode())
        .await
        .unwrap();
    assert_eq!(read_header(&mut client).await, submit(1, wire::DIR_IN, 512));
    client.write_all(&ret_submit(1, 13).encode()).await.unwrap();
    client.write_all(b"hello, driver").await.unwrap();
    assert_eq!(read_header(&mut driver).await, ret_submit(1, 13));
    let mut data = [0; 13];
    driver.read_exact(&mut data).await.unwrap();
    assert_eq!(&data, b"hello, driver");

    // An OUT transfer: its data goes to the client, the reply carries none.
    driver
        .write_all(&submit(2, wire::DIR_OUT, 4).encode())
        .await
        .unwrap();
    driver.write_all(b"ping").await.unwrap();
    assert_eq!(read_header(&mut client).await, submit(2, wire::DIR_OUT, 4));
    let mut data = [0; 4];
    client.read_exact(&mut data).await.unwrap();
    assert_eq!(&data, b"ping");
    client.write_all(&ret_submit(2, 4).encode()).await.unwrap();
    assert_eq!(read_header(&mut driver).await, ret_submit(2, 4));

    // A cancelled transfer: the unlink answers for it.
    driver
        .write_all(&submit(3, wire::DIR_IN, 64).encode())
        .await
        .unwrap();
    let unlink = Header::Unlink {
        base: base(4, wire::DIR_OUT, 1),
        unlink: 3,
    };
    driver.write_all(&unlink.encode()).await.unwrap();
    assert_eq!(read_header(&mut client).await, submit(3, wire::DIR_IN, 64));
    assert_eq!(read_header(&mut client).await, unlink);
    let reset = Header::RetUnlink {
        base: base(4, 0, 0),
        status: wire::ECONNRESET,
    };
    client.write_all(&reset.encode()).await.unwrap();
    assert_eq!(read_header(&mut driver).await, reset);

    // The console detaches it: the relay stops, the driver lets go, the client hears once.
    let id = forwarding.list()[0].id;
    assert!(forwarding.detach(id));
    tokio::time::timeout(Duration::from_secs(5), served)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(vhci.detached(), [(3, BUSID.to_owned())]);
    assert!(forwarding.list().is_empty());
    assert!(matches!(
        forwarding.take(SESSION, Instant::now())[..],
        [
            UsbMessage::Detach { request_id: 5 },
            UsbMessage::Status {
                request_id: 5,
                state: UsbState::Detached,
                reason: UsbReason::None
            }
        ]
    ));
    let mut rest = Vec::new();
    client.read_to_end(&mut rest).await.unwrap();
    assert!(rest.is_empty(), "the tunnel closes with the relay");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_attached_device_detaches_when_its_session_ends() {
    // Both ends stay open: only the session end detaches.
    let Relay {
        forwarding,
        vhci,
        driver: _driver,
        client: _client,
        served,
    } = attached().await;
    assert!(matches!(
        forwarding.end_session(SESSION)[..],
        [
            UsbMessage::Detach { request_id: 5 },
            UsbMessage::Status {
                request_id: 5,
                state: UsbState::Detached,
                reason: UsbReason::SessionEnd
            }
        ]
    ));
    tokio::time::timeout(Duration::from_secs(5), served)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(vhci.detached(), [(3, BUSID.to_owned())]);
    assert!(forwarding.list().is_empty());
    assert!(
        forwarding.take(SESSION, Instant::now()).is_empty(),
        "the session end already told the client"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_relay_detaches_a_client_that_breaks_the_framing() {
    for reply in [
        // A reply to nothing the driver asked.
        ret_submit(9, 0),
        // More IN data than the driver's buffer holds.
        ret_submit(1, 4096),
    ] {
        let Relay {
            forwarding,
            vhci,
            mut driver,
            mut client,
            served,
        } = attached().await;
        driver
            .write_all(&submit(1, wire::DIR_IN, 512).encode())
            .await
            .unwrap();
        read_header(&mut client).await;
        client.write_all(&reply.encode()).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), served)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(vhci.detached(), [(3, BUSID.to_owned())]);
        assert!(matches!(
            forwarding.take(SESSION, Instant::now())[..],
            [UsbMessage::Status {
                request_id: 5,
                state: UsbState::Failed,
                reason: UsbReason::DeviceError
            }]
        ));
        let mut rest = Vec::new();
        assert_eq!(
            driver.read_to_end(&mut rest).await.unwrap(),
            0,
            "nothing reaches the driver"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_client_closing_its_tunnel_detaches_the_device() {
    let Relay {
        forwarding,
        vhci,
        driver: _driver,
        client,
        served,
    } = attached().await;
    drop(client);
    tokio::time::timeout(Duration::from_secs(5), served)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(vhci.detached(), [(3, BUSID.to_owned())]);
    assert!(matches!(
        forwarding.take(SESSION, Instant::now())[..],
        [UsbMessage::Status {
            request_id: 5,
            state: UsbState::Detached,
            reason: UsbReason::None
        }]
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_driver_letting_go_fails_the_attachment() {
    let Relay {
        forwarding,
        vhci,
        driver,
        client: _client,
        served,
    } = attached().await;
    drop(driver);
    tokio::time::timeout(Duration::from_secs(5), served)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(vhci.detached(), [(3, BUSID.to_owned())]);
    assert!(matches!(
        forwarding.take(SESSION, Instant::now())[..],
        [UsbMessage::Status {
            request_id: 5,
            state: UsbState::Failed,
            reason: UsbReason::Driver
        }]
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_serving_another_device_is_refused() {
    let (vhci, _connections) = FakeVhci::new(true);
    let forwarding = forwarding(vhci.clone());
    let token = asked(&forwarding, 6);
    let opened = forwarding
        .claim(&token, "cert-phone", Instant::now())
        .unwrap();
    let (tunnel, mut client) = tokio::io::duplex(1 << 16);
    let served = tokio::spawn({
        let forwarding = forwarding.clone();
        async move { forwarding.serve(opened, tunnel).await }
    });
    serve_import(&mut client, 0x046d).await;
    tokio::time::timeout(Duration::from_secs(5), served)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        forwarding.take(SESSION, Instant::now())[..],
        [UsbMessage::Status {
            request_id: 6,
            state: UsbState::Failed,
            reason: UsbReason::DeviceError
        }]
    ));
    assert!(forwarding.list().is_empty());
}
