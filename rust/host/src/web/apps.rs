//! Application library, artwork, and launcher API handlers.

use super::{cover_id, error};
use crate::state::Shared;
use axum::{
    Json,
    http::{Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use base64::Engine;
use butterpollo_core::{state, state::App};
use serde_json::{Value, json};
use std::collections::HashSet;

pub(super) fn handle(h: &Shared, method: &str, path: &str, data: &Value) -> anyhow::Result<Value> {
    let text = |k: &str| data.get(k).and_then(Value::as_str).unwrap_or("");
    Ok(match (method, path) {
        ("GET", "/api/apps") => {
            let mut d = h.app_document.read().unwrap().clone();
            d["apps"] = apps_with_install_state(h)?;
            d["status"] = json!(true);
            d
        }
        ("POST", "/api/apps") => {
            let mut app: App = serde_json::from_value(data.clone())?;
            if app.name.trim().is_empty() {
                anyhow::bail!("application name required");
            }
            if let Some(overrides) = app.extra.get("config-overrides") {
                butterpollo_core::stream_preset::check_overrides(overrides)?;
            }
            // A Radeon profile the host cannot read is an error here, not a game that starts
            // without it.
            butterpollo_core::radeon_profile::check(&app.extra)?;
            butterpollo_core::device_overrides::check(&app.extra)?;
            crate::game_files::check(&app)?;
            if !app.extra.contains_key("uuid") {
                app.extra
                    .insert("uuid".into(), json!(uuid::Uuid::new_v4().to_string()));
            }
            let uuid = app.extra["uuid"].clone();
            let mut apps = h.apps.write().unwrap();
            let mut next = apps.clone();
            crate::neural::keep_key(
                next.iter().find(|a| a.extra.get("uuid") == Some(&uuid)),
                &mut app,
            );
            // Only a sync stamps an app. A synced app saved here becomes the user's, so no sync
            // renames or removes it again.
            app.extra.remove(butterpollo_core::library::SYNCED_BY);
            app.extra.remove(butterpollo_core::native::NOT_INSTALLED);
            if let Some(sync) = next
                .iter()
                .find(|a| a.extra.get("uuid") == Some(&uuid))
                .and_then(|a| a.extra.get(butterpollo_core::library::SYNCED_BY)?.as_str())
            {
                let mut value = serde_json::to_value(&app)?;
                butterpollo_core::library::hand_over(&mut value, sync);
                app = serde_json::from_value(value)?;
            }
            if let Some(i) = next.iter().position(|a| a.extra.get("uuid") == Some(&uuid)) {
                next[i] = app;
            } else {
                next.push(app);
            }
            h.assign_apps(&mut next)?;
            let mut doc = h.app_document.read().unwrap().clone();
            doc["apps"] = serde_json::to_value(&next)?;
            state::write_json(&h.apps_path, &doc)?;
            *apps = next;
            *h.app_document.write().unwrap() = doc;
            h.config_version
                .fetch_add(1, std::sync::atomic::Ordering::Release);
            json!({"status":true,"uuid":uuid})
        }
        ("POST", "/api/apps/delete") => {
            // One app by `uuid`, or the console's selection by `uuids`, in one save.
            let ids: Vec<&str> = match data.get("uuids") {
                Some(list) => list
                    .as_array()
                    .ok_or_else(|| anyhow::anyhow!("uuids must be an array"))?
                    .iter()
                    .map(|id| {
                        id.as_str()
                            .ok_or_else(|| anyhow::anyhow!("uuids must be strings"))
                    })
                    .collect::<anyhow::Result<_>>()?,
                None => vec![text("uuid")],
            };
            let deleted = delete_apps(h, &ids)?;
            json!({"status":true,"deleted":deleted.count,"kept":deleted.kept})
        }
        ("POST", "/api/apps/reorder") => {
            let order = data["order"]
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("order must be an array"))?;
            let mut apps = h.apps.write().unwrap();
            let next = butterpollo_core::catalog::reorder(&apps, order);
            let mut document = h.app_document.read().unwrap().clone();
            document["apps"] = serde_json::to_value(&next)?;
            state::write_json(&h.apps_path, &document)?;
            *apps = next;
            *h.app_document.write().unwrap() = document;
            json!({"status":true})
        }
        ("POST", "/api/apps/close") => {
            let canceled = h
                .sessions
                .lock()
                .unwrap()
                .stop_role(butterpollo_core::session::Role::Stream, None);
            drop(canceled);
            h.stop_app(crate::state::AppStop::WebUi);
            json!({"status":true})
        }
        ("POST", "/api/apps/launch") => {
            let _transition = h.launch_transition.lock().unwrap();
            if crate::updater::installing(h) {
                anyhow::bail!("Rubylight is installing an update");
            }
            let uuid = text("uuid");
            let app = h
                .apps
                .read()
                .unwrap()
                .iter()
                .find(|a| a.extra.get("uuid").and_then(Value::as_str) == Some(uuid))
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("application not found"))?;
            if h.current_app.lock().unwrap().is_some() {
                anyhow::bail!("application already running");
            }
            // Started without the lock every serverinfo request takes: its
            // prep commands can run for minutes. launch_transition keeps
            // another app from being installed meanwhile.
            let running = crate::process::launch(h, &app, &Default::default())?;
            let mut current = h.current_app.lock().unwrap();
            if current.is_some() {
                drop(current);
                drop(running);
                anyhow::bail!("application already running");
            }
            let started = running.started_event();
            *current = Some(running);
            drop(current);
            h.app_started_webhook(started);
            json!({"status":true})
        }
        ("DELETE", p) if p.starts_with("/api/apps/") => {
            delete_apps(h, &[&p[10..]])?;
            json!({"status":true})
        }
        _ => return Err(anyhow::anyhow!("unknown API endpoint")),
    })
}

