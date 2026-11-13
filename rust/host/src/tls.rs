use anyhow::Result;
use axum::body::Bytes;
use axum::{Extension, Router};
use hyper::{
    body::{Body, Frame, Incoming, SizeHint},
    server::conn::http1,
};
use hyper_util::{
    rt::{TokioIo, TokioTimer},
    service::TowerToHyperService,
};
use std::{
    collections::HashMap,
    future::Future,
    net::{IpAddr, SocketAddr},
    pin::Pin,
    sync::{Arc, Mutex, mpsc},
    task::{Context, Poll},
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::{OwnedSemaphorePermit, Semaphore},
};
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
    /// The request comes from the device's second screen (`rlSecondScreen=1` in its query):
    /// its client is [`butterpollo_core::state::Client::second_screen`]. Set per request by
    /// `nvhttp::router`, never by a connection.
    pub second_screen: bool,
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
/// What one HTTP listener (47984, 47989, 47990) allows its clients. Paired
/// devices polling serverinfo, a console in many tabs and file transfers
/// stay far inside it; a client that connects and stays silent, sends its
/// request slowly, or stalls its upload does not hold a connection for long.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Limits {
    /// Connections the listener serves at once.
    pub connections: usize,
    /// Connections one address may hold at once.
    pub per_peer: usize,
    /// A request's header must arrive within this, counted from when the
    /// connection waits for it: it also closes a keep-alive connection left
    /// idle this long between requests.
    pub header: Duration,
    /// A request body that sends nothing for this long fails its request; a
    /// body that keeps sending, however long, never does ([`IdleBody`]).
    pub body_idle: Duration,
}
impl Limits {
    pub(crate) const LISTENER: Self = Self {
        connections: 512,
        per_peer: 64,
        header: Duration::from_secs(10),
        body_idle: Duration::from_secs(60),
    };
}
pub async fn serve(
    address: SocketAddr,
    router: Router,
    acceptor: Option<TlsAcceptor>,
) -> Result<()> {
    let listener = crate::network::tcp(address)?;
    tracing::debug!(%address,tls=acceptor.is_some(),"HTTP listener ready");
    serve_listener(listener, router, acceptor, Limits::LISTENER).await
}
async fn serve_listener(
    listener: tokio::net::TcpListener,
    router: Router,
    acceptor: Option<TlsAcceptor>,
    limits: Limits,
) -> Result<()> {
    let admission = Admission::new(limits);
    let mut refused = Refusals::default();
    loop {
        let (socket, peer) = crate::network::accept(&listener).await;
        let Some(admitted) = admission.admit(peer.ip()) else {
            refused.refused(peer);
            continue;
        };
        if let Err(error) = socket.set_nodelay(true) {
            tracing::debug!(%peer, %error, "could not disable Nagle's algorithm");
        }
        let Ok(local) = socket.local_addr() else {
            tracing::debug!(%peer, "connection closed before it was served");
            continue;
        };
        let local = SocketAddr::new(local.ip().to_canonical(), local.port());
        let (router, acceptor) = (router.clone(), acceptor.clone());
        tokio::spawn(async move {
            let _admitted = admitted;
            serve_connection_with(socket, peer, local, router, acceptor, limits).await;
        });
    }
}
/// The listener's connection caps ([`Limits`]): a connection holds one
/// [`Admitted`] until it ends.
struct Admission {
    connections: Arc<Semaphore>,
    peers: Arc<Mutex<HashMap<IpAddr, usize>>>,
    per_peer: usize,
}
struct Admitted {
    _permit: OwnedSemaphorePermit,
    peer: IpAddr,
    peers: Arc<Mutex<HashMap<IpAddr, usize>>>,
}
impl Admission {
    fn new(limits: Limits) -> Self {
        Self {
            connections: Arc::new(Semaphore::new(limits.connections)),
            peers: Arc::default(),
            per_peer: limits.per_peer,
        }
    }
    /// `None` when the listener, or `peer`, already holds every connection
    /// it may.
    fn admit(&self, peer: IpAddr) -> Option<Admitted> {
        let permit = self.connections.clone().try_acquire_owned().ok()?;
        let peer = peer.to_canonical();
        let mut peers = self.peers.lock().unwrap();
        let held = peers.entry(peer).or_default();
        if *held >= self.per_peer {
            return None;
        }
        *held += 1;
        Some(Admitted {
            _permit: permit,
            peer,
            peers: self.peers.clone(),
        })
    }
}
impl Drop for Admitted {
    fn drop(&mut self) {
        let mut peers = self.peers.lock().unwrap();
        if let Some(held) = peers.get_mut(&self.peer) {
            *held -= 1;
            if *held == 0 {
                peers.remove(&self.peer);
            }
        }
    }
}
/// Connections closed over a cap, in the log at most every 10 s.
#[derive(Default)]
struct Refusals {
    last: Option<Instant>,
    skipped: u32,
}
impl Refusals {
    fn refused(&mut self, peer: SocketAddr) {
        let now = Instant::now();
        if self
            .last
            .is_some_and(|at| now.duration_since(at) < Duration::from_secs(10))
        {
            self.skipped += 1;
            return;
        }
        tracing::warn!(
            %peer,
            more = self.skipped,
            "HTTP listener closed a connection: the host, or this address, already has as many open as it allows"
        );
        self.last = Some(now);
        self.skipped = 0;
    }
}
/// A request body that fails once it has waited `idle` for the client with
/// nothing arriving, so a stalled upload frees its connection. The clock
/// runs only while the handler waits on the client: a body that keeps
/// sending, or a handler busy between reads, never trips it.
struct IdleBody<B> {
    inner: B,
    idle: Duration,
    waiting: Option<Pin<Box<tokio::time::Sleep>>>,
}
impl<B> Body for IdleBody<B>
where
    B: Body<Data = Bytes> + Unpin,
    B::Error: Into<axum::BoxError>,
{
    type Data = Bytes;
    type Error = axum::BoxError;
    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<std::result::Result<Frame<Bytes>, Self::Error>>> {
        let this = &mut *self;
        if let Poll::Ready(frame) = Pin::new(&mut this.inner).poll_frame(cx) {
            this.waiting = None;
            return Poll::Ready(frame.map(|frame| frame.map_err(Into::into)));
        }
        let idle = this.idle;
        let waiting = this
            .waiting
            .get_or_insert_with(|| Box::pin(tokio::time::sleep(idle)));
        if waiting.as_mut().poll(cx).is_ready() {
            let message = format!("the request body sent nothing for {idle:?}");
            return Poll::Ready(Some(Err(message.into())));
        }
        Poll::Pending
    }
    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }
    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}
