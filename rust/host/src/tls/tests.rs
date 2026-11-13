use super::*;
use tokio_rustls::{
    TlsConnector,
    rustls::{
        client::danger::{ServerCertVerified, ServerCertVerifier},
        pki_types::ServerName,
        sign::{CertifiedKey, SingleCertAndKey},
    },
};
/// Trusts any host: only the host's view of the client is under test.
#[derive(Debug)]
struct AnyHost;
impl ServerCertVerifier for AnyHost {
    fn verify_server_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> std::result::Result<ServerCertVerified, Error> {
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, Error> {
        Ok(HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, Error> {
        Ok(HandshakeSignatureValid::assertion())
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}
/// Whether the host completes a handshake with a client that presents
/// `certificate` and signs with `key`, and then sees that certificate.
async fn accepts(
    acceptor: &TlsAcceptor,
    version: &'static rustls::SupportedProtocolVersion,
    certificate: &[u8],
    key: &str,
) -> bool {
    let config = client(version, certificate, key);
    let (near, far) = tokio::io::duplex(1 << 16);
    let host = ServerName::try_from("localhost").unwrap();
    let (_, accepted) = tokio::join!(
        TlsConnector::from(Arc::new(config)).connect(host, near),
        acceptor.accept(far)
    );
    accepted.is_ok_and(|stream| {
        stream
            .get_ref()
            .1
            .peer_certificates()
            .and_then(|chain| chain.first())
            .is_some_and(|presented| presented.as_ref() == certificate)
    })
}
/// A client that presents `certificate`, signs with `key` and trusts
/// any host.
pub(crate) fn client(
    version: &'static rustls::SupportedProtocolVersion,
    certificate: &[u8],
    key: &str,
) -> rustls::ClientConfig {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let key = rustls_pemfile::private_key(&mut key.as_bytes())
        .unwrap()
        .unwrap();
    let key = provider.key_provider.load_private_key(key).unwrap();
    // CertifiedKey::new skips rustls's key check, which refuses v2 too.
    let client = CertifiedKey::new(vec![CertificateDer::from(certificate.to_vec())], key);
    rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[version])
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AnyHost))
        .with_client_cert_resolver(Arc::new(SingleCertAndKey::from(client)))
}
#[tokio::test]
async fn moonlight_tv_v2_certificate_completes_the_handshake() {
    // Made by moonlight-tv's libgamestream mkcert.c (mbedtls): X.509 v2.
    let certificate = butterpollo_core::crypto::certificate_der(include_str!(
        "../../../core/testdata/moonlight-tv-client.pem"
    ))
    .unwrap();
    let key = include_str!("../../../core/testdata/moonlight-tv-client.key");
    let identity = butterpollo_core::crypto::Identity::generate().unwrap();
    let acceptor = acceptor(&identity, true).unwrap();
    for version in [&rustls::version::TLS12, &rustls::version::TLS13] {
        assert!(accepts(&acceptor, version, &certificate, key).await);
        // The signature is still checked: the host's key is the wrong one.
        assert!(!accepts(&acceptor, version, &certificate, &identity.private_pem).await);
    }
}
/// A browser that trusts only the console's root accepts the console at
/// every loopback name it is opened by.
#[tokio::test]
async fn a_client_trusting_the_console_root_accepts_the_console() -> Result<()> {
    let console = butterpollo_core::crypto::ConsoleCertificate::generate()?;
    let acceptor = console_acceptor(&console)?;
    let mut roots = rustls::RootCertStore::empty();
    for root in rustls_pemfile::certs(&mut console.root_pem.as_bytes()) {
        roots.add(root?)?;
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let client = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_root_certificates(roots)
        .with_no_client_auth();
    let connector = TlsConnector::from(Arc::new(client));
    for name in ["localhost", "127.0.0.1", "::1"] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let acceptor = acceptor.clone();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await?;
            acceptor.accept(socket).await.map(|_| ())
        });
        let socket = tokio::net::TcpStream::connect(address).await?;
        let name = ServerName::try_from(name)?.to_owned();
        connector.connect(name, socket).await?;
        server.await??;
    }
    // Not a name the console's root may vouch for.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    tokio::spawn(async move {
        if let Ok((socket, _)) = listener.accept().await {
            let _ = acceptor.accept(socket).await;
        }
    });
    let socket = tokio::net::TcpStream::connect(address).await?;
    let name = ServerName::try_from("example.com")?;
    assert!(connector.connect(name, socket).await.is_err());
    Ok(())
}