pub(super) async fn upload_cover(h: &Shared, data: &Value) -> Response {
    let text = |k: &str| data.get(k).and_then(Value::as_str).unwrap_or("");
    let result = async {
        let key = text("key");
        if key.is_empty()
            || key.len() > 128
            || !key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            anyhow::bail!("invalid cover key");
        }
        if let Some(data) = data.get("data").and_then(Value::as_str) {
            // The request body limit bounds the size.
            let bytes = base64::engine::general_purpose::STANDARD.decode(data)?;
            if bytes.len() < 24 || !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
                anyhow::bail!("cover must be PNG");
            }
            let path = h.directory.join("covers").join(format!("{key}.png"));
            state::atomic_write(&path, &bytes)?;
            return Ok(json!({"status":true,"path":path}));
        }
        let url = url::Url::parse(text("url"))?;
        if url.scheme() != "https"
            || url.host_str() != Some("images.igdb.com")
            || !url.path().starts_with("/igdb/image/upload/")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port().is_some()
        {
            anyhow::bail!("cover URL must use the IGDB image service");
        }
        let client = reqwest::Client::builder()
            .https_only(true)
            .timeout(std::time::Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        let mut response = client.get(url).send().await?.error_for_status()?;
        let mut bytes = vec![];
        while let Some(chunk) = response.chunk().await? {
            if bytes.len() + chunk.len() > 16 * 1024 * 1024 {
                anyhow::bail!("cover exceeds the limit");
            }
            bytes.extend_from_slice(&chunk);
        }
        if bytes.len() < 24 || !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            anyhow::bail!("cover must be PNG");
        }
        let path = h.directory.join("covers").join(format!("{key}.png"));
        state::atomic_write(&path, &bytes)?;
        Ok::<_, anyhow::Error>(json!({"status":true,"path":path}))
    }
    .await;
    match result {
        Ok(value) => Json(value).into_response(),
        Err(err) => error(StatusCode::BAD_REQUEST, &err.to_string()),
    }
}

pub(super) async fn steam(
    h: &Shared,
    method: &Method,
    action: &str,
    uri: &axum::http::Uri,
    data: &Value,
) -> Response {
    // Reading the libraries and converting covers waits on the disk.
    let h = h.clone();
    let appid = |value: Option<&Value>| -> Option<u32> {
        match value? {
            Value::Number(n) => n.as_u64().and_then(|n| u32::try_from(n).ok()),
            Value::String(s) => s.trim().parse().ok(),
            _ => None,
        }
    };
    let query: std::collections::HashMap<String, String> =
        url::form_urlencoded::parse(uri.query().unwrap_or("").as_bytes())
            .into_owned()
            .collect();
    let task = match (method.as_str(), action) {
        ("GET", "status") => {
            tokio::task::spawn_blocking(move || Ok(crate::steam::status(&h))).await
        }
        ("GET", "games") => {
            let id = query.get("appid").and_then(|v| v.parse().ok());
            tokio::task::spawn_blocking(move || crate::steam::games(&h, id)).await
        }
        ("POST", "force_sync") => {
            tokio::task::spawn_blocking(move || {
                crate::steam::sync(&h).map(|outcome| {
                    json!({"status":true,"changed":outcome.changed,"game_count":outcome.games,"importable_game_count":outcome.importable})
                })
            })
            .await
        }
        ("POST", "launch") => {
            let Some(id) = appid(data.get("appid")).or_else(|| appid(data.get("steam_id")))
            else {
                return error(StatusCode::BAD_REQUEST, "appid required");
            };
            tokio::task::spawn_blocking(move || crate::steam::launch(&h, id)).await
        }
        _ => return error(StatusCode::BAD_REQUEST, "unknown API endpoint"),
    };
    match task {
        Ok(Ok(value)) => Json(value).into_response(),
        Ok(Err(err)) => error(StatusCode::BAD_REQUEST, &format!("{err:#}")),
        Err(err) => error(StatusCode::INTERNAL_SERVER_ERROR, &err.to_string()),
    }
}

