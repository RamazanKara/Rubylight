//! Scoped API token catalog and token issuance/revocation handlers.
use butterpollo_crypto::auth;
use butterpollo_session::state;
use butterpollo_stream::state::Shared;
use serde_json::{Value, json};

pub(super) fn catalog() -> Vec<auth::Scope> {
    [
        ("/api/config", &["GET", "POST", "PATCH"][..]),
        ("/api/configLocale", &["GET"][..]),
        ("/api/meta", &["GET"][..]),
        ("/api/metadata", &["GET"][..]),
        ("/api/warnings/dismiss", &["POST"][..]),
        ("/api/apps", &["GET", "POST"][..]),
        ("/api/apps/[^/]+", &["DELETE"][..]),
        ("/api/apps/delete", &["POST"][..]),
        ("/api/apps/[^/]+/cover", &["GET"][..]),
        ("/api/apps/[^/]+/icon", &["GET"][..]),
        ("/api/apps/close", &["POST"][..]),
        ("/api/apps/launch", &["POST"][..]),
        ("/api/apps/reorder", &["POST"][..]),
        ("/api/clients/list", &["GET"][..]),
        ("/api/clients/update", &["POST"][..]),
        ("/api/clients/unpair", &["POST"][..]),
        ("/api/clients/disconnect", &["POST"][..]),
        ("/api/clients/reset-network-history", &["POST"][..]),
        ("/api/usb", &["GET"][..]),
        ("/api/usb/detach", &["POST"][..]),
        ("/api/session/status", &["GET"][..]),
        ("/api/session/console", &["GET", "POST"][..]),
        ("/api/session/suspend", &["GET", "POST"][..]),
        ("/api/session/resume", &["POST"][..]),
        ("/api/sessions/history", &["GET"][..]),
        ("/api/rtsp/sessions", &["GET"][..]),
        ("/api/display-devices", &["GET"][..]),
        ("/api/framegen/edid-refresh", &["GET"][..]),
        ("/api/clients/display-layout", &["GET", "PUT"][..]),
        ("/api/clients/hdr-profiles", &["GET"][..]),
        ("/api/clients/unpair-all", &["POST"][..]),
        ("/api/frame-limiter/status", &["GET"][..]),
        ("/api/internet-play", &["GET"][..]),
        ("/api/rtss/status", &["GET"][..]),
        ("/api/health/vulkan-hdr-layer", &["GET"][..]),
        ("/api/health/vulkan-hdr-layer/register", &["POST"][..]),
        ("/api/health/crashdump", &["GET"][..]),
        ("/api/health/crashdump/dismiss", &["POST"][..]),
        ("/api/display/golden_status", &["GET"][..]),
        ("/api/display/export_golden", &["POST"][..]),
        ("/api/display/restore_golden", &["POST"][..]),
        ("/api/display/golden", &["DELETE"][..]),
        ("/api/display/terminate_virtual", &["POST"][..]),
        ("/api/reset-display-device-persistence", &["POST"][..]),
        ("/api/updates", &["GET"][..]),
        ("/api/updates/check", &["POST"][..]),
        ("/api/updates/install", &["POST"][..]),
        ("/api/updates/install_now", &["POST"][..]),
        ("/api/updates/cancel", &["POST"][..]),
        ("/api/covers/upload", &["POST"][..]),
        ("/api/covers/[0-9]+", &["GET"][..]),
        ("/api/logs", &["GET"][..]),
        ("/api/logs/export", &["GET"][..]),
        ("/api/logs/export_crash", &["GET"][..]),
        ("/api/logs/export_crash/manifest", &["GET"][..]),
        ("/api/pin", &["POST"][..]),
        ("/api/otp", &["POST"][..]),
        ("/api/pairing/qr", &["POST"][..]),
        ("/api/webhooks", &["GET", "POST"][..]),
        ("/api/webhooks/delete", &["POST"][..]),
        ("/api/webhooks/test", &["POST"][..]),
        ("/api/clients/pending", &["GET"][..]),
        ("/api/logs/tail", &["GET"][..]),
        ("/api/logs/analysis", &["GET"][..]),
        ("/api/library/discovery", &["GET"][..]),
        ("/api/library/import", &["POST"][..]),
        ("/api/library/sync", &["POST"][..]),
        ("/api/library/restore", &["POST"][..]),
        ("/api/steam/status", &["GET"][..]),
        ("/api/steam/games", &["GET"][..]),
        ("/api/steam/force_sync", &["POST"][..]),
        ("/api/steam/launch", &["POST"][..]),
        ("/api/playnite/status", &["GET"][..]),
        ("/api/playnite/games", &["GET"][..]),
        ("/api/playnite/categories", &["GET"][..]),
        ("/api/playnite/install", &["POST"][..]),
        ("/api/playnite/uninstall", &["POST"][..]),
        ("/api/playnite/force_sync", &["POST"][..]),
        ("/api/playnite/cover", &["POST"][..]),
        ("/api/playnite/launch", &["POST"][..]),
        ("/api/apps/purge_autosync", &["POST"][..]),
        ("/api/lossless_scaling/status", &["GET"][..]),
        ("/api/radeon", &["GET"][..]),
        ("/api/restart", &["POST"][..]),
        ("/api/quit", &["POST"][..]),
    ]
    .into_iter()
    .map(|(path, methods)| auth::Scope {
        path: path.into(),
        methods: methods.iter().map(|m| (*m).into()).collect(),
    })
    .collect()
}

pub(super) fn list(h: &Shared) -> anyhow::Result<Value> {
    Ok(json!({"status":true,"tokens":auth::read(&h.aliases.lock().unwrap())?}))
}

pub(super) fn issue(h: &Shared, data: &Value) -> anyhow::Result<Value> {
    let scopes: Vec<auth::Scope> =
        serde_json::from_value(data.get("scopes").cloned().unwrap_or(Value::Null))?;
    let username = h
        .credentials
        .read()
        .unwrap()
        .as_ref()
        .map(|c| c.username.clone())
        .ok_or_else(|| anyhow::anyhow!("credentials required"))?;
    let (secret, token) = auth::issue(username, scopes, &super::token_catalog())?;
    let mut document = h.aliases.lock().unwrap();
    let mut next = document.clone();
    let mut tokens = auth::read(&next)?;
    if tokens.len() >= 256 {
        anyhow::bail!("API token limit reached");
    }
    tokens.push(token);
    next["root"]["api_tokens"] = serde_json::to_value(tokens)?;
    state::write_json(&h.aliases_path, &next)?;
    *document = next;
    Ok(json!({"status":true,"token":secret}))
}

pub(super) fn revoke(h: &Shared, hash: &str) -> anyhow::Result<Value> {
    let mut document = h.aliases.lock().unwrap();
    let mut next = document.clone();
    let mut tokens = auth::read(&next)?;
    tokens.retain(|t| !t.hash.eq_ignore_ascii_case(hash));
    next["root"]["api_tokens"] = serde_json::to_value(tokens)?;
    state::write_json(&h.aliases_path, &next)?;
    *document = next;
    Ok(json!({"status":true}))
}