const QUICK: Limits = Limits {
    connections: 3,
    per_peer: 2,
    header: Duration::from_millis(300),
    body_idle: Duration::from_millis(300),
};
/// A plain listener with `limits`, answering `/` and echoing the length
/// of a `/upload` body.
async fn quick_listener(limits: Limits) -> SocketAddr {
    use axum::routing::{get, post};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let router = Router::new().route("/", get(|| async { "ok" })).route(
        "/upload",
        post(|body: Bytes| async move { body.len().to_string() }),
    );
    tokio::spawn(serve_listener(listener, router, None, limits));
    address
}
/// Everything the host sends until it closes the connection, or `None`
/// if it still holds it after 3 s.
async fn until_closed(socket: &mut tokio::net::TcpStream) -> Option<String> {
    use tokio::io::AsyncReadExt;
    let mut answer = Vec::new();
    tokio::time::timeout(Duration::from_secs(3), socket.read_to_end(&mut answer))
        .await
        .ok()?
        .ok()?;
    Some(String::from_utf8_lossy(&answer).into_owned())
}

#[tokio::test]
async fn silent_slow_and_idle_clients_are_dropped() {
    use tokio::io::AsyncWriteExt;
    let address = quick_listener(QUICK).await;
    // Connects and sends nothing.
    let mut silent = tokio::net::TcpStream::connect(address).await.unwrap();
    assert!(until_closed(&mut silent).await.is_some());
    // Starts its header and never finishes it.
    let mut slow = tokio::net::TcpStream::connect(address).await.unwrap();
    slow.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n")
        .await
        .unwrap();
    assert!(until_closed(&mut slow).await.is_some());
    // Keeps its connection open after an answer, with no next request.
    let mut idle = tokio::net::TcpStream::connect(address).await.unwrap();
    idle.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")
        .await
        .unwrap();
    let answer = until_closed(&mut idle).await.expect("idle keep-alive held");
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    // Stalls its upload: the request fails and the connection ends.
    let mut stalled = tokio::net::TcpStream::connect(address).await.unwrap();
    stalled
        .write_all(b"POST /upload HTTP/1.1\r\nHost: x\r\nContent-Length: 10\r\n\r\nab")
        .await
        .unwrap();
    let answer = until_closed(&mut stalled)
        .await
        .expect("stalled upload held");
    assert!(!answer.starts_with("HTTP/1.1 200"), "{answer}");
}

#[tokio::test]
async fn an_upload_that_keeps_sending_outlasts_the_timeouts() {
    use tokio::io::AsyncWriteExt;
    let address = quick_listener(QUICK).await;
    let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
    socket
        .write_all(
            b"POST /upload HTTP/1.1\r\nHost: x\r\nConnection: close\r\nContent-Length: 12\r\n\r\n",
        )
        .await
        .unwrap();
    // Three times the idle limit in all, never idle for half of it.
    for _ in 0..12 {
        tokio::time::sleep(QUICK.body_idle / 4).await;
        socket.write_all(b"x").await.unwrap();
    }
    let answer = until_closed(&mut socket).await.unwrap();
    assert!(
        answer.starts_with("HTTP/1.1 200") && answer.ends_with("12"),
        "{answer}"
    );
}

#[tokio::test]
async fn a_connection_over_the_cap_is_closed_at_once() {
    use tokio::io::AsyncReadExt;
    let address = quick_listener(Limits {
        header: Duration::from_secs(30),
        ..QUICK
    })
    .await;
    let held = [
        tokio::net::TcpStream::connect(address).await.unwrap(),
        tokio::net::TcpStream::connect(address).await.unwrap(),
    ];
    // The third from this address is closed without an answer.
    let mut over = tokio::net::TcpStream::connect(address).await.unwrap();
    let mut byte = [0; 1];
    let read = tokio::time::timeout(Duration::from_secs(3), over.read(&mut byte)).await;
    assert!(matches!(read, Ok(Ok(0)) | Ok(Err(_))), "{read:?}");
    // One closing frees its place.
    drop(held);
    tokio::time::sleep(Duration::from_millis(200)).await;
    let mut again = tokio::net::TcpStream::connect(address).await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(500), again.read(&mut byte))
            .await
            .is_err(),
        "a freed place was refused"
    );
}

#[test]
fn the_caps_count_per_address_and_in_total() {
    let admission = Admission::new(QUICK);
    let a: IpAddr = "192.168.4.20".parse().unwrap();
    let b: IpAddr = "192.168.4.21".parse().unwrap();
    let first = admission.admit(a).unwrap();
    // The v4-mapped form is the same address.
    let second = admission
        .admit("::ffff:192.168.4.20".parse().unwrap())
        .unwrap();
    assert!(admission.admit(a).is_none(), "over the per-address cap");
    let other = admission.admit(b).unwrap();
    assert!(admission.admit(b).is_none(), "over the listener's cap");
    drop(first);
    assert!(admission.admit(b).is_some());
    drop((second, other));
    assert!(admission.peers.lock().unwrap().is_empty());
}