pub(super) async fn playnite(h: &Shared, method: &Method, action: &str, data: &Value) -> Response {
    let text = |k: &str| data.get(k).and_then(Value::as_str).unwrap_or("");
    let h = h.clone();
    let task = match (method.as_str(), action) {
        ("GET", "status") => {
            tokio::task::spawn_blocking(move || Ok(crate::playnite::status(&h))).await
        }
        ("GET", "games") => tokio::task::spawn_blocking(crate::playnite::games).await,
        ("GET", "categories") => tokio::task::spawn_blocking(crate::playnite::categories).await,
        ("POST", "install") => {
            tokio::task::spawn_blocking(move || {
                crate::playnite::install_plugin(&h).map(|folder| {
                    json!({"status":true,"path":folder,
                           "restart_required":butterpollo_windows::playnite::running().is_some()})
                })
            })
            .await
        }
        ("POST", "uninstall") => {
            tokio::task::spawn_blocking(|| crate::playnite::uninstall_plugin().map(|()| json!({"status":true}))).await
        }
        ("POST", "force_sync") => {
            tokio::task::spawn_blocking(move || {
                crate::playnite::sync(&h)
                    .map(|outcome| json!({"status":true,"changed":outcome.changed,"game_count":outcome.games}))
            })
            .await
        }
        ("POST", "cover") => {
            let (id, key) = (text("playnite_id").to_owned(), text("cover_key").to_owned());
            if id.is_empty() || key.is_empty() {
                return error(StatusCode::BAD_REQUEST, "Playnite game ID and cover key are required");
            }
            tokio::task::spawn_blocking(move || {
                crate::playnite::set_cover(&h, &id, &key)
                    .map(|path| json!({"status":true,"path":path.to_string_lossy().replace('\\', "/")}))
            })
            .await
        }
        ("POST", "launch") => {
            tokio::task::spawn_blocking(|| crate::playnite::restart().map(|()| json!({"status":true}))).await
        }
        _ => return error(StatusCode::BAD_REQUEST, "unknown API endpoint"),
    };
    match task {
        Ok(Ok(value)) => Json(value).into_response(),
        Ok(Err(err)) => error(StatusCode::BAD_REQUEST, &format!("{err:#}")),
        Err(err) => error(StatusCode::INTERNAL_SERVER_ERROR, &err.to_string()),
    }
}

pub(super) async fn lossless_scaling(h: &Shared, uri: &axum::http::Uri) -> Response {
    let candidate = url::form_urlencoded::parse(uri.query().unwrap_or("").as_bytes())
        .find(|(key, _)| key == "path")
        .map(|(_, value)| value.into_owned());
    if candidate.as_ref().is_some_and(|path| {
        let path = std::path::Path::new(path.trim().trim_matches('"'));
        !path.as_os_str().is_empty()
            && (!path.has_root()
                || !matches!(path.components().next(), Some(std::path::Component::Prefix(prefix)) if matches!(prefix.kind(), std::path::Prefix::Disk(_))))
    }) {
        return error(StatusCode::BAD_REQUEST, "path must be an absolute local drive path");
    }
    let configured = h
        .config
        .read()
        .unwrap()
        .get("lossless_scaling_path", "")
        .to_owned();
    let checked = candidate.unwrap_or_else(|| configured.clone());
    let result = tokio::task::spawn_blocking(move || {
        let resolved = butterpollo_windows::lossless::program(&checked);
        let status = match (&resolved, checked.trim().is_empty()) {
            (Some(_), _) => "detected",
            (None, true) => "not-configured",
            (None, false) => "path-not-found",
        };
        json!({"status": status, "configured_path": configured, "checked_path": checked,
               "resolved_path": resolved, "using_configured_path": !configured.trim().is_empty()})
    })
    .await;
    match result {
        Ok(value) => Json(value).into_response(),
        Err(err) => error(StatusCode::INTERNAL_SERVER_ERROR, &err.to_string()),
    }
}

pub(super) async fn browse(uri: &axum::http::Uri) -> Response {
    let query: std::collections::HashMap<String, String> =
        url::form_urlencoded::parse(uri.query().unwrap_or("").as_bytes())
            .into_owned()
            .collect();
    let filter = butterpollo_core::browse::Filter::parse(query.get("type").map(String::as_str));
    let requested = query.get("path").cloned().unwrap_or_default();
    // Listing a folder waits on the disk, or on the network for a share.
    let result = tokio::task::spawn_blocking(move || {
        if butterpollo_core::browse::root(&requested) {
            Ok(butterpollo_core::browse::drives(
                &butterpollo_windows::files::drives(),
            ))
        } else {
            butterpollo_core::browse::listing(std::path::Path::new(&requested), filter)
        }
    })
    .await;
    match result {
        Ok(Ok(value)) => Json(value).into_response(),
        Ok(Err(message)) => error(StatusCode::BAD_REQUEST, &message),
        Err(err) => error(StatusCode::INTERNAL_SERVER_ERROR, &err.to_string()),
    }
}

