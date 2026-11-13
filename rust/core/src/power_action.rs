//! Sleep PC: a paired client asks the host to suspend, as Vibepollo serves
//! it (`src/power_action.cpp`), so one client code path covers both hosts.
//! Restart and Shut down ([`Action`]) reuse its tokens and idle gate from
//! the native API, behind a grant of their own.
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

/// The sleep states Windows reports for this PC.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SleepStates {
    pub s1: bool,
    pub s2: bool,
    pub s3: bool,
    pub modern_standby: bool,
    /// S4 with its hibernation file present.
    pub hibernate: bool,
}
impl SleepStates {
    /// Why Sleep PC is unavailable, `None` when the PC has S1, S2 or S3.
    /// Modern Standby alone is not enough: Windows has no call to enter it,
    /// and on such a laptop with hibernate on the request hibernated the PC.
    pub fn unavailable(&self) -> Option<&'static str> {
        if self.s1 || self.s2 || self.s3 {
            return None;
        }
        Some(match (self.modern_standby, self.hibernate) {
            (true, true) => "this PC only has Modern Standby, so Windows would hibernate it",
            (true, false) => {
                "this PC only has Modern Standby, which Windows offers no call to enter"
            }
            (false, _) => "this PC has no sleep state",
        })
    }
    /// For the log: "S3 available, hibernate on".
    pub fn describe(&self) -> String {
        let states: Vec<_> = [
            (self.s1, "S1"),
            (self.s2, "S2"),
            (self.s3, "S3"),
            (self.modern_standby, "Modern Standby"),
        ]
        .into_iter()
        .filter_map(|(present, name)| present.then_some(name))
        .collect();
        format!(
            "{}, hibernate {}",
            if states.is_empty() {
                "no sleep state".to_owned()
            } else {
                format!("{} available", states.join(", "))
            },
            if self.hibernate { "on" } else { "off" }
        )
    }
}

/// The caller, as its paired TLS certificate identifies it.
#[derive(Clone, Debug)]
pub struct Authority {
    pub uuid: String,
    pub allow_sleep: bool,
    /// Restart and shut down PC, a grant of its own.
    pub allow_restart_shutdown: bool,
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
/// Restart and Shut down, from the native API (`/api/native/v1/power`).
/// Their own grant, the same idle gate as Sleep PC, and tokens of their own
/// [`Tokens`]: a sleep token never restarts the PC.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Restart,
    Shutdown,
}
impl Action {
    /// `restart` or `shutdown`, as the routes and the log name it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Restart => "restart",
            Self::Shutdown => "shutdown",
        }
    }
    /// The webhook event of an accepted request.
    pub fn event(self) -> &'static str {
        match self {
            Self::Restart => "power.restart",
            Self::Shutdown => "power.shutdown",
        }
    }
}

/// What keeps the PC from being idle, for a refusal that says so.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Busy {
    /// These devices stream.
    Streaming(Vec<String>),
    /// These devices start a stream.
    Starting(Vec<String>),
    /// This app still runs, without a stream.
    App(String),
    /// A launch or a stop holds the lifecycle gate.
    Transition,
    /// A stream is being torn down, or a virtual display is still kept.
    Ending,
    /// A browser guest is connected.
    Guest,
    /// A controller session is in use.
    Controller,
}
impl Busy {
    pub fn message(&self) -> String {
        let names = |names: &[String]| match names {
            [] => "A device is".to_owned(),
            [one] => format!("{one} is"),
            [rest @ .., last] => format!("{} and {last} are", rest.join(", ")),
        };
        match self {
            Self::Streaming(devices) => format!("{} streaming from this PC.", names(devices)),
            Self::Starting(devices) => format!("{} starting a stream.", names(devices)),
            Self::App(app) => format!("{app} is still running on this PC."),
            Self::Transition => "A stream is starting or stopping.".to_owned(),
            Self::Ending => "A stream is still ending.".to_owned(),
            Self::Guest => "A browser guest is connected.".to_owned(),
            Self::Controller => "A controller session is in use.".to_owned(),
        }
    }
}

/// Why a restart or shutdown was refused, in the order the host checks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refused {
    /// The device lacks Restart and shut down PC.
    NotAllowed,
    Busy(Busy),
    /// No live token of this device matches.
    Token,
}
pub const RESTART_SHUTDOWN_OFF: &str = "Restart and shut down PC is off for this device.";

/// A restart or shutdown Windows did not confirm in time: no refusal, the PC
/// may still go down. `GET /power` says so in `last_refusal_unconfirmed`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unconfirmed(pub String);
impl std::fmt::Display for Unconfirmed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Unconfirmed {}
const NO_TOKEN: &str = "The host could not create a confirmation token.";

/// `GET /power`'s offer: the same for both actions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer {
    /// Why neither action can run, `None` when both can.
    pub reason: Option<String>,
    pub token: Option<String>,
}

impl Tokens {
    /// `GET /power`: a token for an allowed device of an idle host. A new
    /// offer replaces the device's earlier token.
    pub fn offer<G>(
        &mut self,
        caller: &Authority,
        idle: impl FnOnce() -> Result<G, Busy>,
        now: Instant,
        random: impl FnOnce() -> Option<[u8; 32]>,
    ) -> Offer {
        let refuse = |reason: String| Offer {
            reason: Some(reason),
            token: None,
        };
        if !caller.allow_restart_shutdown {
            return refuse(RESTART_SHUTDOWN_OFF.to_owned());
        }
        let _gate = match idle() {
            Ok(gate) => gate,
            Err(busy) => return refuse(busy.message()),
        };
        self.tokens.retain(|_, (_, expires)| *expires > now);
        if !self.tokens.contains_key(&caller.uuid) && self.tokens.len() >= MAX_TOKENS {
            return refuse(NO_TOKEN.to_owned());
        }
        let Some(bytes) = random() else {
            return refuse(NO_TOKEN.to_owned());
        };
        let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        self.tokens
            .insert(caller.uuid.clone(), (token.clone(), now + TOKEN_LIFETIME));
        Offer {
            reason: None,
            token: Some(token),
        }
    }
    /// `POST /power/restart` or `/shutdown`: the grant, then the idle gate,
    /// then the token, which only an accepted request spends. The gate comes
    /// back held, for the host to keep through the call to Windows.
    pub fn power<G>(
        &mut self,
        caller: &Authority,
        token: &str,
        idle: impl FnOnce() -> Result<G, Busy>,
        now: Instant,
    ) -> Result<G, Refused> {
        if !caller.allow_restart_shutdown {
            return Err(Refused::NotAllowed);
        }
        let gate = idle().map_err(Refused::Busy)?;
        match self.tokens.get(&caller.uuid) {
            Some((live, expires)) if *expires > now && valid_token(token) && live == token => {
                self.tokens.remove(&caller.uuid);
                Ok(gate)
            }
            _ => Err(Refused::Token),
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
mod tests;
