//! Pooled [`NativeTransport`] implementation backed by [`HostClientPool`].
//!
//! Reuses one [`HostClient`] per `(address, http_port, pinned_cert)` across
//! `capabilities`, `library`, `session`, and `add_to_library` calls so each console
//! refresh builds its pinned `rustls::ClientConfig` once and avoids repeating
//! plain-HTTP `GET /serverinfo` HTTPS-port discovery when `host.https_port == 0`.

use super::repo::{HostDescriptor, NativeTransport};
use crate::hosts::client_pool::HostClientPool;
use crate::http::{ClientIdentity, Error, HostClient, pem_decode_certificate};
use nightjar_policy::http::native_api::session::Session;
use nightjar_policy::http::{AddedGame, NativeCapabilities, NativeLibrary};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

const MAX_CACHE_ENTRIES: usize = 64;

/// Production [`NativeTransport`] backed by a shared [`HostClientPool`].
pub struct HostClientTransport {
    pool: Arc<HostClientPool>,
    discovered_https: Mutex<HashMap<(String, u16), u16>>,
    cert_der_cache: Mutex<HashMap<String, Vec<u8>>>,
}

impl HostClientTransport {
    /// Creates a transport with its own [`HostClientPool`] for `identity`.
    pub fn new(identity: Arc<ClientIdentity>) -> Self {
        Self::with_pool(Arc::new(HostClientPool::new(identity)))
    }

    /// Creates a transport sharing an existing [`HostClientPool`].
    pub fn with_pool(pool: Arc<HostClientPool>) -> Self {
        Self {
            pool,
            discovered_https: Mutex::new(HashMap::new()),
            cert_der_cache: Mutex::new(HashMap::new()),
        }
    }

    fn cert_der(&self, pem: Option<&str>) -> Result<Option<Vec<u8>>, String> {
        let Some(pem) = pem else {
            return Ok(None);
        };
        if let Some(cached) = self.cert_der_cache.lock().unwrap().get(pem).cloned() {
            return Ok(Some(cached));
        }
        let der = pem_decode_certificate(pem).map_err(|e| e.to_string())?;
        let mut cache = self.cert_der_cache.lock().unwrap();
        if cache.len() >= MAX_CACHE_ENTRIES {
            cache.clear();
        }
        cache.insert(pem.to_owned(), der.clone());
        Ok(Some(der))
    }

    fn effective_https_port(&self, host: &HostDescriptor) -> u16 {
        if host.https_port > 0 {
            return host.https_port;
        }
        self.discovered_https
            .lock()
            .unwrap()
            .get(&(host.address.clone(), host.http_port))
            .copied()
            .unwrap_or(0)
    }

    fn remember_https_port(&self, host: &HostDescriptor, discovered: u16) {
        if host.https_port > 0 || discovered == 0 {
            return;
        }
        let mut cache = self.discovered_https.lock().unwrap();
        if cache.len() >= MAX_CACHE_ENTRIES {
            cache.clear();
        }
        cache.insert((host.address.clone(), host.http_port), discovered);
    }

    fn clear_discovered_https_port(&self, host: &HostDescriptor) {
        if host.https_port == 0 {
            self.discovered_https
                .lock()
                .unwrap()
                .remove(&(host.address.clone(), host.http_port));
        }
    }

    fn with_client<T>(
        &self,
        host: &HostDescriptor,
        f: impl FnOnce(&HostClient) -> Result<T, Error>,
    ) -> Result<T, String> {
        let cert_der = self.cert_der(host.server_cert_pem.as_deref())?;
        let https_port = self.effective_https_port(host);
        let (client, generation) = self
            .pool
            .take(
                &host.address,
                host.http_port,
                https_port,
                cert_der.as_deref(),
            )
            .map_err(|e| e.to_string())?;
        let result = f(&client);
        match &result {
            Ok(_) => self.remember_https_port(host, client.https_port_cached()),
            Err(_) => self.clear_discovered_https_port(host),
        }
        self.pool.give_back(
            &host.address,
            host.http_port,
            cert_der.as_deref(),
            generation,
            client,
        );
        result.map_err(|e| e.to_string())
    }
}

impl NativeTransport for HostClientTransport {
    fn capabilities(&self, host: &HostDescriptor) -> Result<NativeCapabilities, String> {
        self.with_client(host, HostClient::native_capabilities_with_files)
    }

    fn library(
        &self,
        host: &HostDescriptor,
        include_unadded: bool,
    ) -> Result<NativeLibrary, String> {
        self.with_client(host, |client| client.library_with_unadded(include_unadded))
    }