pub(super) async fn icon(h: &Shared, id: &str) -> Response {
    let icon = h
        .apps
        .read()
        .unwrap()
        .iter()
        .find(|a| a.extra.get("uuid").and_then(Value::as_str) == Some(id))
        .and_then(butterpollo_core::catalog::icon);
    if let Some(icon) = icon
        && let Ok(bytes) = tokio::fs::read(icon).await
    {
        return ([(header::CONTENT_TYPE, "image/png")], bytes).into_response();
    }
    StatusCode::NOT_FOUND.into_response()
}

/// Blocks a worker, not the web server: a game it drops loses Neural
/// Rendering, which runs a helper process for each.
pub(super) async fn purge_autosync(h: &Shared) -> Response {
    let h = h.clone();
    let purged = tokio::task::spawn_blocking(move || {
        let mut removed = 0;
        crate::steam::update_apps(&h, |apps| {
            removed = butterpollo_core::playnite::purge(apps);
            removed > 0
        })
        .map(|_| removed)
    })
    .await;
    match purged {
        Ok(Ok(removed)) => Json(json!({"status":true,"removed":removed})).into_response(),
        Ok(Err(err)) => error(StatusCode::INTERNAL_SERVER_ERROR, &format!("{err:#}")),
        Err(err) => error(StatusCode::INTERNAL_SERVER_ERROR, &err.to_string()),
    }
}

pub(super) async fn cover(h: &Shared, path: &str) -> Response {
    let Some(id) = cover_id(path) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let app = h
        .apps
        .read()
        .unwrap()
        .iter()
        .find(|a| {
            a.extra.get("uuid").and_then(Value::as_str) == Some(id) || a.id().to_string() == id
        })
        .cloned();
    let Some(app) = app else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let path = butterpollo_core::catalog::artwork(&app, h.assets.parent().unwrap_or(&h.assets));
    if tokio::fs::metadata(&path)
        .await
        .is_ok_and(|m| m.len() <= 16 * 1024 * 1024)
        && let Ok(bytes) = tokio::fs::read(path).await
    {
        return ([(header::CONTENT_TYPE, "image/png")], bytes).into_response();
    }
    StatusCode::NOT_FOUND.into_response()
}

/// The most apps one delete takes.
const MOST_DELETED: usize = 10_000;

/// What a delete did: how many apps went, and the ones kept because Neural Rendering could
/// not be taken out of their folder (`{"name", "error"}`).
#[derive(Debug)]
struct Deleted {
    count: usize,
    kept: Vec<Value>,
}

/// Deletes the apps with these uuids or ids in one save. An id that matches nothing is
/// skipped (a sync may have dropped its uninstalled game meanwhile); none matching is an
/// error.
fn delete_apps(h: &Shared, ids: &[&str]) -> anyhow::Result<Deleted> {
    delete_apps_with(h, ids, crate::neural::Removing::remove_mod)
}

/// [`delete_apps`] with the Neural Rendering removal given, so a test can fail one.
fn delete_apps_with(
    h: &Shared,
    ids: &[&str],
    remove_mod: impl Fn(&crate::neural::Removing, &App) -> anyhow::Result<()>,
) -> anyhow::Result<Deleted> {
    if ids.is_empty() {
        anyhow::bail!("Select at least one app.");
    }
    if ids.len() > MOST_DELETED {
        anyhow::bail!("at most {MOST_DELETED} apps can be deleted at once");
    }
    let wanted: HashSet<&str> = ids.iter().copied().collect();
    let key = |a: &App| {
        a.extra
            .get("uuid")
            .and_then(Value::as_str)
            .map_or_else(|| a.id().to_string(), str::to_owned)
    };
    let matches = |a: &App| {
        a.extra
            .get("uuid")
            .and_then(Value::as_str)
            .is_some_and(|uuid| wanted.contains(uuid))
            || wanted.contains(a.id().to_string().as_str())
    };
    // Before the apps are locked: the removal runs a helper process.
    let doomed: Vec<App> = h
        .apps
        .read()
        .unwrap()
        .iter()
        .filter(|a| matches(a))
        .cloned()
        .collect();
    if doomed.is_empty() {
        anyhow::bail!("application not found");
    }
    // Every app's Neural Rendering slot first, each held until the app is gone: one that is
    // busy refuses the delete before any mod is taken out.
    let removing = doomed
        .iter()
        .map(|app| crate::neural::reserve(h, app))
        .collect::<anyhow::Result<Vec<_>>>()?;
    // An app whose mod stays in its folder stays in the library, the others still go.
    let mut failed = HashSet::new();
    let mut kept = vec![];
    let mut first_error = None;
    for (app, slot) in doomed.iter().zip(&removing) {
        if let Err(error) = remove_mod(slot, app) {
            failed.insert(key(app));
            kept.push(json!({"name": app.name, "error": format!("{error:#}")}));
            first_error.get_or_insert(error);
        }
    }
    if failed.len() == doomed.len()
        && let Some(error) = first_error
    {
        return Err(error);
    }
    let mut apps = h.apps.write().unwrap();
    let mut next = apps.clone();
    let before = next.len();
    let mut deleted = vec![];
    next.retain(|a| {
        let keep = !matches(a) || failed.contains(&key(a));
        if !keep {
            deleted.push(serde_json::to_value(a));
        }
        keep
    });
    if next.len() == before {
        anyhow::bail!("application not found");
    }
    h.assign_apps(&mut next)?;
    let mut doc = h.app_document.read().unwrap().clone();
    // A deleted game stays deleted: no sync adds it again until an import or a restore
    // brings it back.
    let count = deleted.len();
    for app in deleted {
        crate::library::dismiss(&mut doc, &app?);
    }
    doc["apps"] = serde_json::to_value(&next)?;
    state::write_json(&h.apps_path, &doc)?;
    *apps = next;
    *h.app_document.write().unwrap() = doc;
    h.config_version
        .fetch_add(1, std::sync::atomic::Ordering::Release);
    Ok(Deleted { count, kept })
}

