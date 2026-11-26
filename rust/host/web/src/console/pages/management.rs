//! Server-rendered console pages for library apps, paired devices, and host settings.
use super::{card, credentials, loss_addresses, pending_card, raw, rows};
use crate::console::{
    Fields, PERMISSIONS, area, button, encoded, esc, field, form, get, hidden, i18n, select,
    settings as console_settings, text,
};
use axum::http::HeaderMap;
use butterpollo_stream::{state::Shared, tls::Connection};
use serde_json::json;

pub(super) async fn apps(
    h: &Shared,
    connection: &Connection,
    headers: &HeaderMap,
    csrf: &str,
    query: &Fields,
) -> Result<String, String> {
    let document = get(h, connection, headers, "/api/apps").await?;
    let apps = rows(&document, "apps");
    let mut content = "<p class=\"intro\">Launch your desktop or an app from any paired device.</p><div class=\"library-grid\">".to_owned();
    if apps.is_empty() {
        content += "<p>No apps yet. Add a game or program below, or leave its command empty to stream the desktop.</p>";
    }
    for app in apps {
        let uuid = text(app, "uuid");
        content += &format!(
            "<section class=\"card app\"><h2>{}</h2><p>{}</p><div class=\"actions\"><a class=\"button secondary\" href=\"/library?edit={}\">Edit</a>{}{}{}{}</div></section>",
            i18n::data(text(app, "name")),
            if text(app, "cmd").is_empty() {
                "Desktop streaming".into()
            } else {
                i18n::data(text(app, "cmd"))
            },
            encoded(uuid),
            button("app-launch", csrf, "/library", "uuid", uuid, "Launch"),
            button("app-delete", csrf, "/library", "uuid", uuid, "Remove"),
            form(
                "app-move",
                csrf,
                "/library",
                &(hidden("uuid", uuid)
                    + &hidden("direction", "up")
                    + "<button class=\"secondary\">Move up</button>")
            ),
            form(
                "app-move",
                csrf,
                "/library",
                &(hidden("uuid", uuid)
                    + &hidden("direction", "down")
                    + "<button class=\"secondary\">Move down</button>")
            )
        );
    }
    content += "</div>";
    let edit = query
        .get("edit")
        .and_then(|uuid| apps.iter().find(|a| text(a, "uuid") == uuid))
        .cloned()
        .unwrap_or(json!({"name":"","cmd":"","working-dir":""}));
    let mut advanced = edit.clone();
    let settings = edit.clone();
    for key in ["name", "cmd", "working-dir", "uuid"] {
        advanced.as_object_mut().unwrap().remove(key);
    }
    let editor = hidden("uuid", text(&edit, "uuid"))
        + &field("name", "App name", text(&edit, "name"), "text")
        + &field(
            "cmd",
            "Command (leave empty for desktop)",
            text(&edit, "cmd"),
            "text",
        )
        + &field(
            "working-dir",
            "Working directory",
            text(&edit, "working-dir"),
            "text",
        )
        + &console_settings::render("app_", console_settings::APP, &settings)
        + &format!(
            "<details><summary>Preparation commands and advanced options</summary><p>Edit these options as JSON. Use double quotes around text and matching brackets. Other saved options are kept when you save.</p>{}</details>",
            area("advanced", "App options (JSON)", &raw(&advanced))
        )
        + "<button>Save app</button> <a class=\"button secondary\" href=\"/library\">Cancel</a>";
    content += &card(
        if text(&edit, "uuid").is_empty() {
            "Add app"
        } else {
            "Edit app"
        },
        &form("app-save", csrf, "/library", &editor),
    );
    Ok(content + &button("app-close", csrf, "/library", "", "", "Close running app"))
}

