//! Sleep PC: a paired client asks the host to suspend, as Vibepollo serves
//! it (`src/power_action.cpp`), so one client code path covers both hosts.
//!
//! `GET /power-actions/v1` hands an idle host's opted-in client a single-use
//! token valid for 30 s; `POST /power-actions/v1/sleep?token=` spends it.
//! Both answer the same XML document, with the HTTP status in `status_code`.
//! The state machine here is pure: the host passes the clock, the random
//! bytes and the lifecycle gate, and runs the suspend itself.
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

/// `PowerActionVersion` in serverinfo and in every response.
pub const VERSION: u8 = 1;
/// How long a token stays valid.
pub const TOKEN_LIFETIME: Duration = Duration::from_secs(30);
/// Live tokens the host keeps, one per paired client.
pub const MAX_TOKENS: usize = 256;

const NO_IDENTITY: &str = "A paired TLS client identity is required";

/// The caller, as its paired TLS certificate identifies it.
#[derive(Clone, Debug)]
pub struct Authority {
    pub uuid: String,
    pub allow_sleep: bool,
}

/// A sleep request: its method, its token, and whether it has exactly one
/// query argument (`token`), an empty body and no `Origin` header.
#[derive(Clone, Debug)]
pub struct Request {
    pub post: bool,
    pub token: String,
    pub valid_shape: bool,
}
impl Request {
    /// Reads the query as Vibepollo's server does: arguments with an empty
    /// name are skipped, names compare without case, values are
    /// percent-decoded.
    pub fn parse(post: bool, query: Option<&str>, body_len: usize, origin: bool) -> Self {
        let mut count = 0;
        let mut token = None;
        for argument in query.unwrap_or("").split('&') {
            let (name, value) = argument.split_once('=').unwrap_or((argument, ""));
            if name.is_empty() {
                continue;
            }
            count += 1;
            if token.is_none() && name.eq_ignore_ascii_case("token") {
                token = Some(percent_decode(value));
            }
        }
        Self {
            post,
            valid_shape: count == 1 && token.is_some() && body_len == 0 && !origin,
            token: token.unwrap_or_default(),
        }
    }
}
fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        match (bytes[i], bytes.get(i + 1), bytes.get(i + 2)) {
            (b'%', Some(&hi), Some(&lo)) if hex(hi).is_some() && hex(lo).is_some() => {
                out.push((hex(hi).unwrap() * 16 + hex(lo).unwrap()) as u8);
                i += 3;
            }
            (b'+', ..) => {
                out.push(b' ');
                i += 1;
            }
            (b, ..) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// One answer of either route.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub message: &'static str,
    pub supported: bool,
    pub allowed: bool,
    pub available: bool,
    pub token: String,
}
impl Response {
    fn status(status: u16, message: &'static str) -> Self {
        Self {
            status,
            message,
            supported: false,
            allowed: false,
            available: false,
            token: String::new(),
        }
    }
    /// The document as Boost's property-tree writer prints Vibepollo's:
    /// the declaration, a newline, then the elements on one line, with an
    /// empty token as `<SleepToken/>`.
    pub fn xml(&self) -> String {
        let flag = |b: bool| u8::from(b);
        let token = if self.token.is_empty() {
            "<SleepToken/>".to_owned()
        } else {
            format!("<SleepToken>{}</SleepToken>", escape(&self.token))
        };
        format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<root status_code=\"{}\" status_message=\"{}\">\
             <PowerActionVersion>{VERSION}</PowerActionVersion>\
             <SleepSupported>{}</SleepSupported>\
             <SleepAllowed>{}</SleepAllowed>\
             <SleepAvailable>{}</SleepAvailable>\
             <SleepActiveSessionPolicy>idle-only</SleepActiveSessionPolicy>\
             {token}\
             <SleepTokenLifetimeSeconds>{}</SleepTokenLifetimeSeconds>\
             <SleepAccepted>{}</SleepAccepted></root>",
            self.status,
            escape(self.message),
            flag(self.supported),
            flag(self.allowed),
            flag(self.available),
            TOKEN_LIFETIME.as_secs(),
            flag(self.status == 202),
        )
    }
}
fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// A sleep request's outcome. `gate` is the held lifecycle gate, only on
/// a 202: the host keeps it through the response and the suspend, so a
/// racing launch waits.
pub struct Decision<G> {
    pub response: Response,
    pub gate: Option<G>,
}