pub(super) async fn library(h: &Shared, method: &Method, action: &str, data: &Value) -> Response {
    let h = h.clone();
    let task = match (method.as_str(), action) {
        ("GET", "discovery") => {
            tokio::task::spawn_blocking(move || crate::library::discovery(&h)).await
        }
        ("POST", "sync") => tokio::task::spawn_blocking(move || crate::library::sync(&h)).await,
        ("POST", "import") => {
            let selected: Vec<butterpollo_core::library::Selection> =
                match serde_json::from_value(data["games"].clone()) {
                    Ok(selected) => selected,
                    Err(_) => {
                        return error(
                            StatusCode::BAD_REQUEST,
                            "games must contain source and id pairs",
                        );
                    }
                };
            tokio::task::spawn_blocking(move || crate::library::import(&h, &selected)).await
        }
        ("POST", "restore") => {
            // `{"all": true}`, or `{"games": ["steam:570", ...]}` from `launcher-dismissed`.
            let keys: Option<Vec<String>> = if data["all"] == true {
                None
            } else {
                match serde_json::from_value::<Vec<String>>(data["games"].clone()) {
                    Ok(keys) if keys.len() > MOST_DELETED => {
                        return error(
                            StatusCode::BAD_REQUEST,
                            &format!("at most {MOST_DELETED} games can be restored at once"),
                        );
                    }
                    Ok(keys) => Some(keys),
                    Err(_) => {
                        return error(
                            StatusCode::BAD_REQUEST,
                            "games must list the removed games to restore",
                        );
                    }
                }
            };
            tokio::task::spawn_blocking(move || crate::library::restore(&h, keys.as_deref())).await
        }
        _ => return error(StatusCode::BAD_REQUEST, "unknown API endpoint"),
    };
    match task {
        Ok(Ok(value)) => Json(value).into_response(),
        Ok(Err(err)) => error(StatusCode::BAD_REQUEST, &format!("{err:#}")),
        Err(err) => error(StatusCode::INTERNAL_SERVER_ERROR, &err.to_string()),
    }
}

/// The apps, each whose launcher game the last scan read and did not find marked
/// `not-installed`, so the console can offer to remove it.
fn apps_with_install_state(h: &Shared) -> anyhow::Result<Value> {
    let scan = crate::library::installed(h);
    let apps = h.apps.read().unwrap();
    let mut values = Vec::with_capacity(apps.len());
    for app in apps.iter() {
        let mut value = serde_json::to_value(app)?;
        if scan
            .as_ref()
            .and_then(|scan| butterpollo_core::native::installed(app, scan))
            == Some(false)
        {
            value[butterpollo_core::native::NOT_INSTALLED] = json!(true);
        }
        values.push(value);
    }
    Ok(Value::Array(values))
}

#[cfg(test)]
mod tests {
    use super::handle;
    use crate::state::test_support::Fixture;
    use serde_json::json;

    #[test]
    fn apps_list_marks_only_known_uninstalled_launcher_games_and_never_stores_it() {
        let f = Fixture::new();
        for app in [
            json!({"name": "Dota 2", "uuid": "dota", "steam-id": "570"}),
            json!({"name": "Gone", "uuid": "gone", "steam-id": "999"}),
            json!({"name": "Fortnite", "uuid": "fn", "launcher-source": "epic", "launcher-id": "x"}),
            json!({"name": "Tool", "uuid": "tool", "steam-id": "1007"}),
        ] {
            handle(&f.host, "POST", "/api/apps", &app).unwrap();
        }
        crate::library::seed(
            &f.host,
            butterpollo_core::native::Scan::new(["steam:570".into()])
                .adding(["steam:1007".into()])
                .skipping([butterpollo_core::library::Source::Epic]),
        );
        let list = handle(&f.host, "GET", "/api/apps", &json!({})).unwrap();
        let flagged: Vec<&str> = list["apps"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|app| app["not-installed"] == json!(true))
            .map(|app| app["name"].as_str().unwrap())
            .collect();
        assert_eq!(flagged, ["Gone"]);
        let mut gone = list["apps"]
            .as_array()
            .unwrap()
            .iter()
            .find(|app| app["name"] == "Gone")
            .unwrap()
            .clone();
        gone["name"] = json!("Gone (renamed)");
        handle(&f.host, "POST", "/api/apps", &gone).unwrap();
        let stored = f.host.apps.read().unwrap();
        let renamed = stored
            .iter()
            .find(|app| app.name == "Gone (renamed)")
            .unwrap();
        assert!(!renamed.extra.contains_key("not-installed"));
    }