pub(super) async fn devices(
    h: &Shared,
    connection: &Connection,
    headers: &HeaderMap,
    csrf: &str,
) -> Result<String, String> {
    let clients = get(h, connection, headers, "/api/clients/list").await?;
    let layout = get(h, connection, headers, "/api/clients/display-layout").await?;
    let pending: Vec<_> = h
        .pins
        .lock()
        .unwrap()
        .iter()
        .map(|(id, pin)| {
            (
                id.clone(),
                pin.name.clone(),
                pin.address,
                pin.certificate.is_empty(),
            )
        })
        .collect();
    let mut content =
        "<p class=\"intro\">Pair Moonlight devices and choose what each device can do.</p>"
            .to_owned();
    if pending.is_empty() {
        content += &card(
            "Pair a device",
            "<p>Add this host in Moonlight. Moonlight shows a four-digit PIN. Reload this page to see the pairing request, then enter that PIN here.</p><a class=\"button secondary\" href=\"/devices\">Check for a pairing request</a>",
        );
    }
    for (id, name, address, webrtc) in pending {
        content += &pending_card(csrf, &id, &name, address, webrtc);
    }
    let paired = rows(&clients, "clients");
    if paired.is_empty() {
        content += &card("Paired devices", "<p>No devices paired yet.</p>");
    }
    for client in paired {
        let uuid = text(client, "uuid");
        let perm = client["perm"].as_u64().unwrap_or(0) as u32;
        let mut editor =
            hidden("uuid", uuid) + &field("name", "Device name", text(client, "name"), "text");
        editor += &format!(
            "<label class=\"check\"><input type=\"checkbox\" name=\"enabled\"{}>Allow this device to connect</label><fieldset><legend>Permissions</legend><div class=\"checks\">",
            if client["enabled"] == true {
                " checked"
            } else {
                ""
            }
        );
        for (bit, label) in PERMISSIONS {
            editor += &format!(
                "<label class=\"check\"><input type=\"checkbox\" name=\"perm_{bit}\"{}>{label}</label>",
                if perm & bit != 0 { " checked" } else { "" }
            );
        }
        editor += "</div></fieldset>";
        editor += &console_settings::render("client_", console_settings::CLIENT, client);
        // What this form rendered, so a save from a stale page does not send back a
        // layout the user did not touch (issue #40).
        editor += &hidden("layout_original", text(client, "virtual_display_layout"));
        let mut advanced = client.clone();
        for key in [
            "name",
            "uuid",
            "perm",
            "enabled",
            "connected",
            "cert",
            "webrtc",
            "network_loss_routes",
            "client_agent",
            "client_warning",
        ] {
            advanced.as_object_mut().unwrap().remove(key);
        }
        editor += &format!(
            "<details><summary>Display preferences and connection commands</summary><p>Edit these options as JSON. Use double quotes around text and matching brackets.</p>{}</details>",
            area("advanced", "Device options (JSON)", &raw(&advanced))
        );
        editor += "<button>Save device</button>";
        let mut actions = button("disconnect", csrf, "/devices", "uuid", uuid, "Disconnect")
            + &button("unpair", csrf, "/devices", "uuid", uuid, "Unpair");
        let routes = rows(client, "network_loss_routes");
        let memory = if routes.is_empty() {
            String::new()
        } else {
            actions += &button(
                "network-reset",
                csrf,
                "/devices",
                "uuid",
                uuid,
                "Reset network history",
            );
            format!(
                "<p>Recent video loss at {}. Future streams start with smaller packet bursts. History expires after 24 hours; every second stream tests the normal rate. Reset applies to the next stream.</p>",
                esc(&loss_addresses(routes))
            )
        };
        content += &card(
            text(client, "name"),
            &format!(
                "<p><span class=\"badge\">{}</span>{}</p>{memory}{}<div class=\"actions\">{actions}</div>",
                if client["connected"] == true {
                    "Connected"
                } else {
                    "Paired"
                },
                if client.get("webrtc").is_some() {
                    " <span class=\"badge\">WebRTC</span>"
                } else {
                    ""
                },
                form("client-save", csrf, "/devices", &editor)
            ),
        );
    }
    Ok(content
        + &card(
            "Remote monitor layout",
            &format!(
                "<p>Set the position, size and availability of remote monitors kept between connections. Edit the layout as JSON, using double quotes around text and matching brackets.</p><details><summary>Edit display layout</summary>{}</details>",
                form(
                    "layout",
                    csrf,
                    "/devices",
                    &(area("layout", "Display layout (JSON)", &raw(&layout))
                        + "<button>Save layout</button>")
                )
            ),
        ))
}

