//! Native API core routes: library listing, live session state/actions, couch menu, and USB/IP tunnel upgrade.
use super::{
    Action, Failure, Reply, caller, expect_empty_body, json, read_body, require,
    require_view_or_launch, session_action,
};
use axum::{
    Extension,
    extract::{Request, State},
    http::{StatusCode, header},
    response::IntoResponse,
};
use butterpollo_session::{
    native::{self, LAUNCH, LIST, VIEW},
    state::App,
};
use butterpollo_stream::{
    couch_menu as stream_couch_menu, library, mic, nucleus, state::Shared, tls::Connection,
    usb_forward,
};
use rubylight_native_api as native_api;
use rubylight_native_api::error::ErrorCode;
use std::{path::Path, sync::PoisonError, time::UNIX_EPOCH};

pub(super) async fn library(State(h): State<Shared>, Extension(c): Extension<Connection>) -> Reply {
    require(&caller(&h, &c)?, LIST, "List applications")?;
    let library = tokio::task::spawn_blocking(move || library_of(&h))
        .await
        .map_err(|error| Failure::internal("read the library", error))?;
    Ok(json(StatusCode::OK, &library))
}

fn library_of(h: &Shared) -> native_api::library::Library {
    h.reload_apps();
    let mut apps = h
        .apps
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    // The co-op entries follow the user's apps, as in `/applist`.
    let configured = nucleus::configured(&h.config.read().unwrap_or_else(PoisonError::into_inner));
    let coop = h.nucleus.entries(&apps, &configured);
    apps.extend(coop.into_iter().map(|coop| coop.entry));
    let current = h
        .current_app
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .map(|app| app.id);
    let scan = library::installed(h);
    if let Some(scan) = &scan {
        library::tell_collections_cut(&apps, scan);
    }
    // The same directory `/appasset` serves covers from.
    let assets = h.assets.parent().unwrap_or(&h.assets);
    native::library_with(&apps, scan.as_ref(), current, |app| {
        cover_fingerprint(app, assets)
    })
}

/// What identifies a cover image file: its path, size and modification time.
/// `None` when the app has none of its own and `/appasset` would send the
/// placeholder.
fn cover_fingerprint(app: &App, assets: &Path) -> Option<String> {
    let path = butterpollo_session::catalog::artwork(app, assets);
    if path == assets.join("box.png") {
        return None;
    }
    let (size, modified) = std::fs::metadata(&path).map_or((0, 0), |meta| {
        let modified = meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |elapsed| elapsed.as_nanos());
        (meta.len(), modified)
    });
    Some(format!("{}|{size}|{modified}", path.display()))
}

pub(super) async fn session(State(h): State<Shared>, Extension(c): Extension<Connection>) -> Reply {
    let client = caller(&h, &c)?;
    let snapshot = h.session_snapshot();
    let mut view = native::session_view(&snapshot, &client);
    view.microphone_busy = mic::busy_for(&h, &client.uuid);
    view.moved_to = native::moved_view(h.moved_note(&client.uuid), &client);
    view.owns_game = h.owns_game(&client.uuid);
    view.press_failed = h.press_failed(&client.uuid);
    Ok(json(StatusCode::OK, &view))
}

/// Continue here: the other devices' plain streams of the running game end, the game stays and
/// the caller starts its own stream of it with the normal resume.
pub(super) async fn continue_here(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    request: Request,
) -> Reply {
    caller(&h, &c)?;
    expect_empty_body(request).await?;
    tokio::task::spawn_blocking(move || {
        // The permission checks use the device as the console has it now.
        let client = caller(&h, &c)?;
        require(&client, VIEW, "View apps")?;
        require(&client, LAUNCH, "Launch applications")?;
        h.continue_here(&client)?;
        Ok(json(
            StatusCode::OK,
            &native::session_view(&h.session_snapshot(), &client),
        ))
    })
    .await
    .map_err(|error| Failure::internal("continue the game here", error))?
}

pub(super) async fn disconnect(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    request: Request,
) -> Reply {
    act(h, c, request, Action::Disconnect).await
}

pub(super) async fn end(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    request: Request,
) -> Reply {
    act(h, c, request, Action::End).await
}

pub(super) async fn close_game(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    request: Request,
) -> Reply {
    act(h, c, request, Action::CloseGame).await
}

async fn act(h: Shared, c: Connection, request: Request, action: Action) -> Reply {
    caller(&h, &c)?;
    expect_empty_body(request).await?;
    // End session and Close game wait up to five seconds for the streams.
    tokio::task::spawn_blocking(move || run(&h, &c, action))
        .await
        .map_err(|error| Failure::internal("run the action", error))?
}

/// Check the device, then run the action and answer with the new session.
/// The permission checks use the device as the console has it now, right
/// before the action runs.
fn run(h: &Shared, connection: &Connection, action: Action) -> Reply {
    let client = caller(h, connection)?;
    session_action(h, &client, action)?;
    let client = caller(h, connection).unwrap_or(client);
    Ok(json(
        StatusCode::OK,
        &native::session_view(&h.session_snapshot(), &client),
    ))
}