    #[test]
    fn an_app_cannot_save_a_preset_the_host_does_not_know() {
        let f = Fixture::new();
        let save = |overrides: serde_json::Value| {
            handle(
                &f.host,
                "POST",
                "/api/apps",
                &json!({"name": "Game", "uuid": "preset-app", "config-overrides": overrides}),
            )
        };
        let stored = || {
            f.host
                .apps
                .read()
                .unwrap()
                .iter()
                .find(|app| app.name == "Game")
                .map(|app| app.extra["config-overrides"].clone())
        };
        save(json!({"stream_preset": "reference"})).unwrap();
        let good = stored();
        assert_eq!(good, Some(json!({"stream_preset": "reference"})));
        for bad in [
            json!("fastest"),
            json!(3),
            json!(true),
            json!(["reference"]),
        ] {
            let error = save(json!({"stream_preset": bad})).unwrap_err();
            assert!(error.to_string().contains("stream_preset"), "{error}");
            assert_eq!(stored(), good, "{bad}");
        }
        save(json!({"stream_preset": ""})).unwrap();
    }

    #[test]
    fn an_app_saves_a_radeon_profile_the_host_can_apply_and_refuses_others() {
        let f = Fixture::new();
        let save = |extra: serde_json::Value| {
            let mut app = json!({"name": "Game", "uuid": "radeon-app"});
            app.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            handle(&f.host, "POST", "/api/apps", &app)
        };
        let stored = || {
            f.host
                .apps
                .read()
                .unwrap()
                .iter()
                .find(|app| app.name == "Game")
                .map(|app| {
                    (
                        app.extra.get("radeon-profile").cloned(),
                        app.extra.get("radeon-settings").cloned(),
                    )
                })
        };
        save(json!({"radeon-profile": "smoothness"})).unwrap();
        assert_eq!(stored(), Some((Some(json!("smoothness")), None)));
        save(json!({
            "radeon-profile": "custom",
            "radeon-settings": {"chill.enabled": true, "chill.max_fps": 90}
        }))
        .unwrap();
        let good = stored();
        assert_eq!(
            good.as_ref().unwrap().1,
            Some(json!({"chill.enabled": true, "chill.max_fps": 90}))
        );
        for (bad, why) in [
            (json!({"radeon-profile": "fastest"}), "radeon-profile"),
            (json!({"radeon-profile": "custom"}), "at least one"),
            (
                json!({"radeon-profile": "custom", "radeon-settings": {"x": 1}}),
                "not a Radeon setting id",
            ),
            (
                json!({"radeon-profile": "quality", "radeon-settings": {"a.b": 1}}),
                "only with the custom",
            ),
        ] {
            let error = save(bad.clone()).unwrap_err();
            assert!(error.to_string().contains(why), "{bad}: {error}");
            assert_eq!(stored(), good, "a refused save changes nothing: {bad}");
        }
        // An app with none saves as before.
        save(json!({})).unwrap();
        assert_eq!(stored(), Some((None, None)));
    }

    #[test]
    fn an_app_saves_per_device_settings_and_game_files_and_refuses_what_a_launch_cannot_use() {
        let f = Fixture::new();
        let save = |extra: serde_json::Value| {
            let mut app = json!({"name": "Game", "uuid": "device-app"});
            app.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            handle(&f.host, "POST", "/api/apps", &app)
        };
        let good = json!({
            "game-files": [{"path": "OptiScaler.ini", "section": "Framerate", "key": "FramerateLimit", "value": "{stream_fps-1}"}],
            "device-overrides": {
                "phone": {
                    "config-overrides": {"stream_preset": "reference", "frame_limiter_fps_limit": 60},
                    "radeon-profile": "smoothness",
                    "game-files": [{"path": "OptiScaler.ini", "section": "Framerate", "key": "FramerateLimit", "value": 60}]
                },
                // A device that is not paired keeps its block.
                "gone": {"config-overrides": {"max_bitrate": 20000}}
            }
        });
        save(good.clone()).unwrap();
        let saved = || stored(&f, "device-app").unwrap();
        assert_eq!(saved()["device-overrides"], good["device-overrides"]);
        for (bad, why) in [
            (
                json!({"device-overrides": {"phone": {"config-overrides": {"port": 50000}}}}),
                "port",
            ),
            (
                json!({"device-overrides": {"phone": {"config-overrides": {"stream_preset": "fastest"}}}}),
                "stream_preset",
            ),
            (
                json!({"device-overrides": {"phone": {"radeon-profile": "fastest"}}}),
                "radeon-profile",
            ),
            (
                json!({"device-overrides": {"phone": {"name": "x"}}}),
                "name",
            ),
            (
                json!({"game-files": [{"path": "..\\other\\x.ini", "section": "A", "key": "B", "value": 1}]}),
                "..",
            ),
            (
                json!({"game-files": [{"path": "settings.cfg", "section": "A", "key": "B", "value": 1}]}),
                ".ini",
            ),
            (
                json!({"game-files": [{"path": "a.ini", "section": "A", "key": "B", "value": "{base_fps}"}]}),
                "{stream_fps}",
            ),
        ] {
            let error = format!("{:#}", save(bad.clone()).unwrap_err());
            assert!(error.contains(why), "{bad}: {error}");
            assert_eq!(
                saved()["device-overrides"],
                good["device-overrides"],
                "a refused save changes nothing: {bad}"
            );
        }
    }

