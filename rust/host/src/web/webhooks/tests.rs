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
    assert_eq!(listed["events"].as_array().unwrap().len(), 9);
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

    // Moved to another host, it gets a new secret: the old one is the old receiver's.
    let elsewhere = "http://192.168.1.20:8123/api/webhook/rubylight";
    let (status, edited) = post(
        &f,
        "/api/webhooks",
        json!({"url": elsewhere, "events": ["pairing.paired"], "previous": moved}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{edited}");
    let fresh = edited["secret"].as_str().unwrap();
    assert!(fresh != replaced && fresh.len() == 64, "{edited}");
    assert_eq!(stored(&f)[0].secret, fresh);
    let moved = elsewhere;

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

    // This PC and link-local addresses: their services may trust whoever reaches them.
    for refused in [
        "http://127.0.0.1:47990/api/apps",
        "http://127.255.0.9/",
        "http://localhost:8080/hook",
        "http://LocalHost./hook",
        "http://api.localhost/hook",
        "http://[::1]:47990/",
        "http://[::ffff:127.0.0.1]/",
        "http://0.0.0.0:47990/",
        "http://169.254.169.254/latest/meta-data/",
        "http://[fe80::1]/",
    ] {
        let (status, answer) = post(
            &f,
            "/api/webhooks",
            json!({"url": refused, "events": ["app.started"]}),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
        let error = answer["error"].as_str().unwrap();
        assert!(
            error.contains("this PC") && error.contains("LAN"),
            "{refused}: {error}"
        );
    }
    // Nor may an edit move a hook there.
    let (status, _) = post(
        &f,
        "/api/webhooks",
        json!({"url": "http://127.0.0.1/", "events": ["app.started"], "previous": url}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(stored(&f).len(), 1);
    assert_eq!(stored(&f)[0].url, url);
    // The LAN stays open: home automation lives there.
    for allowed in [
        "http://192.168.1.5:8123/api/webhook/x",
        "http://10.0.0.2/",
        "http://homeassistant.local:8123/",
        "http://[fd00::5]/",
    ] {
        let (status, answer) = post(
            &f,
            "/api/webhooks",
            json!({"url": allowed, "events": ["app.started"]}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{allowed}: {answer}");
        post(&f, "/api/webhooks/delete", json!({"url": allowed})).await;
    }
    assert_eq!(stored(&f).len(), 1);

    // The settings API never writes them, which would lose the secrets.
    let (status, answer) = post(&f, "/api/config", json!({"webhooks": "[]", "locale": "de"})).await;
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
    // The receiver is on this PC, which the console refuses: saved as the config file would.
    let secret = "receiver secret";
    f.host.config.write().unwrap().values.insert(
        "webhooks".into(),
        json!([{"url": url, "events": ["app.started"], "secret": secret}]).to_string(),
    );
    let (status, answer) = post(&f, "/api/webhooks/test", json!({"url": url})).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["result"]["ok"], true, "{answer}");
    assert_eq!(answer["result"]["event"], "webhook.test");
    let request = received.recv().await.unwrap();
    assert_eq!(request.headers["x-rubylight-event"], "webhook.test");
    let signature = butterpollo_core::webhook::sign(secret, &request.body);
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
