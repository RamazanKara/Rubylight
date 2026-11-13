//! `/serverinfo`: what the host says of itself to a paired and to an unpaired device, and the facts it caches for that.

use super::*;

pub(super) type ServerInfoRoute = Option<(std::net::IpAddr, std::net::SocketAddr)>;
pub(super) type ServerInfoFacts = (String, u64, bool);

#[derive(Default)]
pub struct ServerInfoCache {
    pub(super) entries: HashMap<ServerInfoRoute, (Instant, ServerInfoFacts)>,
}
impl ServerInfoCache {
    pub(super) fn get(&mut self, route: ServerInfoRoute, now: Instant) -> Option<ServerInfoFacts> {
        self.entries
            .retain(|_, (at, _)| now.duration_since(*at) < Duration::from_secs(10));
        self.entries.get(&route).map(|(_, facts)| facts.clone())
    }
}

pub(super) async fn network_path(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
) -> Response {
    if authenticated_viewer(&h, &c).is_err() {
        return ([(header::CACHE_CONTROL, "no-store")], StatusCode::FORBIDDEN).into_response();
    }
    let report = crate::network_path::inspect(c.peer.ip().to_canonical()).await;
    if authenticated_viewer(&h, &c).is_err() {
        return ([(header::CACHE_CONTROL, "no-store")], StatusCode::FORBIDDEN).into_response();
    }
    ([(header::CACHE_CONTROL, "no-store")], axum::Json(report)).into_response()
}

pub(super) async fn hardware_metrics(
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
/// `GET /internet-play/v1` (`docs/internet-play.md`): what a paired device keeps to reach this
/// PC from outside its network. Paired HTTPS only: it carries the device's rendezvous secret.
pub(super) async fn internet_play(
    State(h): State<Shared>,
    Extension(connection): Extension<Connection>,
) -> Response {
    let client = match authenticated(&h, &connection, 0) {
        Ok(client) => client,
        Err(error) => {
            return (
                StatusCode::UNAUTHORIZED,
                axum::Json(json!({"error": error.to_string()})),
            )
                .into_response();
        }
    };
    let state = h.clone();
    match tokio::task::spawn_blocking(move || crate::internet_play::describe(&state, &client.cert))
        .await
    {
        Ok(Ok(value)) => ([(header::CACHE_CONTROL, "no-store")], axum::Json(value)).into_response(),
        Ok(Err(error)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(json!({"error": format!("{error:#}")})),
        )
            .into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
pub(super) async fn serverinfo(
    State(h): State<Shared>,
    Extension(connection): Extension<Connection>,
    headers: axum::http::HeaderMap,
) -> Response {
    h.request_codec_probe();
    h.wait_for_startup_codecs(STARTUP_CODEC_WAIT).await;
    let config = h.config_snapshot();
    let client = authenticated(&h, &connection, 0).ok();
    if let Some(client) = &client {
        note_client_agent(
            &h,
            butterpollo_core::state::device_uuid(&client.uuid),
            &headers,
        );
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
pub(super) fn serverinfo_response(
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
    // Artemis lists these commands; a client runs one by its index: the
    // host-wide ones, then the running app's own (`menu_commands`).
    let commands: Vec<String> = if permission & 0x0010_0000 != 0 {
        crate::process::pc_command_names(h, config)
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
    let local = crate::network::from_this_pc(connection.peer);
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
                (device_uuid(&game.owner) == device_uuid(&client.uuid)
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
            u8::from(permission & VIEW != 0 && config.hardware_metrics()).to_string(),
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
        // Why this device's latest stream failed, in the host's words: Moonlight
        // shows only the termination code, Nightjar asks for this after it.
        (
            "RustHostLastFailure",
            client
                .and_then(|client| h.last_failure(&client.uuid))
                .unwrap_or_default(),
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
            if crate::network::from_this_pc(connection.peer) {
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
        // Play over the internet: Moonlight keeps ExternalIP as the PC's remote address, and
        // Nightjar fetches `/internet-play/v1` when the version is set. Paired only: the
        // public address is the owner's.
        let internet_play = crate::internet_play::enabled(config);
        if (internet_play || config.boolean("upnp", false))
            && let Some(external) = crate::internet_play::external_ipv4()
        {
            fields.push(("ExternalIP", external.to_string()));
        }
        fields.push((
            crate::internet_play::SERVERINFO_FIELD,
            if internet_play {
                crate::internet_play::VERSION
            } else {
                0
            }
            .to_string(),
        ));
    }
    // DESCRIBE's `packed444Modes` mask, from the startup probe and the probe
    // after a save of the full chroma settings, so a client
    // can leave out a packed mode before /launch instead of after it.
    if config.packed_444() {
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