    fn stored(f: &Fixture, uuid: &str) -> Option<serde_json::Value> {
        f.host
            .apps
            .read()
            .unwrap()
            .iter()
            .find(|app| app.extra.get("uuid").and_then(|u| u.as_str()) == Some(uuid))
            .map(|app| serde_json::to_value(app).unwrap())
    }

    #[test]
    fn a_synced_app_saved_in_the_console_becomes_the_users_and_survives_uninstall() {
        use butterpollo_core::steam;
        let f = Fixture::new();
        let uuid = steam::canonical_uuid(620);
        crate::steam::update_apps(&f.host, |apps| {
            apps.push(json!({"name": "Portal 2", "uuid": uuid, "steam-id": "620",
                "steam-managed": "auto", "synced-by": "steam", "cmd": steam::launch_command(620)}));
            true
        })
        .unwrap();
        // The editor sends the app back with the user's name, without the stamp.
        handle(
            &f.host,
            "POST",
            "/api/apps",
            &json!({"name": "My Portal", "uuid": uuid, "steam-id": "620",
                "steam-managed": "auto", "cmd": steam::launch_command(620)}),
        )
        .unwrap();
        let app = stored(&f, &uuid).unwrap();
        assert_eq!(app["name"], "My Portal");
        assert_eq!(app["steam-managed"], "manual");
        assert_eq!(app.get("synced-by"), None);
        // Uninstalled, with a sync that removes missing games: it stays as saved.
        let settings = steam::Settings::from_config(
            &butterpollo_core::config::Config::parse(
                "steam_sync_all_installed=true\nsteam_autosync_remove_uninstalled=true",
            )
            .unwrap(),
        );
        let changed = crate::steam::update_apps(&f.host, |apps| {
            steam::reconcile(apps, &[], &settings, &Default::default(), &[])
        })
        .unwrap();
        assert!(!changed);
        assert_eq!(stored(&f, &uuid).unwrap(), app);
        // Only a sync stamps an app: a stamp sent with one is dropped.
        handle(
            &f.host,
            "POST",
            "/api/apps",
            &json!({"name": "Forged", "uuid": "forged", "synced-by": "steam",
                "steam-id": "440", "steam-managed": "auto"}),
        )
        .unwrap();
        let forged = stored(&f, "forged").unwrap();
        assert_eq!(forged.get("synced-by"), None);
        assert_eq!(forged["steam-managed"], "auto");
    }

    #[test]
    fn deleting_any_app_that_starts_a_steam_game_dismisses_the_game() {
        let f = Fixture::new();
        handle(
            &f.host,
            "POST",
            "/api/apps",
            &json!({"name": "Mine", "uuid": "mine", "cmd": "cmd /c start steam://rungameid/620"}),
        )
        .unwrap();
        handle(
            &f.host,
            "POST",
            "/api/apps/delete",
            &json!({"uuid": "mine"}),
        )
        .unwrap();
        assert!(stored(&f, "mine").is_none());
        assert_eq!(
            f.host.app_document.read().unwrap()["launcher-dismissed"],
            json!(["steam:620"])
        );
        // An app without a launcher game dismisses nothing.
        handle(
            &f.host,
            "POST",
            "/api/apps",
            &json!({"name": "Tool", "uuid": "tool"}),
        )
        .unwrap();
        handle(
            &f.host,
            "POST",
            "/api/apps/delete",
            &json!({"uuid": "tool"}),
        )
        .unwrap();
        assert_eq!(
            f.host.app_document.read().unwrap()["launcher-dismissed"],
            json!(["steam:620"])
        );
    }

