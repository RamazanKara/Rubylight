//! The native API v1 (`docs/native-api.md`) through the router: one test per
//! endpoint and per error code, and snapshots that pin the Moonlight
//! endpoints byte for byte.
use anyhow::Result;
use axum::{
    Extension,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use butterpollo_crypto::crypto::Identity;
use butterpollo_session::native::Scan;
use butterpollo_session::session::Role;
use butterpollo_session::state::{App, Client};
use butterpollo_stream::{
    display_session::test_support::retained_display,
    library,
    nvhttp::{self, VIEW, authenticated, serverinfo_response},
    session_actions::Failure,
    state::{Session, Shared, test_support::Fixture},
    tls::Connection,
};
use butterpollo_web::native_api::{self, ApiFailure};
use rubylight_native_api::{
    capabilities::Capabilities,
    error::ErrorCode,
    library::Library,
    session::{Owner, Session as ApiSession, State as SessionState},
};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceExt;

#[path = "native_tests/audio.rs"]
mod audio;
#[path = "native_tests/compat.rs"]
mod compat;
#[path = "native_tests/continue_here.rs"]
mod continue_here;
#[path = "native_tests/coop.rs"]
mod coop;
#[path = "native_tests/couch_menu.rs"]
mod couch_menu;
#[path = "native_tests/files.rs"]
mod files;
#[path = "native_tests/folders.rs"]
mod folders;
#[path = "native_tests/guests.rs"]
mod guests;
#[path = "native_tests/invites.rs"]
mod invites;
#[path = "native_tests/launcher.rs"]
mod launcher;
#[path = "native_tests/neural.rs"]
mod neural;
#[path = "native_tests/pairing.rs"]
mod pairing;
#[path = "native_tests/power.rs"]
mod power;
#[path = "native_tests/radeon.rs"]
mod radeon;
#[path = "native_tests/reply.rs"]
mod reply;
#[path = "native_tests/usb.rs"]
mod usb;
#[path = "native_tests/wake.rs"]
mod wake;

use reply::{HOST_VERSION, Reply, paired_device as device};

const LIST: u32 = 1 << 24;
const VIEW_BIT: u32 = 1 << 25;
const LAUNCH: u32 = 1 << 26;
const ALL: u32 = LIST | VIEW_BIT | LAUNCH;
const PREFIX: &str = "/api/native/v1";

fn router(h: Shared, https: bool) -> axum::Router {
    nvhttp::router_with(h, https, https.then(native_api::router))
}
async fn send(
    f: &Fixture,
    https: bool,
    connection: &Connection,
    method: &str,
    path: &str,
    content_type: Option<&str>,
    body: impl Into<Vec<u8>>,
) -> Reply {
    let mut request = Request::builder().method(method).uri(path);
    if let Some(content_type) = content_type {
        request = request.header(header::CONTENT_TYPE, content_type);
    }
    let response = nvhttp::router_with(f.host.clone(), https, https.then(native_api::router))
        .layer(Extension(connection.clone()))
        .layer(Extension(butterpollo_stream::tls::ConnectionEnd::default()))
        .oneshot(request.body(Body::from(body.into())).unwrap())
        .await
        .unwrap();
    let (parts, body) = response.into_parts();
    Reply {
        status: parts.status,
        headers: parts.headers,
        body: to_bytes(body, 1 << 20).await.unwrap().to_vec(),
    }
}
/// A request to `PREFIX` + `path` with no body.
async fn api(f: &Fixture, connection: &Connection, method: &str, path: &str) -> Reply {
    send(
        f,
        true,
        connection,
        method,
        &format!("{PREFIX}{path}"),
        None,
        vec![],
    )
    .await
}

/// Another paired device, with its own certificate.
fn other_device(f: &Fixture, perm: u32, host_control: bool) -> Connection {
    let identity = Identity::generate().unwrap();
    let mut client = Client {
        name: "Other device".into(),
        cert: identity.certificate.clone(),
        uuid: "other-device".into(),
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

/// A host with one paired device, which owns whatever the test starts.
struct Setup {
    f: Fixture,
    client: Client,
    connection: Connection,
}
impl Setup {
    fn new() -> Self {
        Self::with(ALL)
    }
    fn with(perm: u32) -> Self {
        let f = Fixture::new();
        let client = f.client(perm);
        let connection = f.connection(true);
        Self {
            f,
            client,
            connection,
        }
    }
    fn host(&self) -> &Shared {
        &self.f.host
    }
    /// The console grants or revokes Host control for the device.
    fn host_control(&self, granted: bool) {
        let mut paired = self.host().paired.write().unwrap();
        let extra = &mut paired.clients[0].extra;
        if granted {
            extra.insert("host_control".into(), true.into());
        } else {
            extra.remove("host_control");
        }
    }
    fn run_game(&self) {
        let app = self.host().apps.read().unwrap()[0].clone();
        let mut running =
            butterpollo_stream::process::RunningApp::with_environment(&app, Default::default())
                .unwrap();
        running.owner = self.client.uuid.clone();
        *self.host().current_app.lock().unwrap() = Some(running);
    }
    fn retain_display(&self) {
        let config = self.host().config.read().unwrap().clone();
        let display = retained_display(self.host(), &config);
        self.host()
            .app_display
            .lock()
            .unwrap()
            .insert(self.client.uuid.clone(), (display, None));
    }
    /// A stream whose launch copy of the device is `client`.
    fn stream_as(&self, client: Client) -> Arc<Session> {
        self.f.session(client, Role::Stream)
    }
    /// A game, its display and a stream: `streaming`.
    fn streaming(&self) -> Arc<Session> {
        self.run_game();
        self.retain_display();
        let session = self.stream_as(self.client.clone());
        assert_eq!(self.state(), SessionState::Streaming);
        session
    }
    fn state(&self) -> SessionState {
        self.host().session_snapshot().state
    }
    fn game_running(&self) -> bool {
        self.host().current_app.lock().unwrap().is_some()
    }
    fn displays(&self) -> usize {
        self.host().app_display.lock().unwrap().len()
    }
    async fn get(&self, path: &str) -> Reply {
        api(&self.f, &self.connection, "GET", path).await
    }
    async fn post(&self, path: &str) -> Reply {
        api(&self.f, &self.connection, "POST", path).await
    }
}

const SERVERINFO_UNPAIRED: &str = r#"<?xml version="1.0" encoding="utf-8"?><root status_code="200"><NetworkPathVersion>0</NetworkPathVersion><HardwareMetricsVersion>0</HardwareMetricsVersion><hostname>Snapshot</hostname><appversion>7.1.431.-1</appversion><GfeVersion>3.23.0.74</GfeVersion><uniqueid>{UNIQUE}</uniqueid><HttpsPort>47984</HttpsPort><ExternalPort>47989</ExternalPort><PairStatus>0</PairStatus><currentgame>0</currentgame><currentgameuuid></currentgameuuid><state>SUNSHINE_SERVER_FREE</state><LocalIP>127.0.0.1</LocalIP><mac>01:02:03:04:05:06</mac><MaxLumaPixelsHEVC>1869449984</MaxLumaPixelsHEVC><ServerCodecModeSupport>769</ServerCodecModeSupport><RustHostVersion>{VERSION}</RustHostVersion><RustHostEncoderBitrateKbps>0</RustHostEncoderBitrateKbps><RustHostInitialEncoderBitrateKbps>0</RustHostInitialEncoderBitrateKbps><RustHostLastFailure></RustHostLastFailure><RustHostSessionCount></RustHostSessionCount><RustHostPendingSessionCount></RustHostPendingSessionCount><RustHostApplicationActive></RustHostApplicationActive><RustHostProfile></RustHostProfile><PyroWaveHostLinkMbps>1000</PyroWaveHostLinkMbps><PyroWaveBandwidthProbeBytes>0</PyroWaveBandwidthProbeBytes><PyroWaveUdpProbeVersion>0</PyroWaveUdpProbeVersion><PyroWaveUdpHandshakeVersion>0</PyroWaveUdpHandshakeVersion><PyroWaveUdpProbeBurstVersion>0</PyroWaveUdpProbeBurstVersion><PyroWaveWireBudgetVersion>0</PyroWaveWireBudgetVersion><PyroWaveCriticalFecPercentage></PyroWaveCriticalFecPercentage><PyroWaveMinParityShards></PyroWaveMinParityShards><Permission>0</Permission><FrameLimiterSupported>1</FrameLimiterSupported><FrameLimiterEnabled>1</FrameLimiterEnabled><VirtualDisplayFrameLimiterEnabled>1</VirtualDisplayFrameLimiterEnabled><FrameLimiterFpsLimitMilliHz>0</FrameLimiterFpsLimitMilliHz><VirtualDisplayCapable>true</VirtualDisplayCapable><VirtualDisplayDriverReady>false</VirtualDisplayDriverReady><ControllerPreparation>1</ControllerPreparation><VirtualDisplayHDRCapable>true</VirtualDisplayHDRCapable><Packed444Modes>0</Packed444Modes><HdrFormats>79</HdrFormats><MaxVideoPacket>8192</MaxVideoPacket><FecControlVersion>1</FecControlVersion><BandwidthProbeVersion>1</BandwidthProbeVersion><AudioCodecs>3</AudioCodecs><AudioLayouts>15</AudioLayouts><TextInputVersion>2</TextInputVersion><KeyboardLayoutVersion>1</KeyboardLayoutVersion><PenBarrelRollVersion>1</PenBarrelRollVersion><PipelineRatesVersion>1</PipelineRatesVersion><DisplayScaleVersion>1</DisplayScaleVersion><SecondScreenVersion>1</SecondScreenVersion><FrameWarpProtocol>2</FrameWarpProtocol><ControllerIdentityVersion>1</ControllerIdentityVersion><HdrDynamicControl>1</HdrDynamicControl></root>"#;
const SERVERINFO_PAIRED: &str = r#"<?xml version="1.0" encoding="utf-8"?><root status_code="200"><NetworkPathVersion>1</NetworkPathVersion><HardwareMetricsVersion>1</HardwareMetricsVersion><hostname>Snapshot</hostname><appversion>7.1.431.-1</appversion><GfeVersion>3.23.0.74</GfeVersion><uniqueid>{UNIQUE}</uniqueid><HttpsPort>47984</HttpsPort><ExternalPort>47989</ExternalPort><PairStatus>1</PairStatus><currentgame>0</currentgame><currentgameuuid></currentgameuuid><state>SUNSHINE_SERVER_FREE</state><LocalIP>127.0.0.1</LocalIP><mac>01:02:03:04:05:06</mac><MaxLumaPixelsHEVC>1869449984</MaxLumaPixelsHEVC><ServerCodecModeSupport>769</ServerCodecModeSupport><RustHostVersion>{VERSION}</RustHostVersion><RustHostEncoderBitrateKbps>0</RustHostEncoderBitrateKbps><RustHostInitialEncoderBitrateKbps>0</RustHostInitialEncoderBitrateKbps><RustHostLastFailure></RustHostLastFailure><RustHostSessionCount></RustHostSessionCount><RustHostPendingSessionCount></RustHostPendingSessionCount><RustHostApplicationActive></RustHostApplicationActive><RustHostProfile></RustHostProfile><PyroWaveHostLinkMbps>1000</PyroWaveHostLinkMbps><PyroWaveBandwidthProbeBytes>33554432</PyroWaveBandwidthProbeBytes><PyroWaveUdpProbeVersion>1</PyroWaveUdpProbeVersion><PyroWaveUdpHandshakeVersion>1</PyroWaveUdpHandshakeVersion><PyroWaveUdpProbeBurstVersion>1</PyroWaveUdpProbeBurstVersion><PyroWaveWireBudgetVersion>1</PyroWaveWireBudgetVersion><PyroWaveCriticalFecPercentage>20</PyroWaveCriticalFecPercentage><PyroWaveMinParityShards>2</PyroWaveMinParityShards><Permission>119480064</Permission><FrameLimiterSupported>1</FrameLimiterSupported><FrameLimiterEnabled>1</FrameLimiterEnabled><VirtualDisplayFrameLimiterEnabled>1</VirtualDisplayFrameLimiterEnabled><FrameLimiterFpsLimitMilliHz>0</FrameLimiterFpsLimitMilliHz><VirtualDisplayCapable>true</VirtualDisplayCapable><VirtualDisplayDriverReady>false</VirtualDisplayDriverReady><ControllerPreparation>1</ControllerPreparation><VirtualDisplayHDRCapable>true</VirtualDisplayHDRCapable><PowerActionVersion>1</PowerActionVersion><SleepSupported>0</SleepSupported><SleepAllowed>0</SleepAllowed><RustHostInternetPlayVersion>0</RustHostInternetPlayVersion><Packed444Modes>0</Packed444Modes><HdrFormats>79</HdrFormats><MaxVideoPacket>8192</MaxVideoPacket><FecControlVersion>1</FecControlVersion><BandwidthProbeVersion>1</BandwidthProbeVersion><AudioCodecs>3</AudioCodecs><AudioLayouts>15</AudioLayouts><TextInputVersion>2</TextInputVersion><KeyboardLayoutVersion>1</KeyboardLayoutVersion><PenBarrelRollVersion>1</PenBarrelRollVersion><PipelineRatesVersion>1</PipelineRatesVersion><DisplayScaleVersion>1</DisplayScaleVersion><SecondScreenVersion>1</SecondScreenVersion><FrameWarpProtocol>2</FrameWarpProtocol><ControllerIdentityVersion>1</ControllerIdentityVersion><HdrDynamicControl>1</HdrDynamicControl></root>"#;
const APPLIST: &str = r#"<?xml version="1.0"?><root status_code="200"><App><AppTitle>Desktop</AppTitle><UUID>11111111-1111-4111-8111-111111111111</UUID><IDX>0</IDX><ID>958645192</ID><ArtVersion>958645192</ArtVersion><IsHdrSupported>1</IsHdrSupported></App><App><AppTitle>Dota 2</AppTitle><UUID>22222222-2222-4222-8222-222222222222</UUID><IDX>1</IDX><ID>831329605</ID><ArtVersion>831329605</ArtVersion><IsHdrSupported>1</IsHdrSupported></App><App><AppTitle>Remote Monitor</AppTitle><UUID>9a1c5a25-58fe-40e0-b9aa-7d3f00000005</UUID><IDX>0</IDX><ID>2147483505</ID><ArtVersion>remote-session-v6</ArtVersion><IsHdrSupported>1</IsHdrSupported></App></root>"#;
const CANCEL: &str =
    r#"<?xml version="1.0" encoding="utf-8"?><root status_code="200"><cancel>1</cancel></root>"#;

async fn serverinfo_xml(f: &Fixture, tls: bool) -> String {
    let mut connection = f.connection(tls);
    connection.peer = "192.168.1.5:1234".parse().unwrap();
    let client = authenticated(&f.host, &connection, 0).ok();
    let config = butterpollo_config::config::Config::parse("sunshine_name=Snapshot").unwrap();
    let response = serverinfo_response(
        &f.host,
        &connection,
        &axum::http::HeaderMap::new(),
        &config,
        None,
        client.as_ref(),
        true,
        "01:02:03:04:05:06".into(),
        1000,
        false,
    );
    String::from_utf8(
        to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}
fn golden(template: &str, f: &Fixture) -> String {
    template
        .replace("{UNIQUE}", &f.host.paired.read().unwrap().unique_id)
        .replace("{VERSION}", HOST_VERSION)
}

#[path = "native_tests/endpoints_tests.rs"]
mod endpoints_tests;
#[path = "native_tests/session_and_snapshots_tests.rs"]
mod session_and_snapshots_tests;
