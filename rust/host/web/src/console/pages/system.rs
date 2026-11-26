//! Server-rendered console pages for host logs, integrations, API tokens, and maintenance.
use super::{card, rows, scope_list, timestamp};
use crate::console::{area, button, form, get, hidden, i18n, text};
use crate::web;
use axum::{
    Extension,
    body::{Bytes, to_bytes},
    extract::State,
    http::{HeaderMap, Method},
};
use butterpollo_stream::{state::Shared, tls::Connection};

pub(super) async fn logs(
    h: &Shared,
    connection: &Connection,
    headers: &HeaderMap,
) -> Result<String, String> {
    let response = web::api(
        State(h.clone()),
        Extension(connection.clone()),
        Method::GET,
        "/api/logs".parse().unwrap(),
        headers.clone(),
        Bytes::new(),
    )
    .await;
    let bytes = to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .map_err(|e| e.to_string())?;
    let start = bytes.len().saturating_sub(256 * 1024);
    let logs = String::from_utf8_lossy(&bytes[start..]);
    Ok(card(
        "Host log",
        &format!(
            "<div class=\"actions\"><a class=\"button secondary\" href=\"/logs\">Reload</a><a class=\"button secondary\" href=\"/api/logs/export\">Download log</a><a class=\"button secondary\" href=\"/api/logs/export_crash\">Download support bundle</a></div><pre class=\"log\">{}</pre><small>Showing the most recent 256 KiB.</small>",
            i18n::data(&logs)
        ),
    ))
}

pub(super) async fn integrations(
    h: &Shared,
    connection: &Connection,
    headers: &HeaderMap,
) -> Result<String, String> {
    let meta = get(h, connection, headers, "/api/metadata").await?;
    let limiter = get(h, connection, headers, "/api/frame-limiter/status").await?;
    let mut features = "<dl>".to_owned();
    for (key, label) in [
        ("virtual_display", "Virtual displays"),
        ("pyrowave", "PyroWave"),
    ] {
        features += &format!(
            "<div><dt>{label}</dt><dd>{}</dd></div>",
            if meta["features"][key] == true {
                "Available"
            } else {
                "Unavailable on this computer"
            }
        );
    }
    features += "</dl>";
    Ok(card("Available integrations", &features)
        + &card(
            "External frame limiter",
            &format!(
                "<p>{}</p><dl><div><dt>Configured limiter</dt><dd>{}</dd></div><div><dt>RTSS</dt><dd>{}</dd></div></dl>",
                i18n::data(text(&limiter, "message")),
                match text(&limiter, "configured_provider") {
                    "auto" => "Automatic",
                    "rtss" => "RivaTuner Statistics Server (RTSS)",
                    "none" | "" => "None",
                    _ => "Other limiter",
                },
                if limiter["rtss_available"] == true {
                    "Detected"
                } else {
                    "Not detected"
                }
            ),
        ))
}

pub(super) async fn api_tokens(
    h: &Shared,
    connection: &Connection,
    headers: &HeaderMap,
    csrf: &str,
) -> Result<String, String> {
    let tokens = get(h, connection, headers, "/api/tokens").await?;
    let sessions = get(h, connection, headers, "/api/auth/sessions").await?;
    let routes = get(h, connection, headers, "/api/token/routes").await?;
    let mut content =
        "<p class=\"intro\">Give scripts access to selected API paths and manage signed-in browsers.</p>"
            .to_owned();
    let mut list = String::new();
    for token in rows(&tokens, "tokens") {
        list += &format!(
            "<div class=\"token-row\"><div><strong>{}</strong><small>Created: {}</small>{}</div>{}</div>",
            i18n::data(text(token, "username")),
            i18n::data(&timestamp(&token["created_at"])),
            scope_list(rows(token, "scopes")),
            button(
                "token-delete",
                csrf,
                "/api-tokens",
                "id",
                text(token, "hash"),
                "Revoke"
            )
        );
    }
    if list.is_empty() {
        list += "<p>No API tokens. Create one below for each script that needs access.</p>";
    }
    content += &card("API tokens", &list);
    content += &card(
        "Create token",
        &form(
            "token-create",
            csrf,
            "/api-tokens",
            &("<p>Select only the API paths and request methods the script needs. Enter permissions as JSON, using the example below. Copy the token after creating it.</p>"
                .to_owned()
                + &area(
                    "scopes",
                    "Token permissions (JSON)",
                    "[{\"path\":\"/api/metadata\",\"methods\":[\"GET\"]}]",
                )
                + &format!(
                    "<details><summary>Available API paths and methods</summary>{}</details><button>Create token</button>",
                    scope_list(rows(&routes, "routes"))
                )),
        ),
    );
    let mut list = String::new();
    for session in rows(&sessions, "sessions") {
        list += &format!(
            "<div class=\"token-row\"><div><strong>{}{}</strong><small>Created: {}</small></div>{}</div>",
            i18n::data(text(session, "username")),
            if session["current"] == true {
                " · This browser"
            } else {
                ""
            },
            i18n::data(&timestamp(&session["created_at"])),
            button(
                "session-revoke",
                csrf,
                "/api-tokens",
                "id",
                text(session, "id"),
                "Sign out"
            )
        );
    }
    if list.is_empty() {
        list += "<p>No browsers are signed in.</p>";
    }
    Ok(content + &card("Signed-in browsers", &list))
}

