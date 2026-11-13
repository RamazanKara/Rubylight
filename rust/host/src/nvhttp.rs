use crate::{
    state::{AppStop, Launch, PendingPin, Sessions, Shared},
    tls::Connection,
};
use anyhow::{Context, Result, bail, ensure};
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
    config::Config,
    crypto,
    pairing::Pairing,
    remote::{self, Control},
    session::{Role, Transport},
    state::{App, Client, device_uuid},
};
#[cfg(not(test))]
use butterpollo_windows::clipboard;
#[cfg(test)]
use parity_tests::clipboard;
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    ops::ControlFlow,
    sync::Mutex,
    time::{Duration, Instant},
};

mod clipboard_route;
mod launch;
mod launch_steps;
mod pairing;
mod probe;
mod quit;
mod serverinfo;

use clipboard_route::*;
pub(crate) use launch::*;
pub(crate) use launch_steps::*;
use pairing::*;
use probe::*;
pub(crate) use quit::*;
pub use serverinfo::*;

type Args = HashMap<String, String>;
/// How long discovery and launches wait for the startup encoder probe. Its
/// standard codecs took 0.6–1.3 s to publish on test PCs; five seconds also
/// cover a driver still starting with Windows. A probe stuck longer keeps
/// each request for no more than that, where 2.2.1 waited ten seconds.
const STARTUP_CODEC_WAIT: Duration = Duration::from_secs(5);
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
        // Microphone only holds no remote input: the device's game stays its own.
        .any(|s| s.client.uuid == client && s.role == Role::InputOnly && !s.mic_only())
        || sessions.active.values().any(|s| {
            !s.stopping()
                && s.launch.client.uuid == client
                && s.launch.role == Role::InputOnly
                && !s.launch.mic_only()
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
    Ok(as_requested(c.clone(), connection))
}
/// The paired device as this request's sender: its second screen when the request says so
/// ([`Connection::second_screen`]), so a request about a stream reaches that screen's stream.
/// The device's own pairing, permissions and settings are the same either way.
pub(crate) fn as_requested(client: Client, connection: &Connection) -> Client {
    if connection.second_screen {
        client.second_screen()
    } else {
        client
    }
}
/// Request parameter of Nightjar's Dual screen mode: `1` on every HTTPS request of a device's
/// second screen, its `/launch` and `/resume` included.
const SECOND_SCREEN: &str = "rlSecondScreen";
fn second_screen_query(query: Option<&str>) -> bool {
    query.is_some_and(|query| {
        url::form_urlencoded::parse(query.as_bytes())
            .any(|(key, value)| key == SECOND_SCREEN && value == "1")
    })
}
/// Marks the request of a device's second screen on its [`Connection`], before any handler
/// resolves the sender.
async fn mark_second_screen(mut request: axum::extract::Request) -> axum::extract::Request {
    let second_screen = second_screen_query(request.uri().query());
    if let Some(connection) = request.extensions_mut().get_mut::<Connection>() {
        connection.second_screen = second_screen;
    }
    request
}
/// Ends every session of a paired device, its second screen's too: an unpaired or turned off
/// device keeps neither streaming.
pub(crate) fn end_device(h: &Shared, device: &str) {
    let device = butterpollo_core::state::device_uuid(device);
    for uuid in [
        device.to_owned(),
        butterpollo_core::state::second_screen_uuid(device),
    ] {
        let canceled = h.sessions.lock().unwrap().request_stop(Some(&uuid));
        drop(canceled);
        crate::remote_display::disconnect(h, Some(&uuid));
    }
}
/// Releases what an unpaired or turned off device held, Radeon settings and game files, for
/// both of its screens: each screen's stream claims them under its own identity.
pub(crate) fn release_device(h: &Shared, device: &str) {
    let device = butterpollo_core::state::device_uuid(device);
    for uuid in [
        device.to_owned(),
        butterpollo_core::state::second_screen_uuid(device),
    ] {
        h.radeon.release_device(&uuid);
        h.game_files.release_device(&h.directory, &uuid);
    }
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
            .route(crate::internet_play::ROUTE, get(internet_play))
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
    r.layer(axum::middleware::map_request(mark_second_screen))
        .layer(axum::extract::DefaultBodyLimit::max(1024 * 1024))
        .with_state(h)
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
    // ApolloSync or MoonDeckBuddy may have changed apps.json since the last list.
    h.reload_apps();
    let apps = h.apps.read().unwrap().clone();
    let config = h.config.read().unwrap().clone();
    // The co-op entries follow the user's apps; reading Nucleus Co-op's
    // handlers and games touches files.
    let coop = {
        let (h, apps) = (h.clone(), apps.clone());
        let configured = crate::nucleus::configured(&config);
        match tokio::task::spawn_blocking(move || h.nucleus.entries(&apps, &configured)).await {
            Ok(coop) => coop,
            // The user's apps still list; only the co-op entries are missing.
            Err(error) => {
                tracing::error!(%error, "Nucleus Co-op: the co-op entries did not load");
                Vec::new()
            }
        }
    };
    let configured = apps
        .iter()
        .map(|app| remote::Entry {
            id: app.id(),
            title: app.name.clone(),
            index: 0,
            uuid: app
                .extra
                .get("uuid")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .into(),
            art_version: app.id().to_string(),
        })
        .chain(coop.iter().map(|coop| remote::Entry {
            id: coop.id(),
            title: coop.entry.name.clone(),
            index: 0,
            uuid: String::new(),
            art_version: coop.id().to_string(),
        }))
        .enumerate()
        .map(|(index, entry)| remote::Entry { index, ..entry })
        .collect();
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
mod tests;