pub(super) async fn settings(
    h: &Shared,
    connection: &Connection,
    headers: &HeaderMap,
    csrf: &str,
) -> Result<String, String> {
    let config = get(h, connection, headers, "/api/config").await?;
    let value = |key: &str, default: &str| match config.get(key) {
        Some(serde_json::Value::String(text)) => text.clone(),
        Some(serde_json::Value::Number(number)) => number.to_string(),
        Some(serde_json::Value::Bool(flag)) => flag.to_string(),
        _ => default.to_owned(),
    };
    let mut content =
        "<p class=\"intro\">Choose the capture, encoder, display, and sound for your stream.</p>"
            .to_owned();
    content += "<p class=\"muted\">Automatic encodes H.264, HEVC and AV1 with the graphics card's own encoder, AMD AMF or NVIDIA NVENC, and PyroWave streams with PyroWave's own encoder. If the encoder cannot start, the stream fails with its error in the log; there is no other encoder to fall back to.</p>";
    content += "<p class=\"muted\">For screen capture, Automatic uses direct driver push on Rubylight virtual displays and Windows Graphics Capture on other displays. Select Windows Graphics Capture or Desktop Duplication to use those methods instead. If driver push is unavailable, capture falls back to Windows Graphics Capture. Windows Graphics Capture, constant rate sends the last picture again whenever a game has no new one ready for a stream frame, so a game capped below the stream's frame rate keeps one frame per stream frame. Every repeat costs bitrate and a still screen keeps the full rate, so choose it only for a game capped below the stream's frame rate.</p>";
    let editor = field(
        "sunshine_name",
        "Host name",
        &value(
            "sunshine_name",
            &butterpollo_stream::network::host_name(&h.config.read().unwrap()),
        ),
        "text",
    ) + &select(
        "encoder",
        "Video encoder",
        &value("encoder", "auto"),
        &[
            ("auto", "Automatic"),
            ("amf", "AMD AMF"),
            ("nvenc", "NVIDIA NVENC"),
            ("pyrowave", "PyroWave"),
        ],
    ) + &select(
        "capture",
        "Screen capture",
        &value("capture", "auto"),
        &[
            ("auto", "Automatic"),
            ("idd-push", "Direct driver push"),
            ("wgc", "Windows Graphics Capture"),
            ("wgcc", "Windows Graphics Capture, constant rate"),
            ("dxgi", "Desktop Duplication"),
            ("amd", "AMD Direct Capture"),
        ],
    ) + &field(
        "output_name",
        "Display (leave empty for the primary display)",
        &value("output_name", ""),
        "text",
    ) + &select(
        "virtual_display_mode",
        "Virtual display",
        &value(
            "virtual_display_mode",
            if butterpollo_win_display::display::virtual_display_os() {
                "per_client"
            } else {
                "disabled"
            },
        ),
        &[
            ("disabled", "Disabled"),
            ("per_client", "One for each device"),
            ("shared", "Shared"),
        ],
    ) + &field(
        "audio_sink",
        "Audio device (leave empty for the Windows default)",
        &value("audio_sink", ""),
        "text",
    );
    let mut advanced = config.clone();
    for key in [
        "status",
        "platform",
        "version",
        "sunshine_name",
        "encoder",
        "capture",
        "output_name",
        "virtual_display_mode",
        "audio_sink",
    ] {
        advanced.as_object_mut().unwrap().remove(key);
    }
    content += &card(
        "Stream settings",
        &form(
            "config",
            csrf,
            "/settings",
            &(editor
                + &console_settings::render("cfg_", console_settings::GLOBAL, &config)
                + &format!(
                    "<details><summary>Other settings</summary><p>Edit these settings as JSON. Keep the configuration keys unchanged and use double quotes around text.</p>{}{}</details><button>Save settings</button>",
                    hidden("advanced_original", &raw(&advanced)),
                    area(
                        "advanced",
                        "Additional configuration (JSON)",
                        &raw(&advanced)
                    )
                )),
        ),
    );
    content += &credentials(csrf, false);
    Ok(content
        + &card(
            "Apply changes",
            &format!(
                "<p>Restart after changing encoder, capture, network, or display settings. Active streams will disconnect.</p>{}",
                button("restart", csrf, "/settings", "", "", "Restart host")
            ),
        ))
}
