use crate::{
    state::{AppStop, Launch, PendingPin, Sessions, Shared},
    tls::Connection,
};
use anyhow::{Context, Result, bail};
#[cfg(not(test))]
use butterpollo_windows::{
    display::virtual_display_available,
    process::ensure_streaming_session as ensure_windows_streaming_session,
};
#[cfg(test)]
use parity_tests::ensure_streaming_session as ensure_windows_streaming_session;
#[cfg(test)]
fn virtual_display_available() -> bool {
    false
}
// Tests never touch the clipboard of the PC running them: they get the fake
// in parity_tests.
use axum::{
    Extension, Router,
    body::Bytes,
    extract::{Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use butterpollo_core::{
    crypto,
    pairing::Pairing,
    remote::{self, Control},
    session::{Role, Transport},
    state::{App, Client},
};
#[cfg(not(test))]
use butterpollo_windows::clipboard;
#[cfg(test)]
use parity_tests::clipboard;
use serde_json::json;
use std::{
    collections::{BTreeMap, HashMap},
    ops::ControlFlow,
    sync::Mutex,
    time::{Duration, Instant},
};
type Args = HashMap<String, String>;
/// How long discovery and launches wait for the startup encoder probe. Its
/// standard codecs took 0.6–1.3 s to publish on test PCs; five seconds also
/// cover a driver still starting with Windows. A probe stuck longer keeps
/// each request for no more than that, where 2.2.1 waited ten seconds.
const STARTUP_CODEC_WAIT: Duration = Duration::from_secs(5);
type ServerInfoRoute = Option<(std::net::IpAddr, std::net::SocketAddr)>;
type ServerInfoFacts = (String, u64, bool);

#[derive(Default)]
pub struct ServerInfoCache {
    entries: HashMap<ServerInfoRoute, (Instant, ServerInfoFacts)>,
}
impl ServerInfoCache {
    fn get(&mut self, route: ServerInfoRoute, now: Instant) -> Option<ServerInfoFacts> {
        self.entries
            .retain(|_, (at, _)| now.duration_since(*at) < Duration::from_secs(10));
        self.entries.get(&route).map(|(_, facts)| facts.clone())
    }
}
#[cfg(test)]
mod bandwidth_probe_tests;
#[cfg(test)]
mod native_tests;
#[cfg(test)]
pub(crate) mod parity_tests;
#[derive(Debug)]
struct LaunchFailure(u16, &'static str);
impl std::fmt::Display for LaunchFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.1)
    }
}
impl std::error::Error for LaunchFailure {}
const APPLICATION_NOT_FOUND: &str = "application not found";
pub(crate) fn wait_for_teardown(
    sessions: &Mutex<Sessions>,
    ids: &[String],
    timeout: Duration,
) -> bool {
    let started = Instant::now();
    loop {
        {
            let sessions = sessions.lock().unwrap();
            let active = ids.iter().any(|id| sessions.active.contains_key(id));
            if !active && !ids.iter().any(|id| sessions.teardown.contains_key(id)) {
                return true;
            }
            if started.elapsed() >= timeout {
                for id in ids {
                    if let Some(since) = sessions.teardown.get(id) {
                        tracing::warn!(session = %id, teardown_seconds = since.elapsed().as_secs_f64(), "session teardown exceeded the launch wait");
                    } else if sessions.active.contains_key(id) {
                        tracing::warn!(session = %id, waited_seconds = started.elapsed().as_secs_f64(), "session worker has not stopped within the launch wait");
                    }
                }
                return false;
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn remote_owner(h: &Shared, client: &str) -> butterpollo_core::remote::Owner {
    use butterpollo_core::remote::Owner;
    if h.monitors.lock().unwrap().contains_key(client) {
        return Owner::Monitor;
    }
    let sessions = h.sessions.lock().unwrap();
    if sessions
        .pending
        .values()
        .any(|s| s.client.uuid == client && s.role == Role::RemoteMonitor)
        || sessions.active.values().any(|s| {
            !s.stopping() && s.launch.client.uuid == client && s.launch.role == Role::RemoteMonitor
        })
    {
        Owner::Monitor
    } else if sessions
        .pending
        .values()
        .any(|s| s.client.uuid == client && s.role == Role::InputOnly)
        || sessions.active.values().any(|s| {
            !s.stopping() && s.launch.client.uuid == client && s.launch.role == Role::InputOnly
        })
    {
        Owner::Input
    } else {
        Owner::None
    }
}
fn remote_game(h: &Shared) -> Option<butterpollo_core::remote::Game> {
    let (id, name, owner, generation) = h.current_app.lock().unwrap().as_ref().map(|app| {
        (
            app.id,
            app.name.clone(),
            app.owner.clone(),
            app.generation.clone(),
        )
    })?;
    let uuid = h
        .apps
        .read()
        .unwrap()
        .iter()
        .find(|app| app.id() == id)
        .and_then(|app| app.extra.get("uuid"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_owned();
    Some(butterpollo_core::remote::Game {
        app: butterpollo_core::remote::Entry {
            id,
            title: name,
            uuid,
            art_version: id.to_string(),
            ..Default::default()
        },
        owner,
        generation,
    })
}
fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
pub fn xml(code: u16, fields: &[(&str, String)], error: Option<String>) -> Response {
    let mut s = format!("<?xml version=\"1.0\" encoding=\"utf-8\"?><root status_code=\"{code}\"");
    if let Some(error) = error {
        s.push_str(&format!(" status_message=\"{}\"", escape(&error)));
    }
    s.push('>');
    for (k, v) in fields {
        s.push_str(&format!("<{k}>{}</{k}>", escape(v)));
    }
    s.push_str("</root>");
    (
        [(header::CONTENT_TYPE, "application/xml; charset=utf-8")],
        s,
    )
        .into_response()
}
fn authenticated(h: &Shared, connection: &Connection, permission: u32) -> Result<Client> {
    if !connection.tls {
        bail!("TLS client authentication required");
    }
    let der = connection
        .certificate
        .as_deref()
        .context("client certificate required")?;
    let state = h.paired.read().unwrap();
    let c = state
        .client_by_certificate(der)
        .context("client is not paired or is disabled")?;
    if !c.allows(permission) {
        let permission = match permission {
            0x0100_0000 => "List applications",
            0x0200_0000 => "View streams",
            0x0400_0000 => "Launch applications",
            _ => "the requested operation",
        };
        bail!("client permission denied: enable {permission} for this device in the host console");
    }
    crate::wake::note_client_request(h);
    Ok(c.clone())
}
/// Watching a stream needs the view or the launch permission, as in
/// Vibepollo: a device that may launch a game may also resume it.
const VIEW: u32 = (1 << 25) | (1 << 26);
fn authenticated_viewer(h: &Shared, connection: &Connection) -> Result<Client> {
    let client = authenticated(h, connection, 0)?;
    if client.perm & VIEW == 0 {
        bail!(
            "client permission denied: enable View streams or Launch applications for this device in the host console"
        );
    }
    Ok(client)
}
fn stream_key_id(value: &str) -> Result<u32> {
    // Android sends Java's signed random int; desktop clients can send the
    // same 32 bits as an unsigned decimal. Both forms describe the same IV.
    value
        .parse::<u32>()
        .or_else(|_| value.parse::<i32>().map(|id| id as u32))
        .context("stream key ID must be a signed or unsigned 32-bit decimal integer")
}
pub fn router(h: Shared, https: bool) -> Router {
    let r = Router::new()
        .route("/serverinfo", get(serverinfo))
        .route("/pair", get(pair).post(pair))
        .route("/pair/", get(pair).post(pair))
        // Plain HTTP answers too, but only HTTPS can identify a client.
        .route("/unpair", get(unpair).post(unpair));
    let r = if https {
        r.route("/applist", get(applist))
            .route("/launch", get(launch))
            .route("/resume", get(resume))
            .route("/cancel", get(cancel))
            .route("/appasset", get(appasset))
            .route("/bitrate", get(bitrate))
            .route("/api/abr/capabilities", get(abr))
            .route("/api/hardware-metrics", get(hardware_metrics))
            .route("/api/network-path", get(network_path))
            .route("/pyrowave-bandwidth-probe", get(pyrowave_bandwidth))
            .route(
                butterpollo_core::bandwidth_probe::ROUTE,
                get(bandwidth_probe),
            )
            .route("/pyrowave-udp-probe", get(pyrowave_udp_probe))
            // An image may be larger than the 1 MiB below: `clipboard_write`
            // sets the limit for each type.
            .route(
                "/actions/clipboard",
                get(clipboard_read).merge(
                    axum::routing::post(clipboard_write)
                        .layer(axum::extract::DefaultBodyLimit::disable()),
                ),
            )
            // Sleep PC, as Vibepollo serves it. Every method reaches the
            // sleep route, which refuses all but POST in its own format.
            .route("/power-actions/v1", get(crate::power_action::capabilities))
            .route(
                "/power-actions/v1/sleep",
                axum::routing::any(crate::power_action::sleep),
            )
            .merge(crate::native_api::router())
    } else {
        r
    };
    r.layer(axum::extract::DefaultBodyLimit::max(1024 * 1024))
        .with_state(h)
}
async fn network_path(State(h): State<Shared>, Extension(c): Extension<Connection>) -> Response {
    if authenticated_viewer(&h, &c).is_err() {
        return ([(header::CACHE_CONTROL, "no-store")], StatusCode::FORBIDDEN).into_response();
    }
    let report = crate::network_path::inspect(c.peer.ip().to_canonical()).await;
    if authenticated_viewer(&h, &c).is_err() {
        return ([(header::CACHE_CONTROL, "no-store")], StatusCode::FORBIDDEN).into_response();
    }
    ([(header::CACHE_CONTROL, "no-store")], axum::Json(report)).into_response()
}

async fn hardware_metrics(
    State(h): State<Shared>,
    Extension(connection): Extension<Connection>,
) -> Response {
    let result = match authenticated_viewer(&h, &connection) {
        Ok(client) => {
            let result = crate::telemetry::sample(&h, client.uuid).await;
            if authenticated_viewer(&h, &connection).is_err() {
                Err((StatusCode::FORBIDDEN, "An authenticated viewer is required"))
            } else {
                result
            }
        }
        Err(_) => Err((StatusCode::FORBIDDEN, "An authenticated viewer is required")),
    };
    let response = match result {
        Ok(value) => (StatusCode::OK, axum::Json(value)),
        Err((status, error)) => (status, axum::Json(json!({"error": error}))),
    };
    ([(header::CACHE_CONTROL, "no-store")], response).into_response()
}
async fn serverinfo(
    State(h): State<Shared>,
    Extension(connection): Extension<Connection>,
    headers: axum::http::HeaderMap,
) -> Response {
    h.request_codec_probe();
    h.wait_for_startup_codecs(STARTUP_CODEC_WAIT).await;
    let config = h.config.read().unwrap().clone();
    let client = authenticated(&h, &connection, 0).ok();
    if let Some(client) = &client {
        note_client_agent(&h, &client.uuid, &headers);
    }
    let paired = client.is_some();
    // The peer port does not affect the routed interface. Keep its IPv6 scope.
    let mut peer = connection.peer;
    peer.set_port(0);
    let route = paired.then_some((connection.local.ip(), peer));
    let cached = h
        .serverinfo_cache
        .lock()
        .unwrap()
        .get(route, Instant::now());
    let (mac, pyrowave_link, driver_ready) = match cached {
        Some(facts) => facts,
        None => {
            let h = h.clone();
            tokio::task::spawn_blocking(move || {
                if let Some(facts) = h
                    .serverinfo_cache
                    .lock()
                    .unwrap()
                    .get(route, Instant::now())
                {
                    return facts;
                }
                // Unpaired requests never query or cache a private MAC or route.
                let (mac, link) = route.map_or_else(
                    || ("00:00:00:00:00:00".into(), 0),
                    |(local, peer)| {
                        (
                            butterpollo_windows::net::local_mac(local)
                                .unwrap_or_else(|_| "00:00:00:00:00:00".into()),
                            butterpollo_windows::net::routed_link_bps(peer) / 1_000_000,
                        )
                    },
                );
                let facts = (mac, link, virtual_display_available());
                h.serverinfo_cache
                    .lock()
                    .unwrap()
                    .entries
                    .insert(route, (Instant::now(), facts.clone()));
                facts
            })
            .await
            .unwrap_or_else(|_| ("00:00:00:00:00:00".into(), 0, false))
        }
    };
    let windows_11 = butterpollo_windows::display::virtual_display_os();
    if let Err(error) = crate::extensions::probe_serverinfo_audio(&config).await {
        tracing::warn!(error = %format!("{error:#}"), "Dolby Digital audio is not offered: FFmpeg was not probed");
    }
    serverinfo_response(
        &h,
        &connection,
        &config,
        client.as_ref(),
        windows_11,
        mac,
        pyrowave_link,
        driver_ready,
    )
}
/// Keeps what the device's client calls itself, for the console's warning
/// about known-broken client builds. A request without one clears it.
fn note_client_agent(h: &Shared, uuid: &str, headers: &axum::http::HeaderMap) {
    let agent = headers
        .get(axum::http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.trim().chars().take(256).collect::<String>())
        .unwrap_or_default();
    let mut agents = h.client_agents.lock().unwrap();
    if agent.is_empty() {
        agents.remove(uuid);
    } else {
        agents.insert(uuid.to_owned(), agent);
    }
}
#[allow(clippy::too_many_arguments)]
fn serverinfo_response(
    h: &Shared,
    connection: &Connection,
    config: &butterpollo_core::config::Config,
    client: Option<&Client>,
    windows_11: bool,
    mac: String,
    pyrowave_link: u64,
    driver_ready: bool,
) -> Response {
    let ports = config.ports().unwrap();
    let paired = client.is_some();
    let (limiter, virtual_limiter, limit) = butterpollo_core::framegen::advertised(
        config,
        config.virtual_display_mode(windows_11) != "disabled",
    );
    let permission = client.as_ref().map_or(0, |client| client.perm);
    // Artemis lists these commands; a client runs one by its index.
    let commands: Vec<String> = if permission & 0x0010_0000 != 0 {
        serde_json::from_str::<serde_json::Value>(config.get("server_cmd", "[]"))
            .ok()
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default()
            .iter()
            .map(|command| {
                command
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned()
            })
            .collect()
    } else {
        Vec::new()
    };
    // Moonlight stores LocalIP as the IPv4 LAN address; over IPv6 it expects
    // GFE's placeholder, as Vibepollo sends.
    let local_ip = match connection.local.ip().to_canonical() {
        std::net::IpAddr::V6(_) => "127.0.0.1".to_owned(),
        v4 => v4.to_string(),
    };
    let game = remote_game(h);
    // GameStream's public state is scoped to the requesting client. Local
    // maintenance needs the actual session counts, including queued launches.
    let local = connection.peer.ip().to_canonical().is_loopback();
    let (session_count, pending_count) = if local {
        let mut sessions = h.sessions.lock().unwrap();
        let expired = sessions.expire();
        let counts = (
            sessions.active.len().to_string(),
            sessions.pending.len().to_string(),
        );
        drop(sessions);
        drop(expired);
        counts
    } else {
        (String::new(), String::new())
    };
    // Runtime changes affect every stream of this client; the minima keep a
    // controller's initialization from raising one stream to another's rate.
    let (encoder_bitrate, initial_encoder_bitrate) = client
        .filter(|client| client.perm & VIEW != 0)
        .and_then(|client| {
            h.sessions
                .lock()
                .unwrap()
                .active
                .values()
                .filter(|session| {
                    session.launch.client.uuid == client.uuid
                        && session.launch.role != Role::InputOnly
                        && !session.stopping()
                })
                .map(|session| {
                    (
                        session.bitrate.load(std::sync::atomic::Ordering::Acquire),
                        session.config.bitrate_kbps,
                    )
                })
                .reduce(|(current, initial), (next_current, next_initial)| {
                    (current.min(next_current), initial.min(next_initial))
                })
        })
        .unwrap_or((0, 0));
    let current = game
        .as_ref()
        .filter(|game| {
            client.as_ref().is_some_and(|client| {
                (game.owner == client.uuid
                    && remote_owner(h, &client.uuid) == butterpollo_core::remote::Owner::None)
                    || h.confirmations.lock().unwrap().active(
                        &client.uuid,
                        butterpollo_core::remote::Confirmation::Replace,
                        &game.generation,
                    )
            })
        })
        .map_or(0, |game| game.app.id);
    let current_uuid = game
        .as_ref()
        .filter(|game| game.app.id == current && current != 0)
        .map(|game| game.app.uuid.clone())
        .unwrap_or_default();
    let mut fields = vec![
        (
            "NetworkPathVersion",
            u8::from(permission & VIEW != 0).to_string(),
        ),
        (
            "HardwareMetricsVersion",
            u8::from(permission & VIEW != 0 && config.boolean("hardware_metrics", false))
                .to_string(),
        ),
        // 0 for plain HTTP and unpaired requests; the native API
        // (`docs/native-api.md`) answers paired HTTPS only.
        (
            butterpollo_core::native_api::SERVERINFO_FIELD,
            if paired {
                butterpollo_core::native_api::API_VERSION
            } else {
                0
            }
            .to_string(),
        ),
        ("hostname", crate::network::host_name(config)),
        ("appversion", "7.1.431.-1".into()),
        ("GfeVersion", "3.23.0.74".into()),
        ("uniqueid", h.paired.read().unwrap().unique_id.clone()),
        ("HttpsPort", ports.https.to_string()),
        ("ExternalPort", ports.http.to_string()),
        ("PairStatus", u8::from(paired).to_string()),
        ("currentgame", current.to_string()),
        ("currentgameuuid", current_uuid),
        (
            "state",
            if current == 0 {
                "SUNSHINE_SERVER_FREE"
            } else {
                "SUNSHINE_SERVER_BUSY"
            }
            .into(),
        ),
        ("LocalIP", local_ip),
        ("mac", mac),
        (
            "MaxLumaPixelsHEVC",
            if h.codecs.load(std::sync::atomic::Ordering::Acquire) & 0x100 != 0 {
                "1869449984"
            } else {
                "0"
            }
            .into(),
        ),
        (
            "ServerCodecModeSupport",
            (h.codecs.load(std::sync::atomic::Ordering::Acquire) & !0x40000000).to_string(),
        ),
        ("RustHostVersion", env!("CARGO_PKG_VERSION").into()),
        ("RustHostEncoderBitrateKbps", encoder_bitrate.to_string()),
        (
            "RustHostInitialEncoderBitrateKbps",
            initial_encoder_bitrate.to_string(),
        ),
        ("RustHostSessionCount", session_count),
        ("RustHostPendingSessionCount", pending_count),
        (
            "RustHostApplicationActive",
            if local {
                u8::from(game.is_some()).to_string()
            } else {
                String::new()
            },
        ),
        (
            "RustHostProfile",
            if connection.peer.ip().to_canonical().is_loopback() {
                butterpollo_core::migration::profile_id(&h.directory)
            } else {
                String::new()
            },
        ),
        ("PyroWaveHostLinkMbps", pyrowave_link.to_string()),
        (
            "PyroWaveBandwidthProbeBytes",
            if paired { "33554432" } else { "0" }.into(),
        ),
        // `/pyrowave-udp-probe`, as Vibeshine serves it, and its wire budget:
        // a PyroWave bitrate is the stream's total on the wire, and each
        // record-framed picture gets what remains after packet overhead and
        // one critical block's parity (`pyrowave::budget`). Nonary's
        // calibration converts its measured total with the same two values.
        ("PyroWaveUdpProbeVersion", u8::from(paired).to_string()),
        ("PyroWaveUdpHandshakeVersion", u8::from(paired).to_string()),
        ("PyroWaveUdpProbeBurstVersion", u8::from(paired).to_string()),
        ("PyroWaveWireBudgetVersion", u8::from(paired).to_string()),
        (
            "PyroWaveCriticalFecPercentage",
            if paired {
                config
                    .integer("pyrowave_critical_fec_percentage", 20)
                    .clamp(0, 100)
                    .to_string()
            } else {
                String::new()
            },
        ),
        (
            "PyroWaveMinParityShards",
            if paired {
                butterpollo_core::pyrowave::MIN_PARITY_SHARDS.to_string()
            } else {
                String::new()
            },
        ),
        ("Permission", permission.to_string()),
        ("FrameLimiterSupported", "1".into()),
        ("FrameLimiterEnabled", u8::from(limiter).to_string()),
        (
            "VirtualDisplayFrameLimiterEnabled",
            u8::from(virtual_limiter).to_string(),
        ),
        ("FrameLimiterFpsLimitMilliHz", limit.to_string()),
        // Artemis offers virtual-display launches only when capable.
        (
            "VirtualDisplayCapable",
            (config.virtual_display_mode(windows_11) != "disabled").to_string(),
        ),
        ("VirtualDisplayDriverReady", driver_ready.to_string()),
        ("ControllerPreparation", "1".into()),
        (
            "VirtualDisplayHDRCapable",
            (config.virtual_display_mode(windows_11) != "disabled"
                && h.codecs.load(std::sync::atomic::Ordering::Acquire)
                    & (0x200 | 0x20000 | 0x2000000 | 0x4000000)
                    != 0)
                .to_string(),
        ),
    ];
    // Sleep PC (`/power-actions/v1`), for paired HTTPS clients only, as in
    // Vibepollo. Stock Moonlight ignores these.
    if let Some(client) = client {
        fields.extend([
            (
                "PowerActionVersion",
                butterpollo_core::power_action::VERSION.to_string(),
            ),
            ("SleepSupported", u8::from(h.power.supported()).to_string()),
            ("SleepAllowed", u8::from(client.allows_sleep()).to_string()),
        ]);
    }
    // DESCRIBE's `packed444Modes` mask, from the startup probe, so a client
    // can leave out a packed mode before /launch instead of after it.
    if config.boolean("experimental_packed_444", false) {
        fields.push((
            "Packed444Modes",
            h.packed_modes
                .load(std::sync::atomic::Ordering::Acquire)
                .to_string(),
        ));
    }
    fields.extend(crate::extensions::offered(h, config).serverinfo_fields());
    fields.extend(commands.into_iter().map(|name| ("ServerCommand", name)));
    xml(200, &fields, None)
}
async fn pyrowave_bandwidth(
    State(h): State<Shared>,
    Extension(connection): Extension<Connection>,
) -> Response {
    if authenticated(&h, &connection, 0).is_err() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    static PAYLOAD: std::sync::OnceLock<Bytes> = std::sync::OnceLock::new();
    let payload = PAYLOAD
        .get_or_init(|| Bytes::from(vec![0xa5; 32 * 1024 * 1024]))
        .clone();
    (
        [
            (header::CONTENT_TYPE, "application/octet-stream"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        payload,
    )
        .into_response()
}
/// `GET /bandwidth-probe?bytes=N` (`butterpollo_core::bandwidth_probe`): an
/// incompressible body a paired client times before it picks its first
/// bitrate, for every codec. Refused while a stream runs or starts, cut short
/// when one starts meanwhile, and past each client's cooldown and budget.
async fn bandwidth_probe(
    State(h): State<Shared>,
    Extension(connection): Extension<Connection>,
    Query(args): Query<Args>,
) -> Response {
    use butterpollo_core::bandwidth_probe as probe;
    let offered = {
        let config = h.config.read().unwrap();
        crate::extensions::bandwidth_probe(&h, &config)
    };
    if !offered {
        return StatusCode::NOT_FOUND.into_response();
    }
    let client = match authenticated(&h, &connection, 0) {
        Ok(client) => client,
        Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
    };
    let bytes = match probe::parse(args.get("bytes").map(String::as_str)) {
        Ok(bytes) => bytes,
        Err(error) => return (StatusCode::BAD_REQUEST, error.to_string()).into_response(),
    };
    let busy = {
        let mut sessions = h.sessions.lock().unwrap();
        let expired = sessions.expire();
        let busy = streaming(&sessions);
        drop(sessions);
        drop(expired);
        busy
    };
    if busy {
        return (
            StatusCode::CONFLICT,
            "Stop streaming before testing bandwidth",
        )
            .into_response();
    }
    let admitted = h
        .bandwidth_probes
        .lock()
        .unwrap()
        .admit(&client.uuid, bytes, Instant::now());
    if let Err(wait) = admitted {
        let seconds = wait.as_secs() + u64::from(wait.subsec_nanos() > 0);
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [(header::RETRY_AFTER, seconds.max(1).to_string())],
            "Bandwidth probe limit reached",
        )
            .into_response();
    }
    tracing::info!(client = %client.name, bytes, "bandwidth probe");
    (
        [
            (header::CONTENT_TYPE, "application/octet-stream"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        axum::body::Body::from_stream(tokio_util::io::ReaderStream::with_capacity(
            ProbeBody::new(
                bytes as usize,
                Instant::now() + probe::MAX_DURATION,
                stream_starts(&h),
            ),
            64 * 1024,
        )),
    )
        .into_response()
}
/// Whether a stream runs or starts, which a probe must not compete with.
fn streaming(sessions: &crate::state::Sessions) -> bool {
    !sessions.active.is_empty() || !sessions.pending.is_empty()
}
/// Whether a stream runs or starts on `h` now, for a probe body to stop at.
fn stream_starts(h: &Shared) -> impl Fn() -> bool + Send + Unpin + 'static {
    let h = h.clone();
    move || streaming(&h.sessions.lock().unwrap())
}
/// The bandwidth probe's body: random bytes, which no compression on the path
/// can shrink, until `remaining` is sent, the deadline passes or `busy` says a
/// stream runs or starts.
struct ProbeBody<F> {
    remaining: usize,
    offset: usize,
    deadline: Instant,
    busy: F,
}
impl<F: Fn() -> bool> ProbeBody<F> {
    fn new(remaining: usize, deadline: Instant, busy: F) -> Self {
        Self {
            remaining,
            offset: 0,
            deadline,
            busy,
        }
    }
    fn noise() -> &'static [u8] {
        static NOISE: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
        NOISE.get_or_init(|| {
            use rand::RngCore;
            let mut bytes = vec![0; butterpollo_core::bandwidth_probe::MAX_BYTES as usize];
            rand::thread_rng().fill_bytes(&mut bytes);
            bytes
        })
    }
}
impl<F: Fn() -> bool + Unpin> tokio::io::AsyncRead for ProbeBody<F> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        // The body ends early at the deadline, or when a stream runs or
        // starts: the client times what arrived.
        if self.remaining > 0 && (self.busy)() {
            tracing::info!(
                unsent = self.remaining,
                "bandwidth probe stopped for a stream"
            );
            self.remaining = 0;
        }
        if self.remaining > 0 && Instant::now() < self.deadline {
            let noise = Self::noise();
            let n = self
                .remaining
                .min(buf.remaining())
                .min(noise.len() - self.offset);
            buf.put_slice(&noise[self.offset..self.offset + n]);
            self.offset = (self.offset + n) % noise.len();
            self.remaining -= n;
        }
        std::task::Poll::Ready(Ok(()))
    }
}
/// Vibeshine's paired UDP probe: two seconds of numbered packets from this
/// host to the client's own address, so the client can count what a rate, or
/// whole frames at a pace, lose on its path. Refused while anything streams.
async fn pyrowave_udp_probe(
    State(h): State<Shared>,
    Extension(connection): Extension<Connection>,
    Query(args): Query<Args>,
) -> Response {
    static RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if authenticated(&h, &connection, 0).is_err() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Some(request) =
        butterpollo_core::pyrowave_probe::parse(|key| args.get(key).map(String::as_str))
    else {
        return (StatusCode::BAD_REQUEST, "Invalid UDP probe parameters").into_response();
    };
    let streaming = {
        let mut sessions = h.sessions.lock().unwrap();
        let expired = sessions.expire();
        let busy = !sessions.active.is_empty() || !sessions.pending.is_empty();
        drop(sessions);
        drop(expired);
        busy
    };
    if streaming || RUNNING.swap(true, std::sync::atomic::Ordering::AcqRel) {
        return (
            StatusCode::BAD_REQUEST,
            "Stop streaming before testing bandwidth",
        )
            .into_response();
    }
    struct Release;
    impl Drop for Release {
        fn drop(&mut self) {
            RUNNING.store(false, std::sync::atomic::Ordering::Release);
        }
    }
    let release = Release;
    let peer = std::net::SocketAddr::new(connection.peer.ip().to_canonical(), request.port);
    let socket = match crate::pyrowave_probe::bind(connection.local.ip().to_canonical()) {
        Ok(socket) => socket,
        Err(error) => {
            drop(release);
            tracing::warn!(error = %format!("{error:#}"), "PyroWave UDP probe could not bind");
            return (StatusCode::INTERNAL_SERVER_ERROR, "UDP probe failed").into_response();
        }
    };
    let port = socket.local_addr().map_or(0, |a| a.port());
    let handshake = request.handshake;
    let probe = tokio::task::spawn_blocking(move || {
        let _release = release;
        let outcome = crate::pyrowave_probe::send(&socket, peer, &request);
        match &outcome {
            Ok(o) => tracing::info!(
                kbps = request.kbps,
                burst_fps = request.burst.map_or(0, |b| b.fps),
                pace_kbps = request.burst.map_or(0, |b| b.pace_kbps),
                sent = o.sent,
                expected = o.expected,
                elapsed_ms = o.elapsed.as_secs_f64() * 1000.,
                send_retries = o.send_retries,
                last_send_error = o.last_send_error,
                "PyroWave UDP probe"
            ),
            Err(error) => {
                tracing::warn!(error = %format!("{error:#}"), "PyroWave UDP probe failed")
            }
        }
        outcome
    });
    let body = |outcome: Result<crate::pyrowave_probe::Outcome>| match outcome {
        Ok(o) => (
            200,
            vec![
                ("expected", o.expected.to_string()),
                ("sent", o.sent.to_string()),
                (
                    "elapsedMs",
                    format!("{:.3}", o.elapsed.as_secs_f64() * 1000.),
                ),
                ("sendRetries", o.send_retries.to_string()),
                ("lastSendError", o.last_send_error.to_string()),
            ],
            None,
        ),
        Err(error) => (500, vec![], Some(format!("{error:#}"))),
    };
    if !handshake {
        let (code, fields, error) = body(probe.await.unwrap_or_else(|e| Err(e.into())));
        if code != 200 {
            return (StatusCode::INTERNAL_SERVER_ERROR, "UDP probe failed").into_response();
        }
        return xml(code, &fields, error);
    }
    // The client learns the host's UDP port from the headers, sends its token
    // there, and reads the result once the probe has run.
    let (mut writer, reader) = tokio::io::duplex(4096);
    tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        let (code, fields, error) = body(probe.await.unwrap_or_else(|e| Err(e.into())));
        let response = xml(code, &fields, error);
        if let Ok(bytes) = axum::body::to_bytes(response.into_body(), 64 * 1024).await {
            let _ = writer.write_all(&bytes).await;
        }
    });
    (
        [
            (
                header::CONTENT_TYPE,
                "application/xml; charset=utf-8".to_owned(),
            ),
            (header::CACHE_CONTROL, "no-store".to_owned()),
            (
                header::HeaderName::from_static("x-pyrowave-udp-port"),
                port.to_string(),
            ),
        ],
        axum::body::Body::from_stream(tokio_util::io::ReaderStream::new(reader)),
    )
        .into_response()
}
async fn pair(
    State(h): State<Shared>,
    Extension(connection): Extension<Connection>,
    Query(args): Query<Args>,
) -> Response {
    match do_pair(h, &args, connection.peer.ip().to_canonical()).await {
        Ok(fields) => xml(
            200,
            &fields
                .iter()
                .map(|(k, v)| (k.as_str(), v.clone()))
                .collect::<Vec<_>>(),
            None,
        ),
        Err(e) => xml(400, &[("paired", "0".into())], Some(e.to_string())),
    }
}
/// Whether a pairing request has to wait for another device's: Moonlight's
/// apps share one unique ID, so only the certificate tells devices apart. The
/// same device asking again replaces its own request, as after a wrong PIN.
fn another_device_pairing(existing: Option<(&str, bool)>, certificate: &str) -> bool {
    existing.is_some_and(|(theirs, active)| active && theirs != certificate)
}
fn pairing_id(id: &str, peer: std::net::IpAddr) -> String {
    format!(
        "{}_{}",
        hex::encode(crypto::hash(id.as_bytes())),
        hex::encode(crypto::hash(peer.to_canonical().to_string().as_bytes()))
    )
}
async fn do_pair(h: Shared, args: &Args, peer: std::net::IpAddr) -> Result<Vec<(String, String)>> {
    let id = args.get("uniqueid").context("missing uniqueid")?.clone();
    if id.is_empty()
        || id.len() > 256
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
    {
        bail!("invalid uniqueid");
    }
    // Moonlight shares wire IDs across devices; one LAN peer must not reserve
    // that ID for every other device waiting to pair.
    let id = pairing_id(&id, peer);
    if !h.config.read().unwrap().boolean("enable_pairing", true) {
        bail!("pairing is disabled");
    }
    if args.get("phrase").is_some_and(|s| s == "getservercert") {
        let text = args
            .get("clientcert")
            .context("missing client certificate")?;
        if text.len() > 32768 {
            bail!("certificate too large");
        }
        let certificate = String::from_utf8(hex::decode(text)?)?;
        crypto::public_key(&certificate)?;
        let salt = hex::decode(args.get("salt").context("missing salt")?)?;
        if salt.len() < 16 || salt.len() > 32 {
            bail!("invalid salt length");
        }
        let name = args
            .get("devicename")
            .cloned()
            .unwrap_or("Moonlight Client".into());
        butterpollo_core::pairing::validate_device_name(&name)?;
        // Another device's request still waiting for its PIN, or in the
        // seconds of its handshake, is not replaced: the PIN typed for it
        // would reach this device instead.
        let waiting = h.pins.lock().unwrap().get(&id).map(|p| {
            (
                p.certificate.clone(),
                p.created.elapsed() < Duration::from_secs(300) && !p.sender.is_closed(),
            )
        });
        let handshaking = h.pairings.lock().unwrap().sessions.get(&id).map(|p| {
            (
                p.certificate.clone(),
                p.created.elapsed() < Duration::from_secs(30),
            )
        });
        if waiting.as_ref().is_some_and(|(_, active)| *active)
            || another_device_pairing(
                handshaking.as_ref().map(|(c, a)| (c.as_str(), *a)),
                &certificate,
            )
        {
            bail!("another device is pairing with this PC; finish or cancel that first");
        }
        // A new certificate request replaces this device's unfinished
        // pairing, as in the previous host. Moonlight abandons a pairing after
        // a wrong PIN without telling the host, and its apps share one unique
        // ID, so the stale session refused every later attempt until restart.
        h.pairings.lock().unwrap().sessions.remove(&id);
        if h.pairings
            .lock()
            .unwrap()
            .sessions
            .values()
            .filter(|p| p.peer == Some(peer) && p.created.elapsed() < Duration::from_secs(300))
            .count()
            >= 2
        {
            bail!("pairing request limit reached for this address");
        }
        if let Some(auth) = args.get("otpauth") {
            // One-time PIN pairing (Artemis). A wrong hash still gets an
            // ordinary answer, with a random PIN that fails the next step.
            let mut otp = h
                .otp
                .lock()
                .unwrap()
                .take()
                .filter(|otp| otp.created.elapsed() < Duration::from_secs(180))
                .context("OTP pairing is not available")?;
            let salt_text = args.get("salt").map_or("", String::as_str);
            let expected = hex::encode_upper(crypto::hash(
                format!("{}{salt_text}{}", otp.pin, otp.passphrase).as_bytes(),
            ));
            let (pin, name) = if expected.eq_ignore_ascii_case(auth) {
                tracing::info!(client=%name, "Pairing with a one-time PIN");
                let name = if otp.device_name.is_empty() {
                    name
                } else {
                    otp.device_name
                };
                (otp.pin, name)
            } else {
                tracing::warn!(client=%name, "One-time PIN pairing failed");
                otp.attempts += 1;
                if otp.attempts < 5 {
                    *h.otp.lock().unwrap() = Some(otp);
                }
                (
                    format!(
                        "{:04}",
                        rand::Rng::gen_range(&mut rand::thread_rng(), 0..10_000u16)
                    ),
                    name,
                )
            };
            let mut pair = Pairing::new(id, name, certificate, &salt, &pin)?;
            pair.peer = Some(peer);
            h.pairings.lock().unwrap().insert(pair)?;
            return Ok(vec![
                ("paired".into(), "1".into()),
                (
                    "plaincert".into(),
                    hex::encode(h.identity.certificate.as_bytes()),
                ),
            ]);
        }
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let created = Instant::now();
        {
            let mut pins = h.pins.lock().unwrap();
            pins.retain(|_, p| p.created.elapsed() < Duration::from_secs(300));
            // Replacing an older request ends its wait for the PIN.
            pins.remove(&id);
            if pins.len() >= 32 {
                bail!("pairing request limit reached");
            }
            let address_key = id.rsplit_once('_').unwrap().1;
            if pins.keys().filter(|key| key.ends_with(address_key)).count() >= 2 {
                bail!("pairing request limit reached for this address");
            }
            pins.insert(
                id.clone(),
                PendingPin {
                    name: name.clone(),
                    certificate: certificate.clone(),
                    created,
                    sender,
                },
            );
        }
        tracing::info!(client=%name,"Pairing PIN required in the web interface");
        butterpollo_windows::tray::notify(
            "Pairing request",
            &format!("{name} wants to pair. Enter the PIN it shows in the Rubylight console."),
        );
        let response = tokio::time::timeout(Duration::from_secs(300), receiver).await;
        {
            // Only this request's entry: a newer request may have replaced it.
            let mut pins = h.pins.lock().unwrap();
            if pins.get(&id).is_some_and(|p| p.created == created) {
                pins.remove(&id);
            }
        }
        let (pin, name) = response
            .context("PIN entry timed out")?
            .context("pairing cancelled")?;
        let mut pair = Pairing::new(id, name, certificate, &salt, &pin)?;
        pair.peer = Some(peer);
        h.pairings.lock().unwrap().insert(pair)?;
        return Ok(vec![
            ("paired".into(), "1".into()),
            (
                "plaincert".into(),
                hex::encode(h.identity.certificate.as_bytes()),
            ),
        ]);
    }
    if args.get("phrase").is_some_and(|s| s == "pairchallenge") {
        return Ok(vec![("paired".into(), "1".into())]);
    }
    // A malformed step is refused before the pairing is touched: from any
    // other device it must not end someone else's pairing.
    enum Step {
        Challenge(Vec<u8>),
        Response(Vec<u8>),
        Secret(Vec<u8>),
    }
    let step = if let Some(v) = args.get("clientchallenge") {
        if v.len() != 32 {
            bail!("invalid challenge length");
        }
        Step::Challenge(hex::decode(v)?)
    } else if let Some(v) = args.get("serverchallengeresp") {
        if v.len() != 64 {
            bail!("invalid challenge response length");
        }
        Step::Response(hex::decode(v)?)
    } else if let Some(v) = args.get("clientpairingsecret") {
        if v.len() != 544 {
            bail!("invalid pairing secret length");
        }
        Step::Secret(hex::decode(v)?)
    } else {
        bail!("unknown pairing phase")
    };
    let mut pairings = h.pairings.lock().unwrap();
    pairings.expire();
    let p = pairings
        .sessions
        .get_mut(&id)
        .context("no pending pairing")?;
    if p.peer.is_some_and(|started| started != peer) {
        bail!("this pairing was started from another address");
    }
    let result = (|| -> Result<Vec<(String, String)>> {
        match &step {
            Step::Challenge(challenge) => {
                let response = p.client_challenge(&h.identity, challenge)?;
                Ok(vec![
                    ("paired".into(), "1".into()),
                    ("challengeresponse".into(), hex::encode(response)),
                ])
            }
            Step::Response(response) => {
                let response = p.server_response(&h.identity, response)?;
                Ok(vec![
                    ("paired".into(), "1".into()),
                    ("pairingsecret".into(), hex::encode(response)),
                ])
            }
            Step::Secret(secret) => {
                p.finish(secret)?;
                let mut state = h.paired.write().unwrap();
                let perm = state.new_device_permission();
                let uuid = uuid::Uuid::new_v4().to_string();
                state.add(
                    &h.paired_path,
                    Client {
                        name: p.name.clone(),
                        cert: p.certificate.clone(),
                        uuid: uuid.clone(),
                        perm,
                        enabled: true,
                        extra: BTreeMap::new(),
                    },
                )?;
                drop(state);
                tracing::info!(client=%p.name,"Client paired and saved");
                h.webhook(
                    "pairing.paired",
                    serde_json::json!({"device": crate::webhooks::device(&uuid, &p.name), "transport": "moonlight"}),
                );
                butterpollo_windows::tray::notify(
                    "Device paired",
                    &format!("{} can now stream from this PC.", p.name),
                );
                Ok(vec![("paired".into(), "1".into())])
            }
        }
    })();
    if result.is_err() || matches!(step, Step::Secret(_)) {
        pairings.sessions.remove(&id);
    }
    result
}
async fn applist(
    State(h): State<Shared>,
    Extension(connection): Extension<Connection>,
) -> Response {
    let client = match authenticated(&h, &connection, 0) {
        Ok(c) => c,
        Err(e) => return xml(401, &[], Some(e.to_string())),
    };
    if !client.allows(1 << 24) {
        // As in Vibepollo: one entry that tells the user what to change.
        return ([(header::CONTENT_TYPE, "application/xml")], "<?xml version=\"1.0\"?><root status_code=\"200\"><App><IsHdrSupported>0</IsHdrSupported><AppTitle>Permission denied - enable &quot;List applications&quot; for this device in the host's Web UI</AppTitle><UUID></UUID><IDX>0</IDX><ID>114514</ID></App></root>").into_response();
    }
    h.request_codec_probe();
    h.wait_for_startup_codecs(STARTUP_CODEC_WAIT).await;
    let configured = h
        .apps
        .read()
        .unwrap()
        .iter()
        .enumerate()
        .map(|(index, app)| remote::Entry {
            id: app.id(),
            title: app.name.clone(),
            index,
            uuid: app
                .extra
                .get("uuid")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .into(),
            art_version: app.id().to_string(),
        })
        .collect();
    let config = h.config.read().unwrap().clone();
    let game = remote_game(&h);
    let projection = remote::project(
        &client.uuid,
        remote_owner(&h, &client.uuid),
        game.as_ref(),
        config.boolean("enable_input_only_mode", false),
        configured,
    );
    let entries: Vec<_> = projection
        .entries
        .into_iter()
        .filter(|entry| {
            let permission = match remote::identify(entry.id, &entry.uuid) {
                Some(Control::Resume | Control::RunningGame) => VIEW,
                Some(_) => 1 << 26,
                None => 1 << 24,
            };
            client.perm & permission != 0
        })
        .collect();
    let legacy = config.boolean("legacy_ordering", false)
        && client
            .extra
            .get("enable_legacy_ordering")
            .is_none_or(|value| value != false && value != "false");
    let hdr = u8::from(
        h.codecs.load(std::sync::atomic::Ordering::Acquire)
            & (0x200 | 0x20000 | 0x2000000 | 0x4000000)
            != 0,
    );
    let mut s = "<?xml version=\"1.0\"?><root status_code=\"200\">".to_owned();
    for (index, entry) in entries.iter().enumerate() {
        let title = if legacy {
            remote::ordered_title(&entry.title, entries.len(), index)
        } else {
            entry.title.clone()
        };
        s.push_str(&format!("<App><AppTitle>{}</AppTitle><UUID>{}</UUID><IDX>{}</IDX><ID>{}</ID><ArtVersion>{}</ArtVersion><IsHdrSupported>{hdr}</IsHdrSupported></App>", escape(&title), escape(&entry.uuid), entry.index, entry.id, escape(&entry.art_version)));
    }
    s.push_str("</root>");
    ([(header::CONTENT_TYPE, "application/xml")], s).into_response()
}
// Launching prepares displays and audio and runs prep commands, for seconds
// to minutes: on the blocking pool, not on the workers that serve every
// other request.
async fn launch(
    State(h): State<Shared>,
    Extension(connection): Extension<Connection>,
    Query(args): Query<Args>,
) -> Response {
    blocking(move || start(h, connection, args, false)).await
}
async fn resume(
    State(h): State<Shared>,
    Extension(connection): Extension<Connection>,
    Query(args): Query<Args>,
) -> Response {
    blocking(move || start(h, connection, args, true)).await
}
async fn blocking(work: impl FnOnce() -> Response + Send + 'static) -> Response {
    tokio::task::spawn_blocking(work)
        .await
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}
pub(crate) fn check_streaming_session(h: &Shared, phase: &'static str) -> Result<()> {
    #[cfg(test)]
    let result = match &h.reconnect_fixture {
        Some(fixture) => fixture.check_streaming_session(),
        None => ensure_windows_streaming_session(),
    };
    #[cfg(not(test))]
    let result = {
        let _ = h;
        ensure_windows_streaming_session()
    };
    // Success is useful evidence too: launch preparation can take seconds or
    // minutes, so the final accepted-launch timestamp is not the first check.
    tracing::info!(
        phase,
        ready = result.is_ok(),
        "Windows streaming session gate sampled"
    );
    result
}
fn start(h: Shared, connection: Connection, args: Args, resume: bool) -> Response {
    let requested = args
        .get("appid")
        .and_then(|id| id.parse::<u32>().ok())
        .unwrap_or(0);
    let active = remote_game(&h);
    let control = remote::identify(requested, args.get("appuuid").map_or("", String::as_str))
        .or_else(|| {
            active
                .as_ref()
                .filter(|game| requested == remote::running_game_id(game.app.id))
                .map(|_| Control::RunningGame)
        });
    let authorized = validate_launch_client(&h, &connection, control, resume);
    let client = match authorized {
        Ok(c) => c,
        Err(error) => {
            tracing::warn!(%error, app_id = requested, resume, "Moonlight launch authorization failed");
            return xml(401, &[], Some(error.to_string()));
        }
    };
    let _transition = h.launch_transition.lock().unwrap();
    // The reply names the endpoint Moonlight called. Android and iOS read
    // <gamesession> from /launch and fail without it, also when the launch
    // joins the running game.
    let reply = if resume { "resume" } else { "gamesession" };
    if control != Some(Control::Terminate)
        && let Err(error) = check_streaming_session(
            &h,
            if resume {
                "resume_request"
            } else {
                "launch_request"
            },
        )
    {
        return launch_response(Err(error), requested, reply, resume, false);
    }
    h.request_codec_probe();
    let owner = match validate_launch_request(&h, &client, control) {
        ControlFlow::Continue(owner) => owner,
        ControlFlow::Break(response) => return response,
    };
    let resume = resume
        || matches!(control, Some(Control::Resume | Control::RunningGame))
        || (control == Some(Control::Monitor) && owner == remote::Owner::Monitor);
    let result = (|| -> Result<(String, String)> {
        let key: [u8; 16] = hex::decode(args.get("rikey").context("missing stream key")?)?
            .try_into()
            .map_err(|_| anyhow::anyhow!("invalid stream key length"))?;
        let key_id = stream_key_id(args.get("rikeyid").context("missing stream key ID")?)?;
        let (role, app, app_id) =
            resolve_launch_app(&h, &args, &client, requested, control, owner, resume)?;
        if role != Role::InputOnly {
            h.wait_for_startup_codecs_blocking(STARTUP_CODEC_WAIT);
        }
        if role != Role::InputOnly
            && h.codecs.load(std::sync::atomic::Ordering::Acquire) & 0x0783_0301 == 0
        {
            return Err(LaunchFailure(503, butterpollo_core::amd_gpu::NO_ENCODER_LAUNCH).into());
        }
        let mut launch =
            build_launch_session(&connection, &args, client, app_id, role, key, key_id);
        // A refused `rlFecPercentage` fails the launch rather than streaming
        // with protection the client did not ask for.
        butterpollo_core::extensions::fec_choice(&launch.options)?;
        launch.background = Some(validate_background_launch(&h, &launch)?);
        let rtsp_port = h.config.read().unwrap().ports()?.rtsp;
        if !launch.rtsp_encrypted
            && crate::network::encryption_mode(&h.config.read().unwrap(), connection.peer.ip()) == 2
        {
            bail!("encrypted RTSP is required for this client address");
        }
        let scheme = if launch.rtsp_encrypted {
            "rtspenc"
        } else {
            "rtsp"
        };
        launch_back_half(&h, &args, launch, app, resume, owner)?;
        let host = match connection.local.ip() {
            std::net::IpAddr::V4(ip) => ip.to_string(),
            std::net::IpAddr::V6(ip) => format!("[{ip}]"),
        };
        Ok((reply.into(), format!("{scheme}://{host}:{rtsp_port}")))
    })();
    let driver_ready = result.is_ok() && butterpollo_windows::display::virtual_display_available();
    launch_response(result, requested, reply, resume, driver_ready)
}
/// What a launch does once its client is known and the request is parsed, the same for a
/// Moonlight and a WebRTC launch: end the sessions it replaces, wait for their resources, begin
/// the session state, queue the launch, prepare the display, controllers and app, and keep the
/// app. The caller holds `launch_transition`. The launch stays queued until its transport starts it.
fn launch_back_half(
    h: &Shared,
    args: &Args,
    launch: Launch,
    app: Option<App>,
    resume: bool,
    owner: remote::Owner,
) -> Result<Launch> {
    let role = launch.role;
    let app_id = launch.app_id;
    let id = launch.id.clone();
    // Stopping streams can still own displays shared with this launch.
    // Read both maps under one lock so a worker moving into teardown
    // cannot disappear from the wait before releasing its resources.
    let (canceled, stopping) = {
        let mut sessions = h.sessions.lock().unwrap();
        let mut canceled = sessions.supersede(&launch.client.uuid, role);
        // A Moonlight stream and a WebRTC stream never run together: the newer one ends the older.
        canceled.extend(sessions.supersede_other_transport(role, launch.transport));
        let stopping: Vec<_> = sessions
            .active
            .values()
            .filter(|s| s.stopping())
            .map(|s| s.launch.id.clone())
            .chain(sessions.teardown.keys().cloned())
            .collect();
        (canceled, stopping)
    };
    drop(canceled);
    if !stopping.is_empty() {
        tracing::info!(client = %launch.client.name, "waiting for previous streams to release their resources");
        if !wait_for_teardown(&h.sessions, &stopping, Duration::from_secs(5)) {
            return Err(LaunchFailure(503, "Another stream operation is still running").into());
        }
    }
    // The state machine follows stream launches; a failure before the
    // reply returns it to what it was.
    let mut session =
        (role == Role::Stream).then(|| h.session_begin(&launch.client.uuid, app.as_ref()));
    replace_launch_app(h, &launch, resume, owner)?;
    // Preparing can outlast the pending launch's 30 s (prep commands may
    // run for minutes); it expires only from the reply on, on every path.
    struct Preparing(std::sync::Arc<std::sync::atomic::AtomicBool>);
    impl Drop for Preparing {
        fn drop(&mut self) {
            self.0.store(false, std::sync::atomic::Ordering::Release);
        }
    }
    launch
        .preparing
        .store(true, std::sync::atomic::Ordering::Release);
    let preparing = Preparing(launch.preparing.clone());
    let canceled = h.sessions.lock().unwrap().queue(launch.clone())?;
    drop(canceled);
    if role != Role::InputOnly {
        #[cfg(test)]
        if let Some(fixture) = &h.reconnect_fixture {
            fixture.prepare(&launch);
        }
        if launch.preparation.lock().unwrap().is_none() {
            let prepared = prepare_launch_display(h, args, &launch);
            match prepared {
                Ok(prepared) => *launch.preparation.lock().unwrap() = Some(Box::new(prepared)),
                Err(error) => {
                    let canceled = h.sessions.lock().unwrap().pending.remove(&id);
                    drop(canceled);
                    return Err(error);
                }
            }
        }
    }
    if let Some(value) = args.get("rlControllers")
        && let Err(error) = crate::controller_sessions::prepare(h, &launch, value)
    {
        let canceled = h.sessions.lock().unwrap().pending.remove(&id);
        drop(canceled);
        return Err(error);
    }
    if let Some(session) = &session {
        session.app_starting();
    }
    let prepared_app = match prepare_launch_app(h, args, app.as_ref(), &launch) {
        Ok(prepared_app) => prepared_app,
        Err(error) => {
            // Any failure here, including a config or app lookup error, must
            // release the pending slot now: the client got an error and will
            // never connect, so waiting for expiry would count it for 30 s.
            let canceled = h.sessions.lock().unwrap().pending.remove(&id);
            drop(canceled);
            return Err(error);
        }
    };
    if let Err(error) = check_streaming_session(
        h,
        if resume {
            "resume_prepared"
        } else {
            "launch_prepared"
        },
    ) {
        let canceled = h.sessions.lock().unwrap().pending.remove(&id);
        // A newly started app still belongs to this attempt. Its RAII
        // teardown stops it and undoes prep commands, outside host locks.
        // An existing/resumed app has never been moved out of current_app.
        drop(prepared_app);
        drop(canceled);
        return Err(error);
    }
    // The client has 30 s to connect from now on.
    match h.sessions.lock().unwrap().pending.get_mut(&id) {
        Some(pending) => pending.created = Instant::now(),
        None => bail!("the launch was replaced by a newer one from this device"),
    }
    retain_launch_app(h, &launch, prepared_app);
    drop(preparing);
    let mode = args.get("mode").map_or("", String::as_str);
    let hdr = args.get("hdrMode").is_some_and(|v| v == "1");
    if launch.transport == Transport::WebRtc {
        tracing::info!(client = %launch.client.name, app_id, role = ?role, mode, hdr, resume, "WebRTC session launched");
    } else {
        tracing::info!(client = %launch.client.name, app_id, role = ?role, mode, hdr, resume, "Moonlight session launched");
    }
    // Everything the client asked for, without the stream's AES key.
    let parameters = args
        .iter()
        .filter(|(name, _)| !matches!(name.as_str(), "rikey" | "rikeyid"))
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("&");
    tracing::info!(client = %launch.client.name, resume, ?parameters, "Moonlight launch parameters");
    if let Some(session) = &mut session {
        session.succeed();
    }
    if role == Role::Stream {
        // Off the launch path: the profile's settings are written on a thread of their own.
        crate::radeon_profile::launched(
            h,
            &launch.client.uuid,
            app.as_ref(),
            args.get("mode")
                .and_then(|mode| crate::radeon_profile::stream_fps(mode)),
        );
    }
    Ok(launch)
}
/// What a WebRTC start-session asks the launch for. The settings are the validated ones.
pub(crate) struct WebRtcLaunch {
    pub client: Client,
    pub peer: std::net::IpAddr,
    pub app_id: u32,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub hdr: bool,
}
/// Why a WebRTC launch did not happen.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum WebRtcLaunchError {
    InvalidApplication,
    /// Another app, or another device's WebRTC stream, holds the host.
    SessionActive(&'static str),
    /// A stream operation of another session has not finished.
    HostBusy,
    Failed(String),
}
/// The launch of a WebRTC session: the same preparation as a Moonlight launch, for the device
/// that signalled start-session. The returned launch is queued; `Sessions::start` takes it over
/// once the peer connection is up. Waits for displays and apps, so never on the async runtime.
pub(crate) fn launch_webrtc(
    h: &Shared,
    request: WebRtcLaunch,
) -> std::result::Result<Launch, WebRtcLaunchError> {
    let _transition = h.launch_transition.lock().unwrap();
    let outcome = (|| -> Result<Launch> {
        check_streaming_session(h, "launch_request")?;
        h.request_codec_probe();
        if crate::updater::installing(h) {
            return Err(LaunchFailure(
                503,
                "Rubylight is installing an update. Reconnect shortly.",
            )
            .into());
        }
        let args = Args::from([
            ("appid".to_owned(), request.app_id.to_string()),
            (
                "mode".to_owned(),
                format!("{}x{}x{}", request.width, request.height, request.fps),
            ),
            (
                "hdrMode".to_owned(),
                if request.hdr { "1" } else { "0" }.to_owned(),
            ),
        ]);
        let owner = remote_owner(h, &request.client.uuid);
        let (role, app, app_id) = resolve_launch_app(
            h,
            &args,
            &request.client,
            request.app_id,
            None,
            owner,
            false,
        )
        .map_err(launch_app_error)?;
        if h.current_app
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|running| running.id != app_id)
        {
            return Err(LaunchFailure(
                409,
                "Another app is running; switch to this one or stop the host session first",
            )
            .into());
        }
        if h.sessions.lock().unwrap().held_by_other(
            Role::Stream,
            Transport::WebRtc,
            &request.client.uuid,
        ) {
            return Err(LaunchFailure(409, "Another device is streaming over WebRTC").into());
        }
        h.wait_for_startup_codecs_blocking(STARTUP_CODEC_WAIT);
        if h.codecs.load(std::sync::atomic::Ordering::Acquire) & 0x0783_0301 == 0 {
            return Err(LaunchFailure(503, butterpollo_core::amd_gpu::NO_ENCODER_LAUNCH).into());
        }
        // The stream key belongs to GameStream's RTSP; nothing on this launch uses it.
        let mut launch = build_launch(
            request.peer,
            &args,
            request.client,
            app_id,
            role,
            crypto::random::<16>(),
            rand::random(),
        );
        launch.transport = Transport::WebRtc;
        launch.background = Some(validate_background_launch(h, &launch)?);
        launch_back_half(h, &args, launch, app, false, owner)
    })();
    outcome.map_err(|error| match error.downcast_ref::<LaunchFailure>() {
        Some(LaunchFailure(404, _)) => WebRtcLaunchError::InvalidApplication,
        Some(LaunchFailure(409, message)) => WebRtcLaunchError::SessionActive(message),
        Some(LaunchFailure(503, message))
            if *message != butterpollo_core::amd_gpu::NO_ENCODER_LAUNCH =>
        {
            WebRtcLaunchError::HostBusy
        }
        _ => WebRtcLaunchError::Failed(format!("{error:#}")),
    })
}
/// A failed app lookup keeps its own status: only an unknown app is a 404.
fn launch_app_error(error: anyhow::Error) -> anyhow::Error {
    if error.downcast_ref::<LaunchFailure>().is_none() && error.to_string() == APPLICATION_NOT_FOUND
    {
        LaunchFailure(404, APPLICATION_NOT_FOUND).into()
    } else {
        error
    }
}
/// Whether `client` may not terminate the running app: remote sessions are active and the client
/// does not own the game. The `cancel` request answers this with a 403.
pub(crate) fn terminate_refused(h: &Shared, client: &str) -> bool {
    let Some(game) = remote_game(h) else {
        return false;
    };
    let remote_active = !h.monitors.lock().unwrap().is_empty()
        || h.sessions
            .lock()
            .unwrap()
            .active
            .values()
            .any(|s| !s.stopping() && s.launch.role != Role::Stream);
    remote_active && game.owner != client
}
/// What `stop_streams` did.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum StopStreams {
    Stopped,
    /// The client may not terminate the app (see `terminate_refused`); nothing was stopped.
    Refused,
    /// A stream worker outlived the wait.
    TimedOut,
}
/// Stop every stream, and the running app with them when `terminate_app` names the client asking,
/// as the `cancel` request does, for WebRTC's stop-host-session and switch-session. Remote Monitor
/// and Remote Input sessions stay, and are not waited for.
pub(crate) fn stop_streams(h: &Shared, terminate_app: Option<&str>) -> StopStreams {
    let _transition = h.launch_transition.lock().unwrap();
    if terminate_app.is_some_and(|client| terminate_refused(h, client)) {
        return StopStreams::Refused;
    }
    let (canceled, ids) = {
        let mut sessions = h.sessions.lock().unwrap();
        // The streams `stop_role` ends, and the teardowns already under way.
        let ids: Vec<String> = sessions
            .active
            .values()
            .filter(|s| s.launch.role == Role::Stream)
            .map(|s| s.launch.id.clone())
            .chain(sessions.teardown.keys().cloned())
            .collect();
        (sessions.stop_role(Role::Stream, None), ids)
    };
    drop(canceled);
    if let Some(client) = terminate_app {
        h.stop_app(AppStop::WebRtc(client));
    }
    if wait_for_teardown(&h.sessions, &ids, Duration::from_secs(10)) {
        StopStreams::Stopped
    } else {
        StopStreams::TimedOut
    }
}
fn validate_launch_client(
    h: &Shared,
    connection: &Connection,
    control: Option<Control>,
    resume: bool,
) -> Result<Client> {
    let permission = match control {
        Some(Control::Terminate) => 1 << 26,
        Some(Control::Resume | Control::RunningGame) => 1 << 25,
        _ if resume => 1 << 25,
        _ => 1 << 26,
    };
    if permission == 1 << 25 {
        authenticated_viewer(h, connection)
    } else {
        authenticated(h, connection, permission)
    }
}
fn validate_launch_request(
    h: &Shared,
    client: &Client,
    control: Option<Control>,
) -> ControlFlow<Response, remote::Owner> {
    if crate::updater::installing(h) {
        return ControlFlow::Break(xml(
            503,
            &[],
            Some("Rubylight is installing an update. Reconnect shortly.".into()),
        ));
    }
    match control {
        Some(Control::Terminate) => {
            let Some(game) = remote_game(h) else {
                return ControlFlow::Break(xml(
                    409,
                    &[("gamesession", "0".into())],
                    Some("No application is running".into()),
                ));
            };
            let guard = !h
                .config
                .read()
                .unwrap()
                .boolean("remote_monitor_terminate_on_first_request", false)
                && game.owner != client.uuid;
            let mut confirmations = h.confirmations.lock().unwrap();
            if guard
                && !confirmations.confirm(
                    &client.uuid,
                    remote::Confirmation::Terminate,
                    &game.generation,
                    game.app.id,
                    Instant::now(),
                )
            {
                return ControlFlow::Break(xml(410, &[("resume", "0".into()), ("gamesession", "0".into())], Some("This will close the active stream but leave Remote Monitor and Remote Input connected. Launch Terminate again within 60 seconds to confirm this was intentional.".into())));
            }
            confirmations.clear(&client.uuid, remote::Confirmation::Terminate);
            drop(confirmations);
            let canceled = h.sessions.lock().unwrap().stop_role(Role::Stream, None);
            drop(canceled);
            h.stop_app(AppStop::Terminate(&client.uuid));
            return ControlFlow::Break(xml(
                410,
                &[("gamesession", "0".into())],
                Some("Application terminated".into()),
            ));
        }
        Some(Control::DisconnectMonitor) => {
            crate::remote_display::disconnect(h, Some(&client.uuid));
            return ControlFlow::Break(xml(
                410,
                &[("gamesession", "0".into())],
                Some("Remote monitor disconnected".into()),
            ));
        }
        Some(Control::DisconnectInput) => {
            let canceled = h
                .sessions
                .lock()
                .unwrap()
                .stop_role(Role::InputOnly, Some(&client.uuid));
            drop(canceled);
            return ControlFlow::Break(xml(
                410,
                &[("gamesession", "0".into())],
                Some("Remote input disconnected".into()),
            ));
        }
        _ => {}
    }
    let owner = remote_owner(h, &client.uuid);
    if (control == Some(Control::Input) && owner != remote::Owner::None)
        || (control == Some(Control::Monitor) && owner == remote::Owner::Input)
    {
        return ControlFlow::Break(xml(
            409,
            &[("gamesession", "0".into())],
            Some("Remote session action conflicts with this client's current session state".into()),
        ));
    }
    ControlFlow::Continue(owner)
}
fn resolve_launch_app(
    h: &Shared,
    args: &Args,
    client: &Client,
    requested: u32,
    control: Option<Control>,
    owner: remote::Owner,
    resume: bool,
) -> Result<(Role, Option<App>, u32)> {
    let mut role = match control {
        Some(Control::Monitor) => Role::RemoteMonitor,
        Some(Control::Input) => Role::InputOnly,
        _ => Role::Stream,
    };
    if args.get("input_only").is_some_and(|v| v == "1") {
        role = Role::InputOnly;
    }
    if args.get("remote_monitor").is_some_and(|v| v == "1") {
        role = Role::RemoteMonitor;
    }
    if role == Role::InputOnly
        && !h
            .config
            .read()
            .unwrap()
            .boolean("enable_input_only_mode", false)
    {
        bail!("remote input is disabled by the administrator");
    }
    let app_id = if resume {
        let game = h.current_app.lock().unwrap().as_ref().map(|a| a.id);
        if owner == remote::Owner::Monitor {
            role = Role::RemoteMonitor;
            2147483505
        } else if let Some(id) = game {
            id
        } else if h.monitors.lock().unwrap().contains_key(&client.uuid) {
            role = Role::RemoteMonitor;
            2147483505
        } else {
            bail!("no application or remote monitor to resume");
        }
    } else {
        requested
    };
    let app = h
        .apps
        .read()
        .unwrap()
        .iter()
        .find(|a| {
            a.id() == app_id
                || a.aliases.contains(&app_id)
                || args.get("appuuid").is_some_and(|id| {
                    a.extra.get("uuid").and_then(serde_json::Value::as_str) == Some(id)
                })
        })
        .cloned();
    if role == Role::Stream && app.is_none() {
        bail!(APPLICATION_NOT_FOUND);
    }
    let app_id = app.as_ref().map_or(app_id, |a| a.id());
    Ok((role, app, app_id))
}
fn build_launch_session(
    connection: &Connection,
    args: &Args,
    client: Client,
    app_id: u32,
    role: Role,
    key: [u8; 16],
    key_id: u32,
) -> Launch {
    build_launch(
        connection.peer.ip(),
        args,
        client,
        app_id,
        role,
        key,
        key_id,
    )
}
fn build_launch(
    peer: std::net::IpAddr,
    args: &Args,
    client: Client,
    app_id: u32,
    role: Role,
    key: [u8; 16],
    key_id: u32,
) -> Launch {
    Launch {
        id: uuid::Uuid::new_v4().to_string(),
        client,
        peer,
        app_id,
        background: None,
        key,
        key_id,
        ping: hex::encode(crypto::random::<8>()),
        connect_data: rand::random(),
        role,
        created: Instant::now(),
        rtsp_encrypted: args
            .get("corever")
            .is_some_and(|v| v.parse::<u32>().unwrap_or(0) >= 1),
        rtsp_counter: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(1)),
        rtsp_received: Default::default(),
        preparation: Default::default(),
        vrr_requested: args.get("vrr").is_some_and(|s| s == "1")
            || args.get("client_vrr").is_some_and(|s| s == "1")
            || args.get("clientVrrRequested").is_some_and(|s| s == "1"),
        host_audio: args.get("localAudioPlayMode").is_some_and(|s| s == "1"),
        requested_rate: args
            .get("mode")
            .and_then(|mode| mode.rsplit('x').next())
            .and_then(|rate| {
                if rate.contains('.') {
                    butterpollo_core::framegen::Rate::parse(rate).ok()
                } else {
                    rate.parse()
                        .ok()
                        .map(butterpollo_core::framegen::Rate::from_client)
                }
            })
            .map_or(0, |rate| rate.0),
        audio_preparation: Default::default(),
        options: args.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        preparing: Default::default(),
        transport: Default::default(),
        roaming_offered: Default::default(),
        claimed_until: None,
        warnings: Default::default(),
    }
}
fn replace_launch_app(
    h: &Shared,
    launch: &Launch,
    resume: bool,
    owner: remote::Owner,
) -> Result<()> {
    let mut current = h.current_app.lock().unwrap();
    if launch.role == Role::Stream
        && !resume
        && current.as_ref().is_some_and(|a| a.id != launch.app_id)
    {
        if owner != remote::Owner::None {
            return Err(LaunchFailure(409, "Remote Input or Remote Monitor is active; launch Terminate before starting a different app").into());
        }
        let generation = &current.as_ref().unwrap().generation;
        if h.config
            .read()
            .unwrap()
            .boolean("remote_monitor_confirm_app_replacement", true)
            && !h.confirmations.lock().unwrap().confirm(
                &launch.client.uuid,
                remote::Confirmation::Replace,
                generation,
                launch.app_id,
                Instant::now(),
            )
        {
            return Err(LaunchFailure(410, "An app is already running. Launch this app again within 60 seconds to confirm that you want to close it.").into());
        }
        h.confirmations
            .lock()
            .unwrap()
            .clear(&launch.client.uuid, remote::Confirmation::Replace);
        let (canceled, stopped) = {
            let mut sessions = h.sessions.lock().unwrap();
            let canceled = sessions.stop_role(Role::Stream, None);
            let stopped: Vec<_> = sessions
                .active
                .values()
                .filter(|s| s.launch.role == Role::Stream)
                .map(|s| s.launch.id.clone())
                .chain(sessions.teardown.keys().cloned())
                .collect();
            (canceled, stopped)
        };
        let previous = current.take();
        drop(current);
        drop(canceled);
        // Stopping the app waits for it to exit, never under the lock
        // that every serverinfo and app list request takes.
        drop(previous);
        let audio = h.app_audio.lock().unwrap().take();
        let displays = std::mem::take(&mut *h.app_display.lock().unwrap());
        drop((audio, displays));
        if !wait_for_teardown(&h.sessions, &stopped, Duration::from_secs(10)) {
            return Err(LaunchFailure(503, "Another stream operation is still running").into());
        }
        current = h.current_app.lock().unwrap();
    }
    // Preparing displays, audio and the app's own commands can take many
    // seconds; serverinfo and the app list read current_app meanwhile.
    // Launches and stops stay serialized by launch_transition.
    drop(current);
    Ok(())
}
fn validate_background_launch(h: &Shared, launch: &Launch) -> Result<bool> {
    if launch.role != Role::Stream {
        return Ok(false);
    }
    let background =
        butterpollo_core::background::enabled(&crate::stream::effective_config(h, launch)?);
    let current = h.current_app.lock().unwrap();
    if let Some(app) = current.as_ref().filter(|app| app.id == launch.app_id) {
        if app.background != background {
            return Err(LaunchFailure(
                409,
                "End the current game before changing background streaming mode",
            )
            .into());
        }
        if background && app.owner != launch.client.uuid {
            return Err(LaunchFailure(409, "This background game belongs to another device; end it before starting a background game here").into());
        }
    }
    Ok(background)
}
fn prepare_launch_display(
    h: &Shared,
    args: &Args,
    launch: &Launch,
) -> Result<crate::display_session::StreamPreparation> {
    let config = crate::stream::effective_config(h, launch)?;
    let mode = args
        .get("mode")
        .map(String::as_str)
        .unwrap_or(config.get("fallback_mode", "1920x1080x60"));
    let dimensions: Vec<_> = mode.split('x').collect();
    if dimensions.len() != 3 {
        bail!("launch mode must be WIDTHxHEIGHTxFPS");
    }
    let rate = if dimensions[2].contains('.') {
        butterpollo_core::framegen::Rate::parse(dimensions[2])?
    } else {
        butterpollo_core::framegen::Rate::from_client(dimensions[2].parse()?)
    };
    let mut stream = butterpollo_core::rtsp::Negotiated {
        // Encoders need even sizes. Moonlight rounds only the
        // height, and only later; a 2556x1179 phone was refused.
        width: dimensions[0]
            .parse::<u32>()
            .context("invalid launch width")?
            & !1,
        height: dimensions[1]
            .parse::<u32>()
            .context("invalid launch height")?
            & !1,
        fps: rate.rounded(),
        rate_millihz: rate.0,
        hdr: args.get("hdrMode").is_some_and(|v| v == "1"),
        codec: 1,
        vrr_low_latency: launch.vrr_requested,
        ..Default::default()
    };
    butterpollo_core::stream_policy::apply_color(&mut stream, &config);
    stream.validate()?;
    if launch.role == Role::Stream {
        // Reuse only this client's own retained display; another
        // client streaming the same app keeps its display. One set up for
        // another display choice or layout is replaced.
        let client_display = crate::display_session::client_display(launch);
        let app = h
            .apps
            .read()
            .unwrap()
            .iter()
            .find(|app| app.id() == launch.app_id || app.aliases.contains(&launch.app_id))
            .cloned();
        let layout = crate::display_session::virtual_display_layout(
            &config,
            &launch.client,
            app.as_ref(),
            client_display,
        );
        let retained = crate::state::take_retained(
            &mut h.app_display.lock().unwrap(),
            &launch.client.uuid,
            |lease| lease.matches(&stream) && lease.serves(client_display, layout),
        );
        match retained {
            Ok(lease) => return lease.resume(&h.directory, &config, launch.warnings.clone()),
            // Dropped here, after the lock: releasing restores Windows.
            Err(released) => drop(released),
        }
    }
    crate::display_session::prepare_stream(h, launch, &stream, &config)
}
fn prepare_launch_app(
    h: &Shared,
    args: &Args,
    app: Option<&App>,
    launch: &Launch,
) -> Result<Option<crate::process::RunningApp>> {
    #[cfg(test)]
    if let Some(fixture) = &h.reconnect_fixture {
        return fixture.prepare_app(h, app, launch);
    }
    let mut background = false;
    if launch.role != Role::InputOnly {
        let config = crate::stream::effective_config(h, launch)?;
        background = butterpollo_core::background::enabled(&config);
        if config.boolean("stream_audio", true)
            && config.get("audio_capture_mode", "system") == "system"
            && !(launch.role == Role::RemoteMonitor
                && config.boolean("remote_monitor_mute_audio", false))
        {
            let channels = args
                .get("surroundAudioInfo")
                .and_then(|s| s.parse::<u32>().ok())
                .map_or(2, |v| v & 0xffff) as usize;
            // 12 is 7.1.4; ANNOUNCE refuses it later unless it is offered.
            let channels = if matches!(channels, 2 | 6 | 8 | 12) {
                channels
            } else {
                2
            };
            match butterpollo_windows::audio_route::Route::acquire(
                &config,
                &h.directory,
                launch.host_audio,
                channels,
            ) {
                Ok(route) => {
                    *launch.audio_preparation.lock().unwrap() =
                        Some(Box::new(std::sync::Arc::new(route)))
                }
                Err(error) => {
                    tracing::warn!(%error, "audio route will be retried after stream negotiation")
                }
            }
        }
    }
    // Prep commands can run for minutes: the app starts without the lock
    // that every serverinfo, app list and asset request takes. Launches
    // and stops stay serialized by launch_transition, so no other app is
    // installed meanwhile.
    if launch.role == Role::Stream && h.current_app.lock().unwrap().is_none() {
        let mut process_args = args.clone();
        process_args.insert("clientName".into(), launch.client.name.clone());
        process_args.insert("clientUuid".into(), launch.client.uuid.clone());
        // The host decides this, never the client's launch query.
        process_args.remove(crate::process::BACKGROUND_ARG);
        if background {
            process_args.insert(crate::process::BACKGROUND_ARG.into(), "1".into());
        }
        return crate::process::launch(h, app.context("application not found")?, &process_args)
            .map(Some);
    }
    Ok(None)
}
fn retain_launch_app(
    h: &Shared,
    launch: &Launch,
    prepared_app: Option<crate::process::RunningApp>,
) {
    if let Some(running) = prepared_app {
        let mut current = h.current_app.lock().unwrap();
        if current.is_none() {
            let started = running.started_event();
            *current = Some(running);
            drop(current);
            h.app_started_webhook(started);
        } else {
            drop(current);
            tracing::warn!("another application started meanwhile; this launch's app is stopped");
            drop(running);
        }
    }
    #[cfg(test)]
    if let Some(fixture) = &h.reconnect_fixture {
        fixture.retain_resources();
        return;
    }
    if launch.role == Role::Stream {
        *h.app_audio.lock().unwrap() = launch
            .audio_preparation
            .lock()
            .unwrap()
            .as_ref()
            .map(|route| route.route());
        if let Some(lease) = launch
            .preparation
            .lock()
            .unwrap()
            .as_ref()
            .map(|prepared| prepared.prepared().display.clone())
        {
            h.app_display
                .lock()
                .unwrap()
                .insert(launch.client.uuid.clone(), (lease, None));
        }
    }
}
fn launch_response(
    result: Result<(String, String)>,
    requested: u32,
    reply: &str,
    resume: bool,
    driver_ready: bool,
) -> Response {
    match result {
        Ok((key, url)) => xml(
            200,
            &[
                (key.as_str(), "1".into()),
                ("sessionUrl0", url),
                ("VirtualDisplayDriverReady", driver_ready.to_string()),
            ],
            None,
        ),
        Err(error) => {
            tracing::warn!(
                error = %format!("{error:#}"),
                app_id = requested,
                resume,
                "Moonlight session launch failed"
            );
            xml(
                error
                    .downcast_ref::<LaunchFailure>()
                    .map_or(503, |failure| failure.0),
                &[(reply, "0".into())],
                Some(format!("{error:#}")),
            )
        }
    }
}
async fn cancel(State(h): State<Shared>, Extension(connection): Extension<Connection>) -> Response {
    // Stopping waits for the app to exit and runs its undo commands.
    blocking(move || cancel_app(h, connection)).await
}
fn cancel_app(h: Shared, connection: Connection) -> Response {
    let client = match authenticated(&h, &connection, 1 << 26) {
        Ok(client) => client,
        Err(e) => return xml(401, &[], Some(e.to_string())),
    };
    let _transition = h.launch_transition.lock().unwrap();
    if remote_game(&h).is_none() {
        return xml(200, &[("cancel", "1".into())], None);
    }
    if terminate_refused(&h, &client.uuid) {
        return xml(
            403,
            &[("cancel", "0".into())],
            Some("Only the game owner may cancel while remote sessions are active".into()),
        );
    }
    let canceled = h.sessions.lock().unwrap().stop_role(Role::Stream, None);
    drop(canceled);
    h.stop_app(AppStop::Cancel(&client.uuid));
    xml(200, &[("cancel", "1".into())], None)
}
async fn unpair(State(h): State<Shared>, Extension(connection): Extension<Connection>) -> Response {
    // Only a paired client's own certificate can unpair it; anything else is
    // answered like Vibepollo, with nothing removed.
    let Ok(client) = authenticated(&h, &connection, 0) else {
        return xml(200, &[("unpaired", "0".into())], None);
    };
    let removed = h.unpair_with_network_history(Some(&client.uuid));
    if let Err(e) = removed {
        return xml(500, &[("unpaired", "0".into())], Some(e.to_string()));
    }
    let canceled = h.sessions.lock().unwrap().request_stop(Some(&client.uuid));
    drop(canceled);
    crate::remote_display::disconnect(&h, Some(&client.uuid));
    h.radeon.release_device(&client.uuid);
    xml(200, &[("unpaired", "1".into())], None)
}
async fn appasset(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    Query(args): Query<Args>,
) -> Response {
    if authenticated(&h, &c, 1 << 24).is_err() {
        return xml(401, &[], None);
    }
    let app = args
        .get("appid")
        .and_then(|id| id.parse::<u32>().ok())
        .and_then(|id| {
            let id = remote_game(&h)
                .filter(|game| id == butterpollo_core::remote::running_game_id(game.app.id))
                .map_or(id, |game| game.app.id);
            h.apps
                .read()
                .unwrap()
                .iter()
                .find(|a| a.id() == id || a.aliases.contains(&id))
                .cloned()
        });
    let assets = h.assets.parent().unwrap_or(&h.assets);
    let control_art = args
        .get("appid")
        .and_then(|id| id.parse().ok())
        .and_then(|id| butterpollo_core::remote::identify(id, ""))
        .and_then(|control| match control {
            butterpollo_core::remote::Control::Resume => Some("resume.png"),
            butterpollo_core::remote::Control::DisconnectMonitor => {
                Some("disconnect-remote-monitor.png")
            }
            butterpollo_core::remote::Control::DisconnectInput => {
                Some("disconnect-remote-input.png")
            }
            butterpollo_core::remote::Control::Terminate => Some("terminate.png"),
            butterpollo_core::remote::Control::Monitor => Some("remote-monitor.png"),
            butterpollo_core::remote::Control::Input => Some("remote-input.png"),
            _ => None,
        });
    let path = control_art
        .map(|name| assets.join("remote-session").join(name))
        .or_else(|| app.map(|a| butterpollo_core::catalog::artwork(&a, assets)))
        .unwrap_or_else(|| assets.join("box.png"));
    if !tokio::fs::metadata(&path)
        .await
        .is_ok_and(|m| m.len() <= 16 * 1024 * 1024)
    {
        return StatusCode::NOT_FOUND.into_response();
    }
    match tokio::fs::read(&path).await {
        Ok(bytes) if bytes.len() <= 16 * 1024 * 1024 => {
            let mime = if bytes.starts_with(b"\x89PNG") {
                "image/png"
            } else if bytes.starts_with(b"\xff\xd8") {
                "image/jpeg"
            } else {
                return StatusCode::UNSUPPORTED_MEDIA_TYPE.into_response();
            };
            ([(header::CONTENT_TYPE, mime)], bytes).into_response()
        }
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}
/// What `/actions/clipboard` carries: `type=text`, or `type=png` while the
/// host's `clipboard_image` setting is on (`docs/native-api.md`).
#[derive(Clone, Copy, Debug, PartialEq)]
enum ClipboardType {
    Text,
    Png,
}
fn clipboard_type(args: &Args) -> Option<ClipboardType> {
    match args.get("type").map(String::as_str) {
        Some("text") => Some(ClipboardType::Text),
        Some("png") => Some(ClipboardType::Png),
        _ => None,
    }
}
/// The longest body a device may send for `kind`: the 1 MiB of the other
/// routes for text, `clipboard_image_max_mb` for an image.
fn clipboard_limit(h: &Shared, kind: Option<ClipboardType>) -> usize {
    match kind {
        Some(ClipboardType::Png) => {
            let max = h.config.read().unwrap().clipboard_image_max_bytes();
            usize::try_from(max).unwrap_or(usize::MAX)
        }
        _ => 1024 * 1024,
    }
}
fn clipboard_client(
    h: &Shared,
    c: &Connection,
    permission: u32,
    args: &Args,
) -> Result<ClipboardType> {
    let client = authenticated(h, c, permission)?;
    if client.perm & ((1 << 25) | (1 << 26)) == 0 {
        bail!("view permission required");
    }
    let kind = match clipboard_type(args) {
        Some(ClipboardType::Png) if !h.config.read().unwrap().clipboard_image() => {
            bail!("the image clipboard is off on this host")
        }
        Some(kind) => kind,
        None => bail!("only text and PNG clipboard data are supported"),
    };
    let sessions: Vec<_> = h
        .sessions
        .lock()
        .unwrap()
        .active
        .values()
        .filter(|s| s.launch.client.uuid == client.uuid && !s.stopping())
        .cloned()
        .collect();
    if sessions.is_empty() {
        bail!("clipboard access requires an active session");
    }
    // The clipboard belongs to the person using the PC, not to a device
    // playing in the background.
    if sessions.iter().any(|s| {
        crate::stream::effective_config(h, &s.launch).map_or(true, |config| {
            butterpollo_core::background::enabled(&config)
        })
    }) {
        bail!("clipboard access is off for background streams");
    }
    Ok(kind)
}
async fn clipboard_read(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    Query(args): Query<Args>,
) -> Response {
    let Ok(kind) = clipboard_client(&h, &c, 1 << 17, &args) else {
        return StatusCode::FORBIDDEN.into_response();
    };
    if kind == ClipboardType::Png {
        let max = clipboard_limit(&h, Some(kind));
        return match tokio::task::spawn_blocking(move || clipboard::read_png(max)).await {
            Ok(Ok(Some(png))) => ([(header::CONTENT_TYPE, "image/png")], png).into_response(),
            Ok(Ok(None)) => StatusCode::NOT_FOUND.into_response(),
            // The device did nothing wrong: an image this host can't
            // convert is 415 whatever is wrong with it.
            Ok(Err(error)) => {
                clipboard_image_failure("reading", &error, StatusCode::UNSUPPORTED_MEDIA_TYPE)
            }
            Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        };
    }
    match tokio::task::spawn_blocking(clipboard::read).await {
        Ok(Ok(text)) => {
            ([(header::CONTENT_TYPE, "text/plain; charset=utf-8")], text).into_response()
        }
        _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
/// The body takes no more memory than the type's limit: a longer one is 413
/// before it is read, if it says so, and once it passes the limit if not.
/// An image's dimensions are then checked before its pixels are decoded
/// (`butterpollo_core::clipboard_image`).
async fn clipboard_write(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    Query(args): Query<Args>,
    request: axum::extract::Request,
) -> Response {
    // Refused before anything else, as the 1 MiB request limit was.
    let limit = clipboard_limit(&h, clipboard_type(&args));
    let declared = request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok()?.trim().parse::<u64>().ok());
    if declared.is_some_and(|length| length > limit as u64) {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    }
    let Ok(kind) = clipboard_client(&h, &c, 1 << 16, &args) else {
        return StatusCode::FORBIDDEN.into_response();
    };
    if kind == ClipboardType::Png && !is_png_content_type(request.headers()) {
        return StatusCode::UNSUPPORTED_MEDIA_TYPE.into_response();
    }
    let body = match body_within(request.into_body(), limit).await {
        Ok(body) => body,
        Err(status) => return status.into_response(),
    };
    if kind == ClipboardType::Png {
        return match tokio::task::spawn_blocking(move || clipboard::write_png(&body)).await {
            Ok(Ok(())) => StatusCode::OK.into_response(),
            Ok(Err(error)) => clipboard_image_failure("writing", &error, StatusCode::BAD_REQUEST),
            Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        };
    }
    let Ok(text) = String::from_utf8(body) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    match tokio::task::spawn_blocking(move || clipboard::write(&text)).await {
        Ok(Ok(())) => StatusCode::OK.into_response(),
        _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
fn is_png_content_type(headers: &axum::http::HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("image/png"))
}
/// A request body of at most `limit` bytes: 413 once it passes that, 400
/// when it can't be read.
async fn body_within(mut body: axum::body::Body, limit: usize) -> Result<Vec<u8>, StatusCode> {
    use axum::body::HttpBody;
    let mut bytes = Vec::new();
    while let Some(frame) =
        std::future::poll_fn(|cx| std::pin::Pin::new(&mut body).poll_frame(cx)).await
    {
        let Ok(data) = frame.map_err(|_| StatusCode::BAD_REQUEST)?.into_data() else {
            continue;
        };
        if bytes.len() + data.len() > limit {
            return Err(StatusCode::PAYLOAD_TOO_LARGE);
        }
        bytes.extend_from_slice(&data);
    }
    Ok(bytes)
}
/// 413 for an image over the limit, `malformed` for one that can't be
/// converted, 500 when the clipboard itself failed. Each is logged.
fn clipboard_image_failure(doing: &str, error: &anyhow::Error, malformed: StatusCode) -> Response {
    use butterpollo_core::clipboard_image::ImageError;
    let status = match error.downcast_ref::<ImageError>() {
        Some(ImageError::TooLarge) => StatusCode::PAYLOAD_TOO_LARGE,
        Some(ImageError::Unsupported(_)) => StatusCode::UNSUPPORTED_MEDIA_TYPE,
        Some(ImageError::NotPng | ImageError::Malformed(_)) => malformed,
        None => StatusCode::INTERNAL_SERVER_ERROR,
    };
    tracing::warn!("Clipboard: {doing} an image failed ({status}): {error:#}");
    status.into_response()
}
async fn bitrate(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    Query(args): Query<Args>,
) -> Response {
    // Vibepollo's reply: the applied bitrate, 0 on failure.
    let failed = |code, message: &str| xml(code, &[("bitrate", "0".into())], Some(message.into()));
    let client = match authenticated_viewer(&h, &c) {
        Ok(client) => client,
        Err(e) => return failed(403, &e.to_string()),
    };
    let Some(requested) = args
        .get("bitrate")
        .or_else(|| args.get("bitrate_kbps"))
        .and_then(|v| v.parse::<u32>().ok())
        .filter(|v| *v > 0)
    else {
        return failed(400, "Missing or invalid bitrate parameter");
    };
    let applied =
        butterpollo_core::stream_policy::runtime_bitrate_kbps(&h.config.read().unwrap(), requested);
    let sessions = h.sessions.lock().unwrap();
    let mut count = 0;
    for s in sessions.active.values() {
        if s.launch.client.uuid == client.uuid {
            butterpollo_core::stream_policy::report_bitrate(
                &s.launch.warnings,
                requested,
                applied,
                "max_bitrate and the 500 Mbps runtime cap",
            );
            s.set_client_bitrate(applied);
            count += 1;
            let minimum = butterpollo_core::pyrowave::minimum_kbps(
                s.config.width,
                s.config.height,
                s.config.fps_millihz(),
            );
            let recommended = butterpollo_core::pyrowave::recommended_kbps(
                s.config.width,
                s.config.height,
                s.config.fps_millihz(),
            );
            if s.config.codec == 3 && applied < minimum {
                tracing::warn!(
                    bitrate_kbps = applied,
                    minimum_kbps = minimum,
                    recommended_kbps = recommended,
                    "PyroWave bitrate is too low: severe detail loss is likely. Raise the bitrate in Moonlight with network headroom, or use HEVC or AV1"
                );
            } else if s.config.codec == 3 && applied < recommended {
                tracing::warn!(
                    bitrate_kbps = applied,
                    minimum_kbps = minimum,
                    recommended_kbps = recommended,
                    "PyroWave bitrate is below recommended: text and textures may lose detail. Quality depends on the picture; raise the bitrate in Moonlight with network headroom, or use HEVC or AV1"
                );
            }
        }
    }
    if count == 0 {
        return failed(404, "No active session for this client");
    }
    tracing::info!(client = %client.name, requested, applied, "client set the stream bitrate");
    xml(
        200,
        &[
            ("bitrate", applied.to_string()),
            ("updated", count.to_string()),
        ],
        None,
    )
}
async fn abr(State(h): State<Shared>, Extension(c): Extension<Connection>) -> Response {
    if authenticated_viewer(&h, &c).is_err() {
        return axum::http::StatusCode::UNAUTHORIZED.into_response();
    }
    // Keep the legacy flag false: it means a client may delegate its complete
    // controller to the host. Our conservative opt-in only starts after real
    // feedback and explicitly yields to every client /bitrate request.
    axum::Json(json!({"supported":false,"version":1,"features":["runtime_bitrate","host_loss_adaptation"],"host_adaptive_bitrate":h.config.read().unwrap().boolean("host_adaptive_bitrate",false),"client_bitrate_takes_ownership":true}))
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::stream_key_id;
    fn running_app(f: &crate::state::test_support::Fixture, owner: &str) {
        let app = f.host.apps.read().unwrap()[0].clone();
        let mut running =
            crate::process::RunningApp::with_environment(&app, Default::default()).unwrap();
        running.owner = owner.into();
        *f.host.current_app.lock().unwrap() = Some(running);
    }
    #[test]
    fn stopping_streams_does_not_wait_for_remote_sessions_it_leaves_running() {
        use super::{Duration, Instant, StopStreams, stop_streams};
        use crate::state::test_support::Fixture;
        use butterpollo_core::session::Role;
        let f = Fixture::new();
        let client = f.client(u32::MAX);
        let monitor = f.session(client.clone(), Role::RemoteMonitor);
        let input = f.session(client, Role::InputOnly);
        for terminate in [None, Some("anyone")] {
            let started = Instant::now();
            assert_eq!(stop_streams(&f.host, terminate), StopStreams::Stopped);
            assert!(started.elapsed() < Duration::from_secs(3));
            assert!(!monitor.stopping() && !input.stopping());
        }
    }
    #[test]
    fn stopping_streams_ends_a_stream_and_waits_for_it_to_leave() {
        use super::{Duration, StopStreams, stop_streams};
        use crate::state::test_support::Fixture;
        use butterpollo_core::session::Role;
        let f = Fixture::new();
        let stream = f.session(f.client(u32::MAX), Role::Stream);
        let host = f.host.clone();
        let id = stream.launch.id.clone();
        // The worker moves out of `active` once it sees the stop.
        let worker = std::thread::spawn({
            let stream = stream.clone();
            move || {
                while !stream.stopping() {
                    std::thread::sleep(Duration::from_millis(2));
                }
                host.sessions.lock().unwrap().active.remove(&id);
            }
        });
        assert_eq!(stop_streams(&f.host, None), StopStreams::Stopped);
        assert!(stream.stopping());
        worker.join().unwrap();
    }
    #[test]
    fn only_the_games_owner_may_terminate_it_while_remote_sessions_are_active() {
        use super::{StopStreams, stop_streams, terminate_refused};
        use crate::state::test_support::Fixture;
        use butterpollo_core::session::Role;
        let f = Fixture::new();
        // No game, nothing to refuse.
        assert!(!terminate_refused(&f.host, "other"));
        running_app(&f, "owner");
        assert!(!terminate_refused(&f.host, "other"));
        let monitor = f.session(f.client(u32::MAX), Role::RemoteMonitor);
        assert!(terminate_refused(&f.host, "other"));
        assert!(!terminate_refused(&f.host, "owner"));
        // A refused stop changes nothing.
        assert_eq!(stop_streams(&f.host, Some("other")), StopStreams::Refused);
        assert!(f.host.current_app.lock().unwrap().is_some());
        assert!(!monitor.stopping());
        // Ending the remote session lifts the guard.
        monitor.stop();
        assert!(!terminate_refused(&f.host, "other"));
        assert_eq!(stop_streams(&f.host, Some("other")), StopStreams::Stopped);
        assert!(f.host.current_app.lock().unwrap().is_none());
    }
    #[test]
    fn a_failed_app_lookup_keeps_its_own_status_and_only_an_unknown_app_is_a_404() {
        use super::{APPLICATION_NOT_FOUND, LaunchFailure, launch_app_error};
        let status = |error: anyhow::Error| {
            error
                .downcast_ref::<LaunchFailure>()
                .map(|failure| failure.0)
        };
        assert_eq!(
            status(launch_app_error(anyhow::anyhow!(APPLICATION_NOT_FOUND))),
            Some(404)
        );
        assert_eq!(
            status(launch_app_error(LaunchFailure(503, "busy").into())),
            Some(503)
        );
        let other = launch_app_error(anyhow::anyhow!("remote input is disabled"));
        assert_eq!(other.to_string(), "remote input is disabled");
        assert!(other.downcast_ref::<LaunchFailure>().is_none());
    }
    #[test]
    fn a_slow_teardown_blocks_the_next_launch_after_the_wait() {
        use super::{Duration, Instant, Mutex, Sessions, wait_for_teardown};
        let sessions = Mutex::new(Sessions::default());
        sessions
            .lock()
            .unwrap()
            .teardown
            .insert("slow".into(), Instant::now() - Duration::from_secs(6));
        assert!(!wait_for_teardown(
            &sessions,
            &["slow".into()],
            Duration::ZERO
        ));
        assert!(sessions.lock().unwrap().owns_capture());
        sessions.lock().unwrap().teardown.remove("slow");
        assert!(wait_for_teardown(
            &sessions,
            &["slow".into()],
            Duration::from_secs(5)
        ));
    }
    #[test]
    fn another_device_cannot_take_over_a_pairing_in_progress() {
        use super::another_device_pairing as blocked;
        // Nothing pending, or this device asking again (after a wrong PIN).
        assert!(!blocked(None, "mine"));
        assert!(!blocked(Some(("mine", true)), "mine"));
        // Another device whose request is still waiting or handshaking.
        assert!(blocked(Some(("theirs", true)), "mine"));
        // Another device that gave up: its stale request is replaced.
        assert!(!blocked(Some(("theirs", false)), "mine"));
    }

    #[test]
    fn android_signed_stream_keys_preserve_the_wire_iv() {
        for (signed, unsigned) in [
            ("0", "0"),
            ("2147483647", "2147483647"),
            ("-2147483648", "2147483648"),
            ("-1", "4294967295"),
        ] {
            assert_eq!(
                stream_key_id(signed).unwrap().to_be_bytes(),
                stream_key_id(unsigned).unwrap().to_be_bytes()
            );
        }
        for invalid in ["", "-2147483649", "4294967296", "1.5", "0x123", "key"] {
            assert!(stream_key_id(invalid).is_err(), "{invalid}");
        }
    }
}
