use super::*;
use crate::tls::Connection;
use butterpollo_core::{rtsp::Negotiated, session::Role, state::Client};
use std::sync::{OnceLock, atomic::Ordering};

pub(crate) struct Fixture {
    pub host: Shared,
}
impl Fixture {
    pub fn new() -> Self {
        Self::load(Self::profile())
    }
    /// A new profile directory holding the fixture's host identity.
    pub fn profile() -> std::path::PathBuf {
        static IDENTITY: OnceLock<Identity> = OnceLock::new();
        let identity = IDENTITY.get_or_init(|| Identity::generate().unwrap());
        let directory =
            std::env::temp_dir().join(format!("butterpollo-parity-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(directory.join("credentials")).unwrap();
        std::fs::write(
            directory.join("credentials/cacert.pem"),
            &identity.certificate,
        )
        .unwrap();
        std::fs::write(
            directory.join("credentials/cakey.pem"),
            &identity.private_pem,
        )
        .unwrap();
        directory
    }
    /// The host from the profile in `directory`.
    pub fn load(directory: std::path::PathBuf) -> Self {
        let host = Host::load(directory.clone(), directory.join("assets"), None).unwrap();
        // Supply capabilities without starting the GPU probe or host runtime.
        host.codecs.store(0x1 | 0x100 | 0x200, Ordering::Release);
        host.probing_codecs.store(false, Ordering::Release);
        host.startup_codecs.send_replace(true);
        Self { host }
    }
    pub fn client(&self, perm: u32) -> Client {
        let client = Client {
            name: "Parity device".into(),
            cert: self.host.identity.certificate.clone(),
            uuid: "parity-device".into(),
            perm,
            enabled: true,
            extra: Default::default(),
        };
        self.host
            .paired
            .write()
            .unwrap()
            .add(&self.host.paired_path, client.clone())
            .unwrap();
        client
    }
    pub fn connection(&self, tls: bool) -> Connection {
        Connection {
            peer: "127.0.0.1:12345".parse().unwrap(),
            local: "127.0.0.1:47984".parse().unwrap(),
            certificate: tls.then(|| self.host.identity.der.clone()),
            tls,
        }
    }
    pub fn launch(&self, client: Client, role: Role) -> Launch {
        Launch {
            id: uuid::Uuid::new_v4().to_string(),
            client,
            peer: "127.0.0.1".parse().unwrap(),
            app_id: self.host.apps.read().unwrap()[0].id(),
            background: None,
            key: [0; 16],
            key_id: 1,
            ping: "parity".into(),
            connect_data: 1,
            role,
            created: Instant::now(),
            rtsp_encrypted: true,
            rtsp_counter: Default::default(),
            rtsp_received: Default::default(),
            preparation: Default::default(),
            vrr_requested: false,
            host_audio: false,
            requested_rate: 60_000,
            options: Default::default(),
            audio_preparation: Default::default(),
            preparing: Default::default(),
            transport: Default::default(),
            roaming_offered: Default::default(),
            claimed_until: None,
            nucleus: None,
            warnings: Default::default(),
        }
    }
    pub fn session(&self, client: Client, role: Role) -> Arc<Session> {
        let session = Session::new(self.launch(client, role), Negotiated::default());
        self.host
            .sessions
            .lock()
            .unwrap()
            .active
            .insert(session.launch.id.clone(), session.clone());
        session
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.host.stop_app(crate::state::AppStop::Test);
        // The journal writer must be done before the directory goes.
        self.host.session_flush();
        let _ = std::fs::remove_dir_all(&self.host.directory);
    }
}
