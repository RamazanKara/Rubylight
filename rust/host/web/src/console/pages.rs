//! Server-rendered HTML pages for the fallback web administration console.
mod management;
mod system;

use super::*;
#[cfg(test)]
use serde_json::json;

fn card(title: &str, content: &str) -> String {
    format!(
        "<section class=\"card\"><h2>{}</h2>{content}</section>",
        i18n::message(title)
    )
}
fn rows<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value
        .get(key)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}
/// The same address on two host adapters is listed once.
fn loss_addresses(routes: &[Value]) -> String {
    let mut seen: Vec<&str> = Vec::new();
    for address in routes.iter().map(|r| text(r, "address")) {
        if !seen.contains(&address) {
            seen.push(address);
        }
    }
    seen.join(", ")
}
fn raw(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default()
}
fn timestamp(value: &Value) -> String {
    let seconds = value.as_i64().or_else(|| value.as_str()?.parse().ok());
    if let Some(date) =
        seconds.and_then(|seconds| time::OffsetDateTime::from_unix_timestamp(seconds).ok())
    {
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02} UTC",
            date.year(),
            date.month() as u8,
            date.day(),
            date.hour(),
            date.minute()
        )
    } else {
        value.as_str().unwrap_or("Not available").to_owned()
    }
}
fn scope_list(scopes: &[Value]) -> String {
    let mut html = "<ul class=\"permissions\">".to_owned();
    for scope in scopes {
        let methods = rows(scope, "methods")
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(", ");
        let path = text(scope, "path")
            .replace("[^/]+", "{id}")
            .replace("[0-9]+", "{number}");
        html += &format!(
            "<li><code>{}</code> <span>{}</span></li>",
            i18n::data(&path),
            i18n::data(&methods)
        );
    }
    html + "</ul>"
}
fn readiness(meta: &Value) -> String {
    let video = meta["encoder_status"]["h264"] == true;
    let checking = meta["encoder_status"]["state"] == "checking";
    let displays = rows(&meta["capture_status"], "displays");
    let virtual_requested = meta["capture_status"]["virtual_display_configured"] == true;
    let display_ready = if virtual_requested {
        meta["virtual_display"]["capable"] == true
    } else {
        !displays.is_empty()
    };
    let audio_ready = !rows(meta, "audio_sinks").is_empty();
    let mut out = String::new();
    for warning in rows(meta, "warnings") {
        out += &format!(
            "<p class=\"notice\" role=\"status\">{}</p>",
            i18n::data(text(warning, "message"))
        );
    }
    out += "<ul class=\"readiness\">";
    for (ready, label, detail) in [
        (
            video,
            "Video",
            if video {
                "Ready to stream. Choose a codec in Moonlight."
            } else if checking {
                "Checking your graphics card. Reload in a few seconds."
            } else {
                "No working encoder found. Retrying automatically."
            },
        ),
        (
            display_ready,
            "Display",
            if display_ready {
                "Ready to capture."
            } else if virtual_requested {
                "Virtual display is unavailable. Start the installed Rubylight service or select your physical display in Settings."
            } else {
                "No active display found. Turn on a monitor or set up a virtual display in Settings."
            },
        ),
        (
            audio_ready || meta["audio_enabled"] == false,
            "Audio",
            if meta["audio_enabled"] == false {
                "Audio streaming is switched off in Settings."
            } else if audio_ready {
                "An audio output is available. Choose the output used by your game in Settings."
            } else {
                "No active audio output found. Connect headphones or speakers, then check Audio in Settings."
            },
        ),
    ] {
        out += &format!(
            "<li><span class=\"badge{}\">{}</span><div><strong>{label}</strong><p>{detail}</p></div></li>",
            if ready {
                " ready"
            } else if label == "Video" && checking {
                ""
            } else {
                " warn"
            },
            if ready {
                "Ready"
            } else if label == "Video" && checking {
                "Checking"
            } else {
                "Needs attention"
            }
        );
    }
    out += "</ul>";
    for (label, detail) in [
        ("Virtual display", text(&meta["virtual_display"], "reason")),
        ("Audio", text(meta, "audio_error")),
        ("Screen capture", text(&meta["capture_status"], "error")),
    ] {
        if !detail.is_empty() {
            out += &format!(
                "<p class=\"notice\"><strong>{label}:</strong> {}</p>",
                i18n::data(detail)
            );
        }
    }
    card("Connection checklist", &out)
}
fn credentials(csrf: &str, setup: bool) -> String {
    let mut content = if setup {
        "<p>Create your administrator account to pair devices and manage this host.</p>".into()
    } else {
        field("currentUsername", "Current username", "", "text")
            + &field("currentPassword", "Current password", "", "password")
    };
    content += &field("newUsername", "Username", "", "text");
    content += &field(
        "newPassword",
        "New password (at least 8 characters)",
        "",
        "password",
    );
    content += &field("confirmNewPassword", "Confirm password", "", "password");
    content += if setup {
        "<button>Create account</button>"
    } else {
        "<button>Change password</button>"
    };
    card(
        if setup {
            "Set up your account"
        } else {
            "Change password"
        },
        &form(
            if setup { "setup" } else { "password" },
            csrf,
            if setup { "/setup" } else { "/settings" },
            &content,
        ),
    )
}
/// A waiting pairing request: where it came from first, as the tray says it, then the name the
/// device chose, which could name another address.
fn pending_card(
    csrf: &str,
    id: &str,
    name: &str,
    address: std::net::IpAddr,
    webrtc: bool,
) -> String {
    card(
        if webrtc {
            "Pairing request (WebRTC)"
        } else {
            "Pairing request"
        },
        &format!(
            "<p class=\"from\"><strong>Pairing request from {}</strong></p>{}",
            i18n::data(&butterpollo_stream::state::pairing_source(address)),
            form(
                "pair",
                csrf,
                "/devices",
                &(hidden("uniqueid", id)
                    + &field("name", "Device name", name, "text")
                    + &field("pin", "Moonlight PIN", "", "text")
                    + "<button>Pair device</button>"),
            )
        ),
    )
}

