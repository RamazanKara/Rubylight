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

/// Adds the hook for `url`, or edits the one at `previous`, and returns the
/// secret when it is new.
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
                hook.url = url.to_owned();
                hook.events = events;
                if rotate {
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
mod tests {
    use super::super::parity_tests::{request, value};
    use crate::state::test_support::Fixture;
    use axum::http::StatusCode;
    use butterpollo_core::config::Config;
    use serde_json::{Value, json};

    async fn post(f: &Fixture, path: &str, data: Value) -> (StatusCode, Value) {
        let response = request(f, "POST", path, data).await;
        let status = response.status();
        (status, value(response, status).await)
    }

    async fn list(f: &Fixture) -> Value {
        value(
            request(f, "GET", "/api/webhooks", Value::Null).await,
            StatusCode::OK,
        )
        .await
    }

    fn stored(f: &Fixture) -> Vec<butterpollo_core::webhook::Webhook> {
        let saved = Config::load(&f.host.config_path).unwrap();
        butterpollo_core::webhook::parse_webhooks(saved.get("webhooks", "[]")).unwrap()
    }

    #[tokio::test]
    async fn a_hook_is_added_edited_and_deleted_and_its_secret_shown_once() {
        let f = Fixture::new();
        let url = "https://example.com/hook?token=t";
        let (status, added) = post(
            &f,
            "/api/webhooks",
            json!({"url": url, "events": ["app.stopped", "app.started", "app.started"]}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{added}");
        let secret = added["secret"].as_str().unwrap().to_owned();
        assert_eq!(secret.len(), 64);
        assert!(secret.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(stored(&f)[0].secret, secret);
        assert_eq!(f.host.config.read().unwrap().webhooks()[0].secret, secret);

        let listed = list(&f).await;
        assert_eq!(
            listed["hooks"],
            json!([{"url": url, "events": ["app.started", "app.stopped"], "last": null}])
        );
        assert_eq!(listed["max"], crate::webhooks::MAX_HOOKS);
        assert_eq!(listed["events"].as_array().unwrap().len(), 6);
        assert!(!listed.to_string().contains(&secret));
        // Nor through the settings API, which would show it as a custom key.
        let config = value(
            request(&f, "GET", "/api/config", Value::Null).await,
            StatusCode::OK,
        )
        .await;
        assert!(config.get("webhooks").is_none());
        assert!(!config.to_string().contains(&secret));

        // An edit keeps the secret unless asked for a new one.
        let moved = "https://example.com/other";
        let (status, edited) = post(
            &f,
            "/api/webhooks",
            json!({"url": moved, "events": ["pairing.paired"], "previous": url}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{edited}");
        assert_eq!(edited["secret"], Value::Null);
        assert_eq!(stored(&f)[0].secret, secret);
        assert_eq!(stored(&f)[0].url, moved);
        let (_, rotated) = post(
            &f,
            "/api/webhooks",
            json!({"url": moved, "events": ["pairing.paired"], "previous": moved, "rotate_secret": true}),
        )
        .await;
        let replaced = rotated["secret"].as_str().unwrap();
        assert_ne!(replaced, secret);
        assert_eq!(stored(&f)[0].secret, replaced);

        let (status, _) = post(&f, "/api/webhooks/delete", json!({"url": moved})).await;
        assert_eq!(status, StatusCode::OK);
        assert!(stored(&f).is_empty());
        assert_eq!(list(&f).await["hooks"], json!([]));
        let (status, _) = post(&f, "/api/webhooks/delete", json!({"url": moved})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn bad_hooks_are_refused_and_save_nothing() {
        let f = Fixture::new();
        let url = "https://example.com/hook";
        post(
            &f,
            "/api/webhooks",
            json!({"url": url, "events": ["app.started"]}),
        )
        .await;
        for (data, message) in [
            (json!({"url": url, "events": ["app.started"]}), "already"),
            (
                json!({"url": "https://b.example/", "events": ["app.crashed"]}),
                "unknown event",
            ),
            (
                json!({"url": "https://b.example/", "events": []}),
                "no events",
            ),
            (
                json!({"url": "ftp://b.example/", "events": ["app.started"]}),
                "http",
            ),
            (
                json!({"url": "b.example", "events": ["app.started"]}),
                "not a URL",
            ),
            (
                json!({"url": "https://b.example/", "events": ["app.started"], "previous": "https://c.example/"}),
                "no webhook",
            ),
        ] {
            let (status, answer) = post(&f, "/api/webhooks", data.clone()).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{data}");
            assert!(
                answer["error"].as_str().unwrap().contains(message),
                "{data}: {answer}"
            );
        }
        assert_eq!(stored(&f).len(), 1);

        // The settings API never writes them, which would lose the secrets.
        let (status, answer) =
            post(&f, "/api/config", json!({"webhooks": "[]", "locale": "de"})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(answer["error"].as_str().unwrap().contains("Webhooks page"));
        assert_eq!(stored(&f).len(), 1);

        for n in 1..crate::webhooks::MAX_HOOKS {
            let (status, answer) = post(
                &f,
                "/api/webhooks",
                json!({"url": format!("https://example.com/{n}"), "events": ["app.started"]}),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{answer}");
        }
        let (status, answer) = post(
            &f,
            "/api/webhooks",
            json!({"url": "https://example.com/last", "events": ["app.started"]}),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(answer["error"].as_str().unwrap().contains("at most"));
    }

    #[tokio::test]
    async fn a_broken_saved_list_is_reported_and_never_overwritten() {
        let f = Fixture::new();
        let broken = "[{\"url\": \"https://example.com/\"}]";
        f.host
            .config
            .write()
            .unwrap()
            .values
            .insert("webhooks".into(), broken.into());
        let listed = list(&f).await;
        assert_eq!(listed["hooks"], json!([]));
        assert!(
            listed["error"]
                .as_str()
                .unwrap()
                .contains("not a JSON list")
        );
        let (status, answer) = post(
            &f,
            "/api/webhooks",
            json!({"url": "https://b.example/", "events": ["app.started"]}),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(answer["error"].as_str().unwrap().contains("broken"));
        assert_eq!(f.host.config.read().unwrap().get("webhooks", ""), broken);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn send_test_reaches_the_receiver_and_shows_as_the_last_status() {
        let (url, mut received) = crate::webhooks::tests::receiver(vec![200]).await;
        let f = Fixture::new();
        let (_, added) = post(
            &f,
            "/api/webhooks",
            json!({"url": url, "events": ["app.started"]}),
        )
        .await;
        let (status, answer) = post(&f, "/api/webhooks/test", json!({"url": url})).await;
        assert_eq!(status, StatusCode::OK, "{answer}");
        assert_eq!(answer["result"]["ok"], true, "{answer}");
        assert_eq!(answer["result"]["event"], "webhook.test");
        let request = received.recv().await.unwrap();
        assert_eq!(request.headers["x-rubylight-event"], "webhook.test");
        let signature =
            butterpollo_core::webhook::sign(added["secret"].as_str().unwrap(), &request.body);
        assert_eq!(
            request.headers["x-rubylight-signature"],
            format!("sha256={signature}")
        );
        assert_eq!(list(&f).await["hooks"][0]["last"]["event"], "webhook.test");

        let (status, _) = post(
            &f,
            "/api/webhooks/test",
            json!({"url": "https://elsewhere.example/"}),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
}
