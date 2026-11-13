//! The console's Webhooks page: the hooks saved in the `webhooks` setting,
//! their last delivery, and a test send. A secret leaves the host once, when
//! the hook is created or its secret is replaced.

use crate::state::Shared;
use butterpollo_core::webhook::{self, DeliveryHost, Webhook};
use serde_json::{Value, json};

pub(super) fn handle(h: &Shared, method: &str, path: &str, data: &Value) -> anyhow::Result<Value> {
    let text = |key: &str| data.get(key).and_then(Value::as_str).unwrap_or("").trim();
    Ok(match (method, path) {
        ("GET", "/api/webhooks") => list(h),
        ("POST", "/api/webhooks") => {
            let events = data
                .get("events")
                .and_then(Value::as_array)
                .ok_or_else(|| anyhow::anyhow!("events must be a list"))?
                .iter()
                .map(|e| e.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| anyhow::anyhow!("events must be names"))?;
            let previous = data.get("previous").and_then(Value::as_str).map(str::trim);
            let rotate = data.get("rotate_secret").and_then(Value::as_bool) == Some(true);
            let secret = save(h, text("url"), events, previous, rotate)?;
            json!({"status": true, "secret": secret})
        }
        ("POST", "/api/webhooks/delete") => {
            let url = text("url");
            super::config::save_setting(h, "webhooks", |stored| {
                let mut hooks = stored_hooks(stored)?;
                let before = hooks.len();
                hooks.retain(|hook| hook.url != url);
                if hooks.len() == before {
                    anyhow::bail!("no webhook sends to {}", crate::webhooks::origin(url));
                }
                Ok(serde_json::to_string(&hooks)?)
            })?;
            h.webhooks.forget(url);
            json!({"status": true})
        }
        ("POST", "/api/webhooks/test") => {
            let url = text("url");
            let hook = h
                .config
                .read()
                .unwrap()
                .webhooks()
                .into_iter()
                .find(|hook| hook.url == url)
                .ok_or_else(|| {
                    anyhow::anyhow!("no webhook sends to {}", crate::webhooks::origin(url))
                })?;
            let name = crate::network::host_name(&h.config.read().unwrap());
            let unique_id = h.paired.read().unwrap().unique_id.clone();
            // The console's API runs on a blocking thread of the host's runtime.
            let result = tokio::runtime::Handle::current().block_on(h.webhooks.test(
                &hook,
                DeliveryHost {
                    name: &name,
                    unique_id: &unique_id,
                },
            ));
            json!({"status": true, "result": result})
        }
        _ => anyhow::bail!("unknown API endpoint"),
    })
}

fn list(h: &Shared) -> Value {
    let stored = h.config.read().unwrap().get("webhooks", "[]").to_owned();
    let (hooks, error) = match webhook::parse_webhooks(&stored) {
        Ok(hooks) => (hooks, None),
        Err(error) => (Vec::new(), Some(error.to_string())),
    };
    let hooks = hooks
        .iter()
        .map(|hook| {
            json!({
                "url": hook.url,
                "events": hook.events,
                "last": h.webhooks.status(&hook.url),
            })
        })
        .collect::<Vec<_>>();
    json!({
        "status": true,
        "events": webhook::EVENTS,
        "max": crate::webhooks::MAX_HOOKS,
        "hooks": hooks,
        "error": error,
    })
}

/// The saved hooks, refusing to overwrite a value that does not parse: it
/// could hold hooks the user still wants.
fn stored_hooks(stored: Option<&str>) -> anyhow::Result<Vec<Webhook>> {
    webhook::parse_webhooks(stored.unwrap_or("[]")).map_err(|error| {
        anyhow::anyhow!(
            "the saved webhooks setting is broken ({error}): fix or remove it in the host config"
        )
    })
}

/// Whether `host` is this PC or a link-local address: services there (the
/// host's own API, a router's or a cloud's metadata page) may trust whoever
/// can reach them, and the console must not be a way in. A LAN host stays
/// allowed: home automation lives there.
fn on_this_pc_or_link(host: &url::Host<&str>) -> bool {
    match host {
        url::Host::Domain(name) => {
            let name = name.trim_end_matches('.').to_ascii_lowercase();
            name == "localhost" || name.ends_with(".localhost")
        }
        // A name is checked again where it resolves, at each delivery.
        url::Host::Ipv4(ip) => crate::webhooks::on_this_pc_or_link((*ip).into()),
        url::Host::Ipv6(ip) => crate::webhooks::on_this_pc_or_link((*ip).into()),
    }
}

/// Adds the hook for `url`, or edits the one at `previous`, and returns the
/// secret when it is new. A hook moved to another host gets a new secret:
/// the old one is the old receiver's.
fn save(
    h: &Shared,
    url: &str,
    mut events: Vec<String>,
    previous: Option<&str>,
    rotate: bool,
) -> anyhow::Result<Option<String>> {
    events.sort_unstable();
    events.dedup();
    let parsed = url::Url::parse(url).map_err(|error| anyhow::anyhow!("not a URL: {error}"))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        anyhow::bail!("a webhook URL starts with http:// or https:// and names a host");
    }
    if let Some(host) = parsed.host()
        && on_this_pc_or_link(&host)
    {
        anyhow::bail!(
            "a webhook can't send to {host}: this PC (localhost, 127.0.0.0/8, ::1) and link-local addresses (169.254.0.0/16, fe80::/10) are refused, so a hook can't reach services that trust them. Use the receiver's LAN address or name"
        );
    }
    let mut revealed = None;
    super::config::save_setting(h, "webhooks", |stored| {
        let mut hooks = stored_hooks(stored)?;
        let at = match previous {
            Some(previous) => Some(
                hooks
                    .iter()
                    .position(|hook| hook.url == previous)
                    .ok_or_else(|| {
                        anyhow::anyhow!("no webhook sends to {}", crate::webhooks::origin(previous))
                    })?,
            ),
            None => None,
        };
        if hooks
            .iter()
            .enumerate()
            .any(|(i, hook)| Some(i) != at && hook.url == url)
        {
            anyhow::bail!("a webhook already sends to this URL");
        }
        let fresh = || hex::encode(butterpollo_core::crypto::random::<32>());
        match at {
            Some(at) => {
                let hook = &mut hooks[at];
                let moved =
                    !url::Url::parse(&hook.url).is_ok_and(|before| before.host() == parsed.host());
                hook.url = url.to_owned();
                hook.events = events;
                if rotate || moved {
                    hook.secret = fresh();
                    revealed = Some(hook.secret.clone());
                }
            }
            None => {
                if hooks.len() >= crate::webhooks::MAX_HOOKS {
                    anyhow::bail!(
                        "at most {} webhooks: delete one first",
                        crate::webhooks::MAX_HOOKS
                    );
                }
                let secret = fresh();
                revealed = Some(secret.clone());
                hooks.push(Webhook {
                    url: url.to_owned(),
                    events,
                    secret,
                });
            }
        }
        let text = serde_json::to_string(&hooks)?;
        // The same checks the host applies when it reads the setting.
        webhook::parse_webhooks(&text)?;
        Ok(text)
    })?;
    if let Some(previous) = previous
        && previous != url
    {
        h.webhooks.forget(previous);
    }
    Ok(revealed)
}

#[cfg(test)]
mod tests;