/// Opens or closes the couch menu in the caller's stream
/// (`docs/couch-menu-design.md` section 7.1). Idempotent.
pub(super) async fn couch_menu(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    request: Request,
) -> Reply {
    let client = caller(&h, &c)?;
    let body = read_body(request).await?;
    let request: native_api::session::CouchMenuRequest =
        serde_json::from_slice(&body).map_err(|error| {
            Failure::new(
                ErrorCode::InvalidRequest,
                format!("The body is not a valid couch menu request: {error}."),
            )
        })?;
    require_view_or_launch(&client)?;
    let open =
        tokio::task::spawn_blocking(move || stream_couch_menu::request(&h, &client, request.open))
            .await
            .map_err(|error| Failure::internal("open the couch menu", error))??;
    Ok(json(
        StatusCode::OK,
        &native_api::session::CouchMenu { open },
    ))
}

/// Upgrades to the USB/IP pipe of the device a `TunnelReady` token names, and relays it to the
/// driver until it detaches.
pub(super) async fn usb_tunnel(
    State(h): State<Shared>,
    Extension(c): Extension<Connection>,
    mut request: Request,
) -> Reply {
    use native_api::usb::{TOKEN_HEADER, UPGRADE};
    let client = caller(&h, &c)?;
    let enabled = h
        .config
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .usb_forwarding();
    if !enabled {
        return Err(Failure::new(
            ErrorCode::UsbDisabled,
            "USB forwarding is off on this host.",
        )
        .for_setting("usb_forwarding")
        .into());
    }
    if !client.allows_usb() {
        return Err(Failure::new(
            ErrorCode::UsbDisabled,
            "Turn on USB forwarding for this device in the Rubylight console.",
        )
        .into());
    }
    let headers = request.headers();
    let upgrades = headers
        .get(header::UPGRADE)
        .is_some_and(|value| value.as_bytes().eq_ignore_ascii_case(UPGRADE.as_bytes()));
    let token = headers
        .get(TOKEN_HEADER)
        .and_then(|value| usb_token(value.to_str().ok()?));
    let (true, Some(token)) = (upgrades, token) else {
        return Err(Failure::new(
            ErrorCode::InvalidRequest,
            format!("Open the tunnel with Upgrade: {UPGRADE} and the token in {TOKEN_HEADER}."),
        )
        .into());
    };
    let opened = h
        .usb
        .claim(&token, &client.cert, std::time::Instant::now())
        .map_err(|refused| match refused {
            usb_forward::Refused::Unknown => Failure::new(
                ErrorCode::NotFound,
                "This USB tunnel token is unknown or expired. Ask for the device again.",
            ),
            // Not usb_limit: the limit was checked when the device asked.
            usb_forward::Refused::Used => Failure::new(
                ErrorCode::NotFound,
                "This USB tunnel token was already used.",
            ),
        })?;
    let upgrade = hyper::upgrade::on(&mut request);
    tokio::spawn(async move {
        match upgrade.await {
            Ok(tunnel) => {
                h.usb
                    .serve(opened, hyper_util::rt::TokioIo::new(tunnel))
                    .await
            }
            Err(error) => h.usb.abandon(opened, error),
        }
    });
    Ok((
        StatusCode::SWITCHING_PROTOCOLS,
        [(header::CONNECTION, "upgrade"), (header::UPGRADE, UPGRADE)],
    )
        .into_response())
}

/// A tunnel token: 32 hex digits.
fn usb_token(hex: &str) -> Option<[u8; native_api::usb::TOKEN_BYTES]> {
    let mut token = [0; native_api::usb::TOKEN_BYTES];
    if hex.len() != token.len() * 2 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    for (byte, pair) in token.iter_mut().zip(hex.as_bytes().chunks(2)) {
        *byte = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some(token)
}

#[cfg(test)]
mod cover_tests {
    use super::*;

    #[test]
    fn an_app_without_a_cover_of_its_own_has_no_fingerprint() {
        let assets = std::env::temp_dir().join(format!("rubylight-cover-{}", std::process::id()));
        std::fs::create_dir_all(&assets).unwrap();
        let png = b"\x89PNG\r\n\x1a\n0000";
        std::fs::write(assets.join("box.png"), png).unwrap();
        std::fs::write(assets.join("dota.png"), png).unwrap();
        let app = |image: &str| -> App {
            serde_json::from_value(serde_json::json!({"name": "Dota 2", "image-path": image}))
                .unwrap()
        };
        let own = cover_fingerprint(&app("dota.png"), &assets);
        // None, a missing file and a file that isn't an image all end at the placeholder.
        let none = [app(""), app("gone.png"), app("notes.txt")]
            .map(|app| cover_fingerprint(&app, &assets));
        let _ = std::fs::remove_dir_all(&assets);
        assert!(own.unwrap().contains("dota.png"));
        assert_eq!(none, [None, None, None]);
    }
}
