use anyhow::Result;
use axum::{Extension, Router};
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    server::conn::auto::Builder,
    service::TowerToHyperService,
};
use std::{
    net::SocketAddr,
    sync::{Arc, Mutex, mpsc},
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_rustls::{
    TlsAcceptor,
    rustls::{
        self, CertificateError, DigitallySignedStruct, DistinguishedName, Error, PeerMisbehaved,
        SignatureScheme,
        client::danger::HandshakeSignatureValid,
        pki_types::{CertificateDer, SubjectPublicKeyInfoDer, UnixTime},
        server::danger::{ClientCertVerified, ClientCertVerifier},
    },
};

#[derive(Clone)]
pub struct Connection {
    pub peer: SocketAddr,
    pub local: SocketAddr,
    pub certificate: Option<Vec<u8>>,
    pub tls: bool,
}
/// How a connection ended, for a handler that may act only once its answer
/// has left: `true` once hyper wrote every response and closed the
/// connection without an error, `false` on an error. A watcher whose
/// connection task was dropped sees its channel disconnect instead.
#[derive(Clone, Default)]
pub struct ConnectionEnd(Arc<Mutex<Vec<mpsc::SyncSender<bool>>>>);
impl ConnectionEnd {
    pub fn watch(&self) -> mpsc::Receiver<bool> {
        let (sender, receiver) = mpsc::sync_channel(1);
        self.0.lock().unwrap().push(sender);
        receiver
    }
    pub fn finish(&self, clean: bool) {
        for watcher in self.0.lock().unwrap().drain(..) {
            let _ = watcher.try_send(clean);
        }
    }
}
/// TLS proves possession of any presented key. Protected HTTP routes additionally
/// require one exact enabled pairing record; a self-signed certificate is not authorization.
#[derive(Debug)]
struct ClientProof {
    provider: Arc<rustls::crypto::CryptoProvider>,
}
/// rustls's own signature checks parse the certificate with webpki, which
/// takes only X.509 v3. Moonlight for webOS (moonlight-tv) sends a v2
/// certificate, so its handshake failed before it could pair (issue #8);
/// the signature needs only the public key, which any version carries.
fn client_key<'a>(
    certificate: &'a CertificateDer<'_>,
) -> Result<SubjectPublicKeyInfoDer<'a>, Error> {
    butterpollo_core::crypto::subject_public_key_info(certificate)
        .map(SubjectPublicKeyInfoDer::from)
        .map_err(|_| CertificateError::BadEncoding.into())
}
impl ClientCertVerifier for ClientProof {
    fn client_auth_mandatory(&self) -> bool {
        false
    }
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }
    fn verify_client_cert(
        &self,
        end: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: UnixTime,
    ) -> std::result::Result<ClientCertVerified, Error> {
        if end.is_empty() || end.len() > 16384 {
            return Err(Error::General("invalid client certificate size".into()));
        }
        Ok(ClientCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, Error> {
        let (_, algorithms) = self
            .provider
            .signature_verification_algorithms
            .mapping
            .iter()
            .find(|(scheme, _)| *scheme == signature.scheme)
            .ok_or(PeerMisbehaved::SignedHandshakeWithUnadvertisedSigScheme)?;
        let spki = client_key(cert)?;
        let key = webpki::RawPublicKeyEntity::try_from(&spki)
            .map_err(|_| Error::from(CertificateError::BadEncoding))?;
        // A TLS 1.2 ECDSA scheme leaves the curve open, as in rustls.
        for algorithm in *algorithms {
            match key.verify_signature(*algorithm, message, signature.signature()) {
                Ok(()) => return Ok(HandshakeSignatureValid::assertion()),
                Err(webpki::Error::UnsupportedSignatureAlgorithmForPublicKeyContext(_)) => {}
                Err(_) => break,
            }
        }
        Err(CertificateError::BadSignature.into())
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, Error> {
        rustls::crypto::verify_tls13_signature_with_raw_key(
            message,
            &client_key(cert)?,
            signature,
            &self.provider.signature_verification_algorithms,
        )
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}
pub fn acceptor(
    identity: &butterpollo_core::crypto::Identity,
    client_auth: bool,
) -> Result<TlsAcceptor> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ServerConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()?;
    let builder = if client_auth {
        builder.with_client_cert_verifier(Arc::new(ClientProof { provider }))
    } else {
        builder.with_no_client_auth()
    };
    let mut keys = identity.private_pem.as_bytes();
    let key = rustls_pemfile::private_key(&mut keys)?
        .ok_or_else(|| anyhow::anyhow!("empty server private key"))?;
    let config = builder.with_single_cert(vec![CertificateDer::from(identity.der.clone())], key)?;
    Ok(TlsAcceptor::from(Arc::new(config)))
}
/// The console's listener presents its own certificate, which browsers trust
/// once setup has trusted its root, instead of the self-signed pairing one.
pub fn console_acceptor(
    console: &butterpollo_core::crypto::ConsoleCertificate,
) -> Result<TlsAcceptor> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let chain = rustls_pemfile::certs(&mut console.chain_pem.as_bytes())
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let key = rustls_pemfile::private_key(&mut console.key_pem.as_bytes())?
        .ok_or_else(|| anyhow::anyhow!("empty console private key"))?;
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_single_cert(chain, key)?;
    Ok(TlsAcceptor::from(Arc::new(config)))
}
pub async fn serve(
    address: SocketAddr,
    router: Router,
    acceptor: Option<TlsAcceptor>,
) -> Result<()> {
    let listener = crate::network::tcp(address)?;
    tracing::debug!(%address,tls=acceptor.is_some(),"HTTP listener ready");
    loop {
        let (socket, peer) = crate::network::accept(&listener).await;
        if let Err(error) = socket.set_nodelay(true) {
            tracing::debug!(%peer, %error, "could not disable Nagle's algorithm");
        }
        let Ok(local) = socket.local_addr() else {
            tracing::debug!(%peer, "connection closed before it was served");
            continue;
        };
        let local = SocketAddr::new(local.ip().to_canonical(), local.port());
        tokio::spawn(serve_connection(
            socket,
            peer,
            local,
            router.clone(),
            acceptor.clone(),
        ));
    }
}
/// Serves one accepted connection, over TLS with an acceptor, and tells the
/// handlers' [`ConnectionEnd`] how it ended.
pub(crate) async fn serve_connection<S>(
    socket: S,
    peer: SocketAddr,
    local: SocketAddr,
    router: Router,
    acceptor: Option<TlsAcceptor>,
) where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let end = ConnectionEnd::default();
    let served = if let Some(acceptor) = acceptor {
        let Ok(Ok(stream)) =
            tokio::time::timeout(std::time::Duration::from_secs(10), acceptor.accept(socket)).await
        else {
            tracing::debug!(%peer, "TLS handshake rejected");
            return;
        };
        let certificate = stream
            .get_ref()
            .1
            .peer_certificates()
            .and_then(|chain| chain.first())
            .map(|c| c.to_vec());
        let service = TowerToHyperService::new(
            router
                .layer(Extension(Connection {
                    peer,
                    local,
                    certificate,
                    tls: true,
                }))
                .layer(Extension(end.clone())),
        );
        Builder::new(TokioExecutor::new())
            .serve_connection_with_upgrades(TokioIo::new(stream), service)
            .await
    } else {
        let service = TowerToHyperService::new(
            router
                .layer(Extension(Connection {
                    peer,
                    local,
                    certificate: None,
                    tls: false,
                }))
                .layer(Extension(end.clone())),
        );
        Builder::new(TokioExecutor::new())
            .serve_connection_with_upgrades(TokioIo::new(socket), service)
            .await
    };
    end.finish(served.is_ok());
}
#[cfg(test)]
pub(crate) mod tests {
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
            "../../core/testdata/moonlight-tv-client.pem"
        ))
        .unwrap();
        let key = include_str!("../../core/testdata/moonlight-tv-client.key");
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
}
