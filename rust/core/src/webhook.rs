//! Webhooks: the host POSTs JSON to the URLs of the `webhooks` setting when
//! something happens (the host's `webhooks` module sends them; `docs/webhooks.md`
//! is the receiver's guide).
//!
//! Each delivery is [`Delivery`] as JSON with the headers [`HEADER_EVENT`],
//! [`HEADER_DELIVERY`], [`HEADER_TIMESTAMP`] and [`HEADER_SIGNATURE`], whose
//! value is [`SIGNATURE_PREFIX`] and [`sign`] of the body with the hook's
//! secret, so a receiver can check the host sent it. The signed body carries
//! the delivery id and time, so a receiver refuses replays by both:
//! [`REPLAY_WINDOW_MS`].
use anyhow::{Context, Result, bail};
use ring::hmac;
use serde::{Deserialize, Serialize};

/// The events a hook may subscribe to.
pub const EVENTS: &[&str] = &[
    "pairing.paired",
    "pairing.unpaired",
    "app.started",
    "app.stopped",
    "session.started",
    "session.ended",
];
pub const HEADER_EVENT: &str = "X-Rubylight-Event";
/// A UUID per delivery, the same as [`Delivery::delivery`].
pub const HEADER_DELIVERY: &str = "X-Rubylight-Delivery";
pub const HEADER_SIGNATURE: &str = "X-Rubylight-Signature";
/// [`Delivery::time_unix_ms`] again, in decimal. The signature covers the
/// body's copy, which is the one to check.
pub const HEADER_TIMESTAMP: &str = "X-Rubylight-Timestamp";
/// The event of the console's "Send test", sent to one hook whatever it
/// subscribes to. Not one of [`EVENTS`].
pub const TEST_EVENT: &str = "webhook.test";
/// How far a delivery's time may be from the receiver's clock before it
/// refuses it as a replay; retries resend the same body well within it.
pub const REPLAY_WINDOW_MS: u64 = 5 * 60 * 1000;
/// Before the hex digest in [`HEADER_SIGNATURE`].
pub const SIGNATURE_PREFIX: &str = "sha256=";

/// One entry of the `webhooks` setting.
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Webhook {
    /// An `http://` or `https://` URL.
    pub url: String,
    /// Some of [`EVENTS`].
    pub events: Vec<String>,
    /// The HMAC key; never logged.
    pub secret: String,
}

impl std::fmt::Debug for Webhook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Webhook")
            .field("url", &self.url)
            .field("events", &self.events)
            .field("secret", &"<redacted>")
            .finish()
    }
}

/// The body of one delivery.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Delivery<'a> {
    pub event: &'a str,
    pub delivery: &'a str,
    pub time_unix_ms: u64,
    pub host: DeliveryHost<'a>,
    pub data: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DeliveryHost<'a> {
    pub name: &'a str,
    pub unique_id: &'a str,
}

/// Reads the `webhooks` setting: a JSON list of [`Webhook`]s. An unknown
/// event, a URL that is not http(s), an empty event list or an empty secret
/// refuses the whole list.
pub fn parse_webhooks(text: &str) -> Result<Vec<Webhook>> {
    let hooks: Vec<Webhook> =
        serde_json::from_str(text).context("webhooks is not a JSON list of hooks")?;
    for hook in &hooks {
        let rest = hook
            .url
            .strip_prefix("https://")
            .or_else(|| hook.url.strip_prefix("http://"));
        if rest.is_none_or(|rest| rest.is_empty() || rest.chars().any(char::is_whitespace)) {
            bail!("webhook URL {:?} is not an http(s) URL", hook.url);
        }
        if hook.events.is_empty() {
            bail!("webhook {} subscribes to no events", hook.url);
        }
        if let Some(event) = hook.events.iter().find(|e| !EVENTS.contains(&e.as_str())) {
            bail!("webhook {} names an unknown event {event:?}", hook.url);
        }
        if hook.secret.is_empty() {
            bail!("webhook {} has no secret", hook.url);
        }
    }
    Ok(hooks)
}

/// Lowercase hex HMAC-SHA256 of `body` with `secret`.
pub fn sign(secret: &str, body: &[u8]) -> String {
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
    hex::encode(hmac::sign(&key, body).as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signing_matches_rfc_4231_case_2() {
        assert_eq!(
            sign("Jefe", b"what do ya want for nothing?"),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn a_valid_list_parses_and_hides_its_secret() {
        let hooks = parse_webhooks(
            r#"[{"url":"https://example.test/hook","events":["app.started","session.ended"],"secret":"s3cret"},
                {"url":"http://192.168.1.5:8080/","events":["pairing.paired"],"secret":"x"}]"#,
        )
        .unwrap();
        assert_eq!(hooks.len(), 2);
        assert_eq!(hooks[0].events, ["app.started", "session.ended"]);
        assert!(!format!("{hooks:?}").contains("s3cret"));
        assert!(parse_webhooks("[]").unwrap().is_empty());
    }

    #[test]
    fn a_bad_hook_refuses_the_list() {
        for bad in [
            r#"[{"url":"https://a.test","events":["app.crashed"],"secret":"s"}]"#,
            r#"[{"url":"ftp://a.test","events":["app.started"],"secret":"s"}]"#,
            r#"[{"url":"https://","events":["app.started"],"secret":"s"}]"#,
            r#"[{"url":"https://a test","events":["app.started"],"secret":"s"}]"#,
            r#"[{"url":"a.test","events":["app.started"],"secret":"s"}]"#,
            r#"[{"url":"https://a.test","events":[],"secret":"s"}]"#,
            r#"[{"url":"https://a.test","events":["app.started"],"secret":""}]"#,
            r#"[{"url":"https://a.test","events":["app.started"]}]"#,
            r#"[{"url":"https://a.test","events":["app.started"],"secret":"s","extra":1}]"#,
            r#"{"url":"https://a.test"}"#,
            "not json",
        ] {
            assert!(parse_webhooks(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_delivery_has_the_documented_shape() {
        let body = Delivery {
            event: "app.started",
            delivery: "0b6f2d8a-4c11-4c52-9e10-5e0f4b1c7a3d",
            time_unix_ms: 1_792_000_000_000,
            host: DeliveryHost {
                name: "GAMING-PC",
                unique_id: "5E0F4B1C",
            },
            data: serde_json::json!({"app": "Desktop"}),
        };
        assert_eq!(
            serde_json::to_value(&body).unwrap(),
            serde_json::json!({
                "event": "app.started",
                "delivery": "0b6f2d8a-4c11-4c52-9e10-5e0f4b1c7a3d",
                "time_unix_ms": 1_792_000_000_000u64,
                "host": {"name": "GAMING-PC", "unique_id": "5E0F4B1C"},
                "data": {"app": "Desktop"},
            })
        );
    }
}