pub(super) async fn maintenance(
    h: &Shared,
    connection: &Connection,
    headers: &HeaderMap,
    csrf: &str,
) -> Result<String, String> {
    let golden = get(
        h,
        connection,
        headers,
        "/api/display/golden_status?compare_current=1",
    )
    .await?;
    let updates = get(h, connection, headers, "/api/updates").await?;
    let crash = get(h, connection, headers, "/api/health/crashdump").await?;
    let mut content =
        "<p class=\"intro\">Recover displays, inspect updates, and collect support information.</p>"
            .to_owned();
    let actions = button(
        "golden-capture",
        csrf,
        "/maintenance",
        "",
        "",
        "Save current layout",
    ) + &button(
        "golden-restore",
        csrf,
        "/maintenance",
        "",
        "",
        "Restore saved layout",
    ) + &button(
        "golden-delete",
        csrf,
        "/maintenance",
        "",
        "",
        "Delete saved layout",
    );
    content += &card(
        "Display recovery",
        &format!(
            "<p>Save a working display layout before changing your setup.</p><p>{}</p><div class=\"actions\">{actions}</div>",
            if golden["exists"] != true {
                "No display layout is saved."
            } else if golden["comparison_available"] != true {
                "A display layout is saved. It cannot be compared with the current layout right now."
            } else if !text(&golden, "current_mismatch_reason").is_empty() {
                "The saved layout differs from the current layout. Restore it to go back."
            } else {
                "The saved layout matches the current layout."
            }
        ),
    );
    content += &card(
        "Virtual displays",
        &format!(
            "<p>Disconnect remote monitors and release the displays owned by this host.</p>{}",
            button(
                "vdd-terminate",
                csrf,
                "/maintenance",
                "",
                "",
                "Disconnect virtual displays"
            )
        ),
    );
    let mut update_status = format!(
        "<p>{}</p><dl><div><dt>Latest version</dt><dd>{}</dd></div><div><dt>Automatic installation</dt><dd>{}</dd></div></dl>",
        match text(&updates, "phase") {
            "waiting" | "ready" => "The update is waiting for streams and apps to stop.",
            "downloading" => "Downloading the update.",
            "installing" => "Installing the update. Rubylight will restart shortly.",
            "failed" => "The update could not be installed.",
            _ if updates["checking"] == true => "Checking for updates.",
            _ if updates["update_available"] == true => "An update is available.",
            _ => "No update is queued.",
        },
        i18n::data(
            updates["latest_version"]
                .as_str()
                .unwrap_or("Not checked yet")
        ),
        if updates["auto_update"] == true {
            "On"
        } else {
            "Off"
        },
    );
    for key in ["check_error", "error"] {
        if let Some(error) = updates[key].as_str().filter(|error| !error.is_empty()) {
            update_status += &format!("<p class=\"notice error\">{}</p>", i18n::data(error));
        }
    }
    content += &card(
        "Updates",
        &("<div class=\"actions\">".to_owned()
            + &button(
                "check-update",
                csrf,
                "/maintenance",
                "",
                "",
                "Check for updates",
            )
            + &button(
                "install-update",
                csrf,
                "/maintenance",
                "",
                "",
                "Install when idle",
            )
            + &button(
                "install-update-now",
                csrf,
                "/maintenance",
                "",
                "",
                "Install now",
            )
            + &button(
                "cancel-update",
                csrf,
                "/maintenance",
                "",
                "",
                "Cancel queued update",
            )
            + &format!(
                "</div><p>Install when idle waits until streams and host apps stop. Install now ends a running stream and closes games started from Moonlight; connect again once Rubylight has restarted. Automatic installation is off by default and always waits; enable it in Settings.</p>{update_status}",
            )),
    );
    let mut crash_content = if crash["available"] == true {
        format!(
            "<p>{}</p><dl><div><dt>File</dt><dd>{}</dd></div><div><dt>Captured</dt><dd>{}</dd></div></dl>",
            if crash["dismissed"] == true {
                "No new crash reports. The last report was dismissed."
            } else {
                "A crash report is available. Download the support bundle with the crash memory dump to include it."
            },
            i18n::data(text(&crash, "filename")),
            i18n::data(text(&crash, "captured_at")),
        )
    } else {
        "<p>No crash reports from the last 7 days.</p>".to_owned()
    };
    if crash["available"] == true && crash["dismissed"] != true {
        let fields = hidden("filename", crash["filename"].as_str().unwrap_or(""))
            + &hidden("captured_at", crash["captured_at"].as_str().unwrap_or(""));
        crash_content += &form(
            "crash-dismiss",
            csrf,
            "/maintenance",
            &(fields + "<button type=\"submit\">Dismiss this report</button>"),
        );
    }
    content += &card("Crash reports", &crash_content);
    Ok(content
        + &card(
            "Support",
            "<p>The support bundle includes host logs and diagnostics for troubleshooting, with addresses, user and device names and game titles replaced by marks.</p><a class=\"button secondary\" href=\"/api/logs/export_crash\">Download support bundle</a><p>A crash memory dump can hold anything Rubylight had in memory, such as names, addresses and passwords. Include it only when asked, and share it privately.</p><a class=\"button secondary\" href=\"/api/logs/export_crash?include_dump=1\">Download with the crash memory dump</a>",
        ))
}