    #[test]
    fn removing_several_apps_at_once_keeps_every_synced_game_out_in_one_save() {
        let f = Fixture::new();
        for app in [
            json!({"name": "Portal 2", "uuid": "portal", "steam-id": "620"}),
            json!({"name": "Mirage", "uuid": "mirage", "launcher-source": "ubisoft",
                "launcher-id": "13504"}),
            json!({"name": "Kept", "uuid": "kept", "launcher-source": "epic", "launcher-id": "k"}),
        ] {
            handle(&f.host, "POST", "/api/apps", &app).unwrap();
        }
        // A uuid already gone (a sync dropped its game meanwhile) is skipped, not an error.
        let removed = handle(
            &f.host,
            "POST",
            "/api/apps/delete",
            &json!({"uuids": ["portal", "mirage", "gone"]}),
        )
        .unwrap();
        assert_eq!(removed["deleted"], 2);
        assert!(stored(&f, "portal").is_none() && stored(&f, "mirage").is_none());
        assert!(stored(&f, "kept").is_some());
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&f.host.apps_path).unwrap()).unwrap();
        assert_eq!(
            saved["launcher-dismissed"],
            json!(["steam:620", "ubisoft:13504"])
        );
        let uuids: Vec<&str> = saved["apps"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|app| app["uuid"].as_str())
            .collect();
        assert!(uuids.contains(&"kept"));
        assert!(!uuids.contains(&"portal") && !uuids.contains(&"mirage"));
        // Nothing to remove, nothing found, or a malformed list is refused and changes nothing.
        for bad in [
            json!({"uuids": []}),
            json!({"uuids": ["gone"]}),
            json!({"uuids": "kept"}),
            json!({"uuids": ["kept", 7]}),
        ] {
            assert!(
                handle(&f.host, "POST", "/api/apps/delete", &bad).is_err(),
                "{bad}"
            );
        }
        assert!(stored(&f, "kept").is_some());
    }

    #[test]
    fn a_busy_or_failed_neural_rendering_removal_keeps_its_app_and_touches_nothing_early() {
        use std::cell::Cell;
        let f = Fixture::new();
        for app in [
            json!({"name": "Portal 2", "uuid": "portal", "steam-id": "620"}),
            json!({"name": "Mirage", "uuid": "mirage", "launcher-source": "ubisoft",
                "launcher-id": "13504"}),
            json!({"name": "Fortnite", "uuid": "fortnite", "launcher-source": "epic",
                "launcher-id": "f"}),
            json!({"name": "Installing", "uuid": "busy", "launcher-source": "gog",
                "launcher-id": "b"}),
        ] {
            handle(&f.host, "POST", "/api/apps", &app).unwrap();
        }
        let removals = Cell::new(0);
        let count = |_: &crate::neural::Removing, _: &butterpollo_core::state::App| {
            removals.set(removals.get() + 1);
            Ok(())
        };
        // An install still running: refused before any app's mod is taken out.
        crate::neural::start_job(&f.host, "busy", "install", "download").unwrap();
        let busy = super::delete_apps_with(&f.host, &["portal", "busy"], count).unwrap_err();
        assert!(
            format!("{busy:#}").contains("busy with Installing"),
            "{busy:#}"
        );
        assert_eq!(removals.get(), 0);
        assert!(stored(&f, "portal").is_some() && stored(&f, "busy").is_some());
        // Mirage's mod stays in its folder partway through: Mirage stays, the others go.
        let deleted =
            super::delete_apps_with(&f.host, &["portal", "mirage", "fortnite"], |_, app| {
                if app.name == "Mirage" {
                    anyhow::bail!("a file is in use");
                }
                Ok(())
            })
            .unwrap();
        assert_eq!(deleted.count, 2);
        assert_eq!(
            deleted.kept,
            [json!({"name": "Mirage", "error": "a file is in use"})]
        );
        assert!(stored(&f, "mirage").is_some());
        assert!(stored(&f, "portal").is_none() && stored(&f, "fortnite").is_none());
        assert_eq!(
            f.host.app_document.read().unwrap()["launcher-dismissed"],
            json!(["steam:620", "epic:f"])
        );
        // Every removal failing is the error itself, and the app stays.
        let failed =
            super::delete_apps_with(&f.host, &["mirage"], |_, _| anyhow::bail!("still in use"))
                .unwrap_err();
        assert_eq!(failed.to_string(), "still in use");
        assert!(stored(&f, "mirage").is_some());
    }

    /// Purging synced apps waits on a worker, so the web server answers
    /// meanwhile: a game it drops loses Neural Rendering through a helper
    /// process, which can take minutes.
    #[tokio::test(flavor = "current_thread")]
    async fn purging_synced_apps_never_blocks_the_web_server() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let f = Fixture::new();
        let h = f.host.clone();
        // The apps held elsewhere for a while, as a slow removal holds the purge.
        let (held, holding) = std::sync::mpsc::channel();
        let reader = h.clone();
        let holder = std::thread::spawn(move || {
            let _apps = reader.apps.read().unwrap();
            held.send(()).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(500));
        });
        holding.recv().unwrap();
        let answered = std::sync::Arc::new(AtomicBool::new(false));
        let other = answered.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            other.store(true, Ordering::SeqCst);
        });
        let response = super::purge_autosync(&h).await;
        assert!(
            answered.load(Ordering::SeqCst),
            "nothing else ran during the purge"
        );
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        holder.join().unwrap();
    }
}