/// The live tokens, one per client UUID.
#[derive(Default)]
pub struct Tokens {
    tokens: HashMap<String, (String, Instant)>,
}
impl Tokens {
    /// `GET /power-actions/v1`. `idle` takes the lifecycle gate without
    /// waiting and answers it only while nothing runs; `random` is 32
    /// random bytes, `None` if the system could not supply them.
    pub fn capabilities<G>(
        &mut self,
        caller: Option<&Authority>,
        supported: bool,
        idle: impl FnOnce() -> Option<G>,
        now: Instant,
        random: impl FnOnce() -> Option<[u8; 32]>,
    ) -> Response {
        let Some(caller) = caller else {
            return Response::status(401, NO_IDENTITY);
        };
        let mut result = Response {
            supported,
            allowed: caller.allow_sleep,
            ..Response::status(200, "")
        };
        if !result.supported || !result.allowed {
            return result;
        }
        let Some(_gate) = idle() else {
            return result;
        };
        self.tokens.retain(|_, (_, expires)| *expires > now);
        if !self.tokens.contains_key(&caller.uuid) && self.tokens.len() >= MAX_TOKENS {
            return result;
        }
        let Some(bytes) = random() else {
            return Response::status(503, "Could not create a sleep request token");
        };
        result.token = bytes.iter().map(|b| format!("{b:02x}")).collect();
        self.tokens.insert(
            caller.uuid.clone(),
            (result.token.clone(), now + TOKEN_LIFETIME),
        );
        result.available = true;
        result
    }
    /// `POST /power-actions/v1/sleep`, checked in Vibepollo's order. A 202
    /// has consumed the token, so a replay after wake cannot suspend again.
    pub fn sleep<G>(
        &mut self,
        request: &Request,
        caller: Option<&Authority>,
        supported: impl FnOnce() -> bool,
        idle: impl FnOnce() -> Option<G>,
        now: Instant,
    ) -> Decision<G> {
        let refuse = |status, message| Decision {
            response: Response::status(status, message),
            gate: None,
        };
        if !request.post {
            return refuse(405, "Use POST for a sleep request");
        }
        if !request.valid_shape || !valid_token(&request.token) {
            return refuse(
                400,
                "Expected one sleep token and an empty body; browser origins are not accepted",
            );
        }
        let Some(caller) = caller else {
            return refuse(401, NO_IDENTITY);
        };
        if !caller.allow_sleep {
            return refuse(403, "Sleep control is disabled for this paired client");
        }
        if !supported() {
            return refuse(501, "Native sleep control is unavailable on this host");
        }
        let Some(gate) = idle() else {
            return refuse(
                409,
                "End all applications and sessions before requesting sleep",
            );
        };
        match self.tokens.get(&caller.uuid) {
            Some((token, expires)) if *expires > now && *token == request.token => {
                self.tokens.remove(&caller.uuid);
            }
            _ => {
                return refuse(
                    409,
                    "Sleep token expired or already used; confirm the action again",
                );
            }
        }
        Decision {
            response: Response::status(
                202,
                "Sleep request accepted; native completion is not confirmed",
            ),
            gate: Some(gate),
        }
    }
}
fn valid_token(token: &str) -> bool {
    token.len() == 64
        && token
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}
/// 32 bytes from the operating system's generator.
pub fn os_random() -> Option<[u8; 32]> {
    use rand::RngCore;
    let mut bytes = [0; 32];
    rand::rngs::OsRng.try_fill_bytes(&mut bytes).ok()?;
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn phone(allow_sleep: bool) -> Authority {
        Authority {
            uuid: "phone".into(),
            allow_sleep,
        }
    }
    fn bytes(b: u8) -> Option<[u8; 32]> {
        Some([b; 32])
    }
    fn idle() -> Option<()> {
        Some(())
    }
    fn busy() -> Option<()> {
        None
    }
    fn post(token: &str) -> Request {
        Request::parse(true, Some(&format!("token={token}")), 0, false)
    }
    fn token(tokens: &mut Tokens, caller: &Authority, now: Instant) -> String {
        let r = tokens.capabilities(Some(caller), true, idle, now, || bytes(0xab));
        assert!(r.available, "{r:?}");
        r.token
    }
    fn sleep(
        tokens: &mut Tokens,
        request: &Request,
        caller: Option<&Authority>,
        now: Instant,
    ) -> Response {
        tokens.sleep(request, caller, || true, idle, now).response
    }

    #[test]
    fn documents_match_vibepollo_byte_for_byte() {
        let mut tokens = Tokens::default();
        let now = Instant::now();
        let r = tokens.capabilities(Some(&phone(true)), true, idle, now, || bytes(0x0f));
        assert_eq!(
            r.xml(),
            format!(
                "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<root status_code=\"200\" status_message=\"\">\
                 <PowerActionVersion>1</PowerActionVersion><SleepSupported>1</SleepSupported>\
                 <SleepAllowed>1</SleepAllowed><SleepAvailable>1</SleepAvailable>\
                 <SleepActiveSessionPolicy>idle-only</SleepActiveSessionPolicy>\
                 <SleepToken>{}</SleepToken><SleepTokenLifetimeSeconds>30</SleepTokenLifetimeSeconds>\
                 <SleepAccepted>0</SleepAccepted></root>",
                "0f".repeat(32)
            )
        );
        let accepted = sleep(&mut tokens, &post(&r.token), Some(&phone(true)), now);
        assert_eq!(
            accepted.xml(),
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<root status_code=\"202\" \
             status_message=\"Sleep request accepted; native completion is not confirmed\">\
             <PowerActionVersion>1</PowerActionVersion><SleepSupported>0</SleepSupported>\
             <SleepAllowed>0</SleepAllowed><SleepAvailable>0</SleepAvailable>\
             <SleepActiveSessionPolicy>idle-only</SleepActiveSessionPolicy><SleepToken/>\
             <SleepTokenLifetimeSeconds>30</SleepTokenLifetimeSeconds>\
             <SleepAccepted>1</SleepAccepted></root>"
        );
    }

    #[test]
    fn capabilities_hand_a_token_only_to_an_allowed_client_of_an_idle_host() {
        let mut tokens = Tokens::default();
        let now = Instant::now();
        let none = tokens.capabilities(None, true, idle, now, || bytes(1));
        assert_eq!((none.status, none.message), (401, NO_IDENTITY));
        let off = tokens.capabilities(Some(&phone(false)), true, idle, now, || bytes(1));
        assert_eq!(
            (off.status, off.supported, off.allowed, off.available),
            (200, true, false, false)
        );
        assert!(off.token.is_empty());
        let unsupported = tokens.capabilities(Some(&phone(true)), false, idle, now, || bytes(1));
        assert_eq!(
            (
                unsupported.supported,
                unsupported.allowed,
                unsupported.available
            ),
            (false, true, false)
        );
        let busy = tokens.capabilities(Some(&phone(true)), true, busy, now, || bytes(1));
        assert_eq!(
            (busy.status, busy.supported, busy.allowed, busy.available),
            (200, true, true, false)
        );
        assert!(busy.token.is_empty());
        let failed = tokens.capabilities(Some(&phone(true)), true, idle, now, || None);
        assert_eq!(
            (failed.status, failed.message, failed.available),
            (503, "Could not create a sleep request token", false)
        );
        assert!(tokens.tokens.is_empty());
        let ok = tokens.capabilities(Some(&phone(true)), true, idle, now, || bytes(0xab));
        assert_eq!((ok.status, ok.available, ok.token.len()), (200, true, 64));
    }

    #[test]
    fn every_sleep_refusal_in_vibepollos_order() {
        let mut tokens = Tokens::default();
        let now = Instant::now();
        let token = token(&mut tokens, &phone(true), now);
        let get = Request {
            post: false,
            ..post(&token)
        };
        let check = |r: Response, status: u16, message: &str| {
            assert_eq!((r.status, r.message), (status, message));
            assert!(!r.xml().contains("<SleepAccepted>1"));
        };
        let mut t = Tokens::default();
        check(
            sleep(&mut t, &get, None, now),
            405,
            "Use POST for a sleep request",
        );
        let shape = "Expected one sleep token and an empty body; browser origins are not accepted";
        check(sleep(&mut t, &post("ABCD"), None, now), 400, shape);
        check(
            sleep(&mut t, &post(&token.to_uppercase()), None, now),
            400,
            shape,
        );
        check(sleep(&mut t, &post(&token), None, now), 401, NO_IDENTITY);
        check(
            sleep(&mut t, &post(&token), Some(&phone(false)), now),
            403,
            "Sleep control is disabled for this paired client",
        );
        check(
            tokens
                .sleep(&post(&token), Some(&phone(true)), || false, idle, now)
                .response,
            501,
            "Native sleep control is unavailable on this host",
        );
        let decision = tokens.sleep(&post(&token), Some(&phone(true)), || true, busy, now);
        assert!(decision.gate.is_none());
        check(
            decision.response,
            409,
            "End all applications and sessions before requesting sleep",
        );
        // None of these spent the token.
        let accepted = tokens.sleep(&post(&token), Some(&phone(true)), || true, idle, now);
        assert_eq!(accepted.response.status, 202);
        assert!(accepted.gate.is_some());
    }

    #[test]
    fn tokens_expire_are_single_use_and_replaced() {
        let used = "Sleep token expired or already used; confirm the action again";
        let now = Instant::now();
        let caller = phone(true);
        // Spent once.
        let mut tokens = Tokens::default();
        let first = token(&mut tokens, &caller, now);
        assert_eq!(
            sleep(&mut tokens, &post(&first), Some(&caller), now).status,
            202
        );
        let replay = tokens.sleep(&post(&first), Some(&caller), || true, idle, now);
        assert_eq!(
            (replay.response.status, replay.response.message),
            (409, used)
        );
        assert!(replay.gate.is_none());
        // Expired at 30 s, still good just before.
        let t = token(&mut tokens, &caller, now);
        let late = sleep(&mut tokens, &post(&t), Some(&caller), now + TOKEN_LIFETIME);
        assert_eq!((late.status, late.message), (409, used));
        let t = token(&mut tokens, &caller, now);
        let just = now + TOKEN_LIFETIME - Duration::from_millis(1);
        assert_eq!(
            sleep(&mut tokens, &post(&t), Some(&caller), just).status,
            202
        );
        // A new GET replaces the client's earlier token.
        let old = tokens
            .capabilities(Some(&caller), true, idle, now, || bytes(1))
            .token;
        let new = tokens
            .capabilities(Some(&caller), true, idle, now, || bytes(2))
            .token;
        assert_eq!(
            sleep(&mut tokens, &post(&old), Some(&caller), now).message,
            used
        );
        assert_eq!(
            sleep(&mut tokens, &post(&new), Some(&caller), now).status,
            202
        );
        // Another client's token is not this client's.
        let other = Authority {
            uuid: "tablet".into(),
            allow_sleep: true,
        };
        let theirs = token(&mut tokens, &other, now);
        assert_eq!(
            sleep(&mut tokens, &post(&theirs), Some(&caller), now).message,
            used
        );
        assert_eq!(
            sleep(&mut tokens, &post(&theirs), Some(&other), now).status,
            202
        );
    }

    #[test]
    fn at_most_256_live_tokens() {
        let now = Instant::now();
        let mut tokens = Tokens::default();
        for i in 0..MAX_TOKENS {
            let caller = Authority {
                uuid: format!("c{i}"),
                allow_sleep: true,
            };
            token(&mut tokens, &caller, now);
        }
        let late = Authority {
            uuid: "late".into(),
            allow_sleep: true,
        };
        let full = tokens.capabilities(Some(&late), true, idle, now, || bytes(1));
        assert_eq!(
            (full.status, full.available, full.token.as_str()),
            (200, false, "")
        );
        // A client with a live token may still replace it.
        let first = Authority {
            uuid: "c0".into(),
            allow_sleep: true,
        };
        assert!(
            tokens
                .capabilities(Some(&first), true, idle, now, || bytes(2))
                .available
        );
        // Expired tokens free their places.
        let later = now + TOKEN_LIFETIME;
        assert!(
            tokens
                .capabilities(Some(&late), true, idle, later, || bytes(3))
                .available
        );
        assert_eq!(tokens.tokens.len(), 1);
    }

    #[test]
    fn only_a_bare_token_query_without_body_or_origin_has_the_shape() {
        let t = "ab".repeat(32);
        assert!(Request::parse(true, Some(&format!("token={t}")), 0, false).valid_shape);
        // Names compare without case, values are percent-decoded.
        let r = Request::parse(true, Some(&format!("TOKEN=%61{}", &t[1..])), 0, false);
        assert!(r.valid_shape);
        assert_eq!(r.token, t);
        assert!(Request::parse(true, Some(&format!("token={t}&")), 0, false).valid_shape);
        for (query, body, origin) in [
            (format!("token={t}"), 1, false),
            (format!("token={t}"), 0, true),
            (format!("token={t}&x=1"), 0, false),
            (format!("token={t}&token={t}"), 0, false),
            (format!("other={t}"), 0, false),
            (String::new(), 0, false),
        ] {
            let request = Request::parse(true, Some(&query), body, origin);
            assert!(!request.valid_shape, "{query} {body} {origin}");
            let mut tokens = Tokens::default();
            let r = tokens
                .sleep(&request, Some(&phone(true)), || true, idle, Instant::now())
                .response;
            assert_eq!(r.status, 400);
        }
        assert!(!Request::parse(true, None, 0, false).valid_shape);
    }

    #[test]
    fn random_tokens_are_64_lowercase_hex() {
        let mut tokens = Tokens::default();
        let r = tokens.capabilities(Some(&phone(true)), true, idle, Instant::now(), os_random);
        assert!(valid_token(&r.token), "{}", r.token);
    }
}