/// One connection as a listener serves it, for tests that bring their own
/// socket.
#[cfg(test)]
pub(crate) async fn serve_connection<S>(
    socket: S,
    peer: SocketAddr,
    local: SocketAddr,
    router: Router,
    acceptor: Option<TlsAcceptor>,
) where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    serve_connection_with(socket, peer, local, router, acceptor, Limits::LISTENER).await;
}
/// Serves one accepted connection, over TLS with an acceptor, within
/// `limits`, and tells the handlers' [`ConnectionEnd`] how it ended.
async fn serve_connection_with<S>(
    socket: S,
    peer: SocketAddr,
    local: SocketAddr,
    router: Router,
    acceptor: Option<TlsAcceptor>,
    limits: Limits,
) where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    // HTTP/1 only: no listener offers HTTP/2 by ALPN, and the auto
    // builder's version sniffing waits on a silent client with no timeout.
    let mut builder = http1::Builder::new();
    builder
        .timer(TokioTimer::new())
        .header_read_timeout(limits.header);
    let idle = limits.body_idle;
    let bodies = move |request: axum::http::Request<Incoming>| {
        request.map(|inner| IdleBody {
            inner,
            idle,
            waiting: None,
        })
    };
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
        let service = TowerToHyperService::new(tower::ServiceExt::map_request(
            router
                .layer(Extension(Connection {
                    peer,
                    local,
                    certificate,
                    tls: true,
                    second_screen: false,
                }))
                .layer(Extension(end.clone())),
            bodies,
        ));
        builder
            .serve_connection(TokioIo::new(stream), service)
            .with_upgrades()
            .await
    } else {
        let service = TowerToHyperService::new(tower::ServiceExt::map_request(
            router
                .layer(Extension(Connection {
                    peer,
                    local,
                    certificate: None,
                    tls: false,
                    second_screen: false,
                }))
                .layer(Extension(end.clone())),
            bodies,
        ));
        builder
            .serve_connection(TokioIo::new(socket), service)
            .with_upgrades()
            .await
    };
    end.finish(served.is_ok());
}
#[cfg(test)]
pub(crate) mod tests;