    fn session(&self, host: &HostDescriptor) -> Result<Session, String> {
        self.with_client(host, HostClient::session)
    }

    fn add_to_library(
        &self,
        host: &HostDescriptor,
        game_id: &str,
    ) -> Result<Option<AddedGame>, String> {
        self.with_client(host, |client| client.add_to_library(game_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::identity::tests::fixture;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn read_http_head(stream: &mut impl Read) -> String {
        let mut raw = Vec::new();
        let mut byte = [0u8; 1];
        while !raw.ends_with(b"\r\n\r\n") && raw.len() < 8192 {
            if stream.read_exact(&mut byte).is_err() {
                break;
            }
            raw.push(byte[0]);
        }
        String::from_utf8_lossy(&raw).into_owned()
    }

    #[test]
    fn host_client_transport_reuses_pooled_client_and_discovers_https_port_once() {
        let host_id = fixture();
        let cert_pem = host_id.certificate_pem().to_owned();
        let tls_cfg = Arc::new(
            rustls::ServerConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![CertificateDer::from(host_id.certificate_der().to_vec())],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
                    host_id.private_key_pkcs8().to_vec(),
                )),
            )
            .unwrap(),
        );

        let https_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let https_port = https_listener.local_addr().unwrap().port();
        let https_calls = Arc::new(AtomicUsize::new(0));
        let https_calls_srv = Arc::clone(&https_calls);
        let https_srv = std::thread::spawn(move || {
            for _ in 0..3 {
                let (mut sock, _) = https_listener.accept().unwrap();
                let mut conn = rustls::ServerConnection::new(Arc::clone(&tls_cfg)).unwrap();
                let mut tls = rustls::Stream::new(&mut conn, &mut sock);
                let head = read_http_head(&mut tls);
                https_calls_srv.fetch_add(1, Ordering::SeqCst);
                let body = if head.contains("/api/native/v1/capabilities") {
                    r#"{"api_version":1,"host":{"name":"PoolPC","version":"7.0.0","unique_id":"pc-pool-1"},"features":["library","session"],"permissions":{"list":true,"view":true,"launch":true,"host_control":true}}"#
                } else if head.contains("/api/native/v1/library") {
                    r#"{"revision":"r1","games":[]}"#
                } else if head.contains("/api/native/v1/session") {
                    r#"{"revision":1,"state":"idle","owner":"none","app":null,"game_running":false,"actions":[]}"#
                } else {
                    panic!("unexpected HTTPS request: {head}");
                };
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = tls.write_all(reply.as_bytes());
                let _ = tls.flush();
            }
        });

        let http_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let http_port = http_listener.local_addr().unwrap().port();
        let http_calls = Arc::new(AtomicUsize::new(0));
        let http_calls_srv = Arc::clone(&http_calls);
        let http_srv = std::thread::spawn(move || {
            let (mut sock, _) = http_listener.accept().unwrap();
            let _ = read_http_head(&mut sock);
            http_calls_srv.fetch_add(1, Ordering::SeqCst);
            let xml = format!(
                r#"<root status_code="200"><hostname>PoolPC</hostname><uniqueid>pc-pool-1</uniqueid><state>MJ_SERVER_FREE</state><PairStatus>0</PairStatus><HttpsPort>{https_port}</HttpsPort></root>"#
            );
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{xml}",
                xml.len()
            );
            let _ = sock.write_all(reply.as_bytes());
            let _ = sock.flush();
        });

        let transport = HostClientTransport::new(Arc::new(fixture()));
        let host = HostDescriptor {
            uuid: "pc-pool-1".into(),
            name: "PoolPC".into(),
            online: true,
            paired: true,
            is_native: true,
            address: "127.0.0.1".into(),
            http_port,
            https_port: 0,
            server_cert_pem: Some(cert_pem),
            classic_apps: None,
            running_app_id: None,
            running_app_uuid: None,
        };

        let caps = transport.capabilities(&host).unwrap();
        assert_eq!(caps.capabilities.host.version, "7.0.0");
        let lib = transport.library(&host, false).unwrap();
        assert_eq!(lib.library.revision, "r1");
        let session = transport.session(&host).unwrap();
        assert!(!session.game_running);

        http_srv.join().unwrap();
        https_srv.join().unwrap();
        assert_eq!(
            http_calls.load(Ordering::SeqCst),
            1,
            "HTTPS port should be discovered once and reused across capabilities/library/session"
        );
        assert_eq!(https_calls.load(Ordering::SeqCst), 3);
    }
}