pub(super) async fn render(
    h: &Shared,
    connection: &Connection,
    headers: &HeaderMap,
    path: &str,
    csrf: &str,
    query: &Fields,
) -> Result<String, String> {
    match path {
        "/login" => Ok(login(csrf)),
        "/setup" => Ok(setup(csrf)),
        "/" => overview(h, connection, headers, csrf, query).await,
        "/library" => management::apps(h, connection, headers, csrf, query).await,
        "/devices" => management::devices(h, connection, headers, csrf).await,
        "/settings" => management::settings(h, connection, headers, csrf).await,
        "/logs" => system::logs(h, connection, headers).await,
        "/integrations" => system::integrations(h, connection, headers).await,
        "/api-tokens" => system::api_tokens(h, connection, headers, csrf).await,
        "/maintenance" => system::maintenance(h, connection, headers, csrf).await,
        _ => Err("Page not found".into()),
    }
}

fn login(csrf: &str) -> String {
    card(
        "Sign in",
        &form(
            "login",
            csrf,
            "/login",
            &(field("username", "Username", "", "text")
                + &field("password", "Password", "", "password")
                + "<label class=\"check\"><input type=\"checkbox\" name=\"remember_me\" value=\"true\"> Keep me signed in on this browser</label>"
                + "<button>Sign in</button>"),
        ),
    )
}

fn setup(csrf: &str) -> String {
    credentials(csrf, true)
}

async fn overview(
    h: &Shared,
    connection: &Connection,
    headers: &HeaderMap,
    csrf: &str,
    query: &Fields,
) -> Result<String, String> {
    let meta = get(h, connection, headers, "/api/metadata").await?;
    let sessions = get(h, connection, headers, "/api/rtsp/sessions").await?;
    let active = rows(&sessions, "sessions");
    let mut content = format!(
        "<p class=\"intro\">Manage your library, pair devices, and tune your stream.</p>{}<div class=\"metrics\"><section class=\"card\"><span>Host</span><strong>{}</strong><small>Version {}</small></section><section class=\"card\"><span>Active streams</span><strong>{}</strong><small>Connected right now</small></section><section class=\"card\"><span>Encoder</span><strong>{}</strong><small>Capture: {}</small></section></div>",
        super::theme::telemetry_strip(active),
        i18n::data(text(&meta, "host_name")),
        i18n::data(text(&meta, "version")),
        active.len(),
        match text(&meta["encoder_status"], "state") {
            "ready" => "Ready",
            "checking" => "Checking",
            "failed" => "Unavailable",
            _ => "Not available",
        },
        match text(&meta["capture_status"], "configured_backend") {
            "wgc" => "Windows Graphics Capture",
            "wgcc" => "Windows Graphics Capture, constant rate",
            "dxgi" | "ddx" => "Desktop Duplication",
            "idd-push" => "Direct driver push",
            "amd" => "AMD Direct Capture",
            "auto" | "" => "Automatic",
            _ => "Configured capture method",
        }
    );
    let mut badges = String::new();
    for (key, name) in [
        ("h264", "H.264"),
        ("hevc", "HEVC"),
        ("av1", "AV1"),
        ("pyrowave", "PyroWave"),
    ] {
        badges += &format!(
            "<span class=\"badge{}\">{} · {}</span>",
            if meta["encoder_status"][key] == true {
                " ready"
            } else {
                ""
            },
            name,
            if meta["encoder_status"][key] == true {
                "Ready"
            } else if meta["encoder_status"]["state"] == "checking" {
                "Checking"
            } else {
                "Unavailable"
            }
        );
    }
    // The native YUV 4:4:4 modes the encoder check found, as the codecs above.
    let full: Vec<&str> = meta["encoder_status"]["yuv444"]
        .as_array()
        .map(|modes| modes.iter().filter_map(|mode| mode.as_str()).collect())
        .unwrap_or_default();
    if !full.is_empty() {
        badges += &format!(
            "<span class=\"badge ready\">YUV 4:4:4 · {}</span>",
            i18n::data(&full.join(", ").replace(" 4:4:4", ""))
        );
    }
    content += &card(
        "Streaming capabilities",
        &format!(
            "<div class=\"badges\">{badges}</div><p>H.264, HEVC and AV1 work with Moonlight. PyroWave works with <a href=\"https://github.com/RamazanKara/nightjar\" target=\"_blank\" rel=\"noopener noreferrer\">Nightjar</a> on Android, and on a PC with <a href=\"https://github.com/Nonary/moonlight-qt\" target=\"_blank\" rel=\"noopener noreferrer\">Nonary’s version of Moonlight</a>, which VRR also requires. PyroWave is best suited to a fast local network.</p>"
        ),
    );
    content += &readiness(&meta);
    if meta["paired_devices"].as_u64().unwrap_or(0) == 0 {
        content += &card(
            "Your first stream",
            &format!(
                "<ol class=\"setup-steps\"><li><strong>Open Moonlight on your other device.</strong> Keep both devices on the same network. Add <code>{}</code> if this PC does not appear automatically.</li><li><strong>Enter the PIN shown by Moonlight.</strong> Open <a href=\"/devices\">Devices</a>, reload to see its pairing request, and enter that PIN.</li><li><strong>Open Desktop in Moonlight.</strong> Start with 1080p at 60 fps, then choose your preferred resolution, frame rate and HDR. Add games in <a href=\"/library\">Library</a>.</li></ol>",
                i18n::data(text(&meta, "pc_address"))
            ),
        );
        let addresses = rows(&meta, "pc_addresses");
        if addresses.len() > 1 {
            content += &format!(
                "<p>Other active network addresses: {}. Use the address on the same network as your Moonlight device.</p>",
                addresses
                    .iter()
                    .filter_map(Value::as_str)
                    .filter(|address| *address != text(&meta, "pc_address"))
                    .map(|address| format!("<code>{}</code>", i18n::data(address)))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        content += "<p class=\"muted\">If Moonlight cannot connect, allow Butterpollo.exe through Windows Firewall for your private network, then check for a pairing request in Devices.</p>";
    }
    content += &card(
        "Get started",
        "<div class=\"quick-links\"><a href=\"/devices\"><strong>Pair a device</strong><span>Connect Moonlight to this host</span></a><a href=\"/library\"><strong>Your library</strong><span>Manage apps and desktop streaming</span></a><a href=\"/settings\"><strong>Stream settings</strong><span>Choose your encoder, display, and audio</span></a></div>",
    );
    if active.is_empty() {
        content += &card(
            "Connections",
            "<p>No devices are streaming. Connect from Moonlight to start.</p>",
        );
    } else {
        let mut table = "<div class=\"table-scroll\"><table><thead><tr><th>Device</th><th>Video</th><th>Recent rate</th><th>Encoding time (95th percentile)</th><th></th></tr></thead><tbody>".to_owned();
        for session in active {
            let disconnect = button(
                "disconnect",
                csrf,
                "/",
                "uuid",
                text(session, "uuid"),
                "Disconnect",
            );
            if session["role"] == "input_only" {
                table += &format!(
                    "<tr><td data-label=\"Device\"><span>{}</span></td><td data-label=\"Connection\"><span>Remote input</span></td><td data-label=\"Status\" colspan=\"2\"><span>Input connection active</span></td><td class=\"table-actions\">{disconnect}</td></tr>",
                    i18n::data(text(session, "device_name")),
                );
                continue;
            }
            table += &format!(
                "<tr><td data-label=\"Device\"><span>{}</span></td><td data-label=\"Video\"><span>{}×{} · {} fps{}{}{}</span></td><td data-label=\"Recent rate\"><span>{:.1} fps<br>{:.1} Mbps</span></td><td data-label=\"Encoding time\"><span>{:.2} ms</span></td><td class=\"table-actions\">{disconnect}</td></tr>",
                i18n::data(text(session, "device_name")),
                session["width"],
                session["height"],
                session["fps"],
                if session["hdr"] == true {
                    " · HDR"
                } else {
                    ""
                },
                if session["vrr"] == true {
                    " · VRR"
                } else {
                    ""
                },
                if session["role"] == "remote_monitor" {
                    "<br>Remote monitor"
                } else {
                    ""
                },
                session["performance"]["fps"].as_f64().unwrap_or(0.),
                session["performance"]["bitrate_mbps"]
                    .as_f64()
                    .unwrap_or(0.),
                session["performance"]["encode_p95_ms"]
                    .as_f64()
                    .unwrap_or(0.),
            );
        }
        content += &card("Connections", &(table + "</tbody></table></div>"));
        for session in active {
            let skipped = session["frames_replaced"].as_u64().unwrap_or(0);
            if skipped > 0 {
                content += &format!(
                    "<p>{}: {skipped} older frames skipped to keep the stream current. If this keeps increasing, lower the PyroWave bitrate or check your wired connection.</p>",
                    i18n::data(text(session, "device_name"))
                );
            }
        }
        content += "<p class=\"muted\">Rates cover the last two seconds. An unchanged desktop may send fewer frames. Encoding time covers the host encoder; 95% of frames are encoded faster than the value shown. Decoding on the device and network delay add to it.</p>";
        for session in active {
            let history = rows(&session["performance"], "history");
            if history.is_empty() {
                continue;
            }
            let mut table = "<details><summary>Recent performance history</summary><div class=\"table-scroll\"><table><thead><tr><th>Sample</th><th>Frame rate (fps)</th><th>Mbps</th><th>Average encoding time</th><th>Slowest encoding time</th></tr></thead><tbody>".to_owned();
            for (i, sample) in history.iter().enumerate().rev().take(30) {
                table += &format!(
                    "<tr><td data-label=\"Sample\"><span>{}</span></td><td data-label=\"Frame rate\"><span>{:.1} fps</span></td><td data-label=\"Data rate\"><span>{:.1} Mbps</span></td><td data-label=\"Average encoding\"><span>{:.2} ms</span></td><td data-label=\"Slowest encoding\"><span>{:.2} ms</span></td></tr>",
                    i + 1,
                    sample["fps"].as_f64().unwrap_or(0.),
                    sample["bitrate_mbps"].as_f64().unwrap_or(0.),
                    sample["encode_mean_ms"].as_f64().unwrap_or(0.),
                    sample["encode_max_ms"].as_f64().unwrap_or(0.)
                );
            }
            table += "</tbody></table></div><p>The latest 30 completed intervals are shown. Each sample covers a completed measurement interval.</p></details>";
            content += &card(text(session, "device_name"), &table);
        }
    }
    Ok(content
        + if query.get("live").is_some_and(|v| v == "1") {
            "<p class=\"muted\">Updating every five seconds. <a href=\"/\">Pause updates</a></p>"
        } else {
            "<p class=\"muted\"><a href=\"/?live=1\">Update every five seconds</a></p>"
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn timestamps_accept_numeric_and_saved_text_values() {
        assert_eq!(timestamp(&json!(0)), "1970-01-01 00:00 UTC");
        assert_eq!(timestamp(&json!("60")), "1970-01-01 00:01 UTC");
        assert_eq!(timestamp(&Value::Null), "Not available");
    }
    #[test]
    fn an_address_remembered_on_two_adapters_is_listed_once() {
        let routes = json!([
            {"address":"192.0.2.9","interface":1},
            {"address":"192.0.2.9","interface":2},
            {"address":"192.0.2.10"}
        ]);
        assert_eq!(
            loss_addresses(routes.as_array().unwrap()),
            "192.0.2.9, 192.0.2.10"
        );
    }
    #[test]
    fn a_pending_card_shows_where_the_request_came_from_before_its_name() {
        let name = "Living room TV (from 192.168.4.20)";
        for (address, source) in [
            ("127.0.0.1", "this PC (a tunnel or proxy)"),
            ("192.168.4.30", "192.168.4.30"),
        ] {
            let html = i18n::render(
                &pending_card("token", "id", name, address.parse().unwrap(), true),
                "en",
            );
            let from = html
                .find(&format!("Pairing request from {source}"))
                .unwrap_or_else(|| panic!("{html}"));
            assert!(from < html.find("Living room TV").unwrap(), "{html}");
            assert!(!html.contains("127.0.0.1"), "{html}");
        }
    }
    #[test]
    fn token_permissions_are_readable_and_escaped() {
        let scopes = json!([{"path":"/api/apps/[^/]+/<script>","methods":["GET","DELETE"]}]);
        let html = i18n::render(&scope_list(scopes.as_array().unwrap()), "en");
        assert!(html.contains("/api/apps/{id}/&lt;script&gt;"));
        assert!(html.contains("GET, DELETE"));
        assert!(!html.contains("<pre>"));
        assert!(!html.contains("<script>"));
    }
    #[test]
    fn readiness_shows_escaped_encoder_warnings_until_recovery() {
        let mut meta = json!({
            "encoder_status":{"h264":false,"state":"failed"},
            "warnings":[{"code":"video_encoder","message":"No video encoder available: <driver error>. Retrying."}]
        });
        for state in ["failed", "checking"] {
            meta["encoder_status"]["state"] = json!(state);
            let html = i18n::render(&readiness(&meta), "en");
            assert!(html.contains("No video encoder available: &lt;driver error&gt;. Retrying."));
            assert!(html.contains("role=\"status\""));
            assert!(!html.contains("<driver error>"));
            assert!(!html.contains("<pre>"));
        }
        meta["encoder_status"] = json!({"h264":true,"state":"ready"});
        meta["warnings"] = json!([]);
        assert!(!readiness(&meta).contains("No video encoder available"));
    }
}
