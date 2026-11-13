//! Sleep PC (`/power-actions/v1`), as Vibepollo serves it (`src/power_action.cpp`) and Rubylight
//! follows: `GET` hands a paired, opted-in client a single-use token valid for 30 s while the
//! host is idle; `POST .../sleep?token=` with an empty body spends it. Both answer the same XML
//! document with the HTTP status in `status_code`. Same pinned HTTPS and client certificate as
//! the classic calls; gate them on `Feature::Sleep`.
//!
//! Only 200 (the offer) and 202 (accepted) are successes. Every refusal the host documents
//! becomes [`PowerError::Refused`] with the parsed reply; anything else is an [`Error`], never a
//! quiet success.

use super::transport::{self, Body, Request, Timeouts};
use super::xml::{self, Element};
use super::{Error, HostClient, LONG_CONNECT_TIMEOUT, MAX_XML_RESPONSE_BYTES, READ_TIMEOUT};

const CAPABILITIES_PATH: &str = "power-actions/v1";
const SLEEP_PATH: &str = "power-actions/v1/sleep";
/// Hex characters in a sleep token (32 random bytes).
const TOKEN_LENGTH: usize = 64;

/// One answer of either route.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PowerReply {
    /// The HTTP status, equal to the document's `status_code`.
    pub status: u16,
    /// The host's `status_message`; empty on an offer.
    pub message: String,
    pub version: u32,
    pub sleep_supported: bool,
    pub sleep_allowed: bool,
    /// Supported, allowed and idle: a token was handed out.
    pub sleep_available: bool,
    /// `idle-only` from Vibepollo and Rubylight 3.0.
    pub active_session_policy: String,
    /// 64 lowercase hex characters, only on an offer the client may spend.
    pub token: Option<String>,
    pub token_lifetime_seconds: u32,
    pub accepted: bool,
}

/// A refusal the host documents, by HTTP status.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Refusal {
    /// 400: not exactly one token and an empty body, or a browser origin.
    BadRequest,
    /// 401: no paired TLS client identity.
    Unauthorized,
    /// 403: sleep is off for this paired client.
    Forbidden,
    /// 405: not a POST.
    MethodNotAllowed,
    /// 409: a session or app is running, or the token expired or was spent.
    Conflict,
    /// 501: the host cannot suspend natively.
    NotImplemented,
    /// 503: the host could not create a token.
    Unavailable,
}

impl Refusal {
    fn from_status(status: u16) -> Option<Self> {
        Some(match status {
            400 => Refusal::BadRequest,
            401 => Refusal::Unauthorized,
            403 => Refusal::Forbidden,
            405 => Refusal::MethodNotAllowed,
            409 => Refusal::Conflict,
            501 => Refusal::NotImplemented,
            503 => Refusal::Unavailable,
            _ => return None,
        })
    }
}

/// Why a power action gave no success.
#[derive(Debug)]
pub enum PowerError {
    /// The host answered with a documented refusal; `reply.message` says why.
    Refused {
        refusal: Refusal,
        reply: PowerReply,
    },
    Host(Error),
}

impl std::fmt::Display for PowerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PowerError::Refused { reply, .. } => {
                write!(f, "host error {}: {}", reply.status, reply.message)
            }
            PowerError::Host(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for PowerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            PowerError::Host(e) => Some(e),
            PowerError::Refused { .. } => None,
        }
    }
}

impl From<Error> for PowerError {
    fn from(e: Error) -> Self {
        PowerError::Host(e)
    }
}

fn is_token(token: &str) -> bool {
    token.len() == TOKEN_LENGTH
        && token
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn malformed(what: impl std::fmt::Display) -> Error {
    Error::Malformed(format!("power action reply: {what}"))
}

fn number(root: &Element, name: &str) -> Result<u32, Error> {
    let text = root.required_text(name)?;
    text.trim()
        .parse()
        .map_err(|_| malformed(format_args!("{name} is {text:?}")))
}

fn bit(root: &Element, name: &str) -> Result<bool, Error> {
    match number(root, name)? {
        0 => Ok(false),
        1 => Ok(true),
        other => Err(malformed(format_args!("{name} is {other}"))),
    }
}

/// The power document in `body`, or None when the body is not one (no XML, or no
/// `PowerActionVersion`): a host or proxy without the API answered.
fn parse(body: &[u8]) -> Result<Option<PowerReply>, Error> {
    let Some(root) = std::str::from_utf8(body)
        .ok()
        .and_then(|text| xml::parse(text).ok())
        .filter(|root| root.name == "root")
    else {
        return Ok(None);
    };
    if !root.elements().any(|e| e.name == "PowerActionVersion") {
        return Ok(None);
    }
    let status = root
        .attribute("status_code")
        .and_then(|code| code.trim().parse().ok())
        .ok_or_else(|| malformed("no status_code"))?;
    let version = number(&root, "PowerActionVersion")?;
    if version < 1 {
        return Err(malformed("PowerActionVersion is 0"));
    }
    let token = root.find_text("SleepToken");
    if let Some(token) = token.as_deref().filter(|t| !is_token(t)) {
        return Err(malformed(format_args!(
            "SleepToken {token:?} is not 64 hex digits"
        )));
    }
    Ok(Some(PowerReply {
        status,
        message: root.attribute("status_message").unwrap_or("").to_owned(),
        version,
        sleep_supported: bit(&root, "SleepSupported")?,
        sleep_allowed: bit(&root, "SleepAllowed")?,
        sleep_available: bit(&root, "SleepAvailable")?,
        active_session_policy: root
            .find_text("SleepActiveSessionPolicy")
            .unwrap_or_default(),
        token,
        token_lifetime_seconds: number(&root, "SleepTokenLifetimeSeconds")?,
        accepted: bit(&root, "SleepAccepted")?,
    }))
}

/// Reads an answer to `path`: `success` is the one status that is not a refusal.
fn decode(
    response: transport::Response,
    path: &str,
    success: u16,
) -> Result<PowerReply, PowerError> {
    let status = response.status;
    let Some(reply) = parse(&response.body)? else {
        // Vibepollo answers a failed certificate check with HTTP 200 and a bare
        // `<root status_code="401">`: that status, not the HTTP one, is the answer. A
        // success code without the power document stays malformed below.
        if let Some(Err(error @ Error::Status { code, .. })) = std::str::from_utf8(&response.body)
            .ok()
            .map(xml::parse_reply)
        {
            if code != i32::from(success) {
                return Err(error.into());
            }
        }
        return Err(PowerError::Host(match status {
            404 => Error::NotFound(format!("/{path}")),
            s if s == success => malformed(format_args!("HTTP {s} without a power document")),
            s => Error::Status {
                code: i32::from(s),
                message: response.reason,
            },
        }));
    };
    if reply.status != status {
        return Err(malformed(format_args!(
            "status_code {} under HTTP {status}",
            reply.status
        ))
        .into());
    }
    if status == success {
        // Only a 202 says the host took the request; only it may carry SleepAccepted.
        if reply.accepted != (success == 202) {
            return Err(malformed(format_args!(
                "SleepAccepted is {} on {status}",
                reply.accepted
            ))
            .into());
        }
        return Ok(reply);
    }
    Err(match Refusal::from_status(status) {
        Some(refusal) => PowerError::Refused { refusal, reply },
        None => PowerError::Host(Error::Status {
            code: i32::from(status),
            message: reply.message,
        }),
    })
}

impl HostClient {
    fn power(&self, target: &str, body: Option<Body<'_>>) -> Result<transport::Response, Error> {
        let request = Request {
            host: &self.host,
            port: self.https_port(true)?,
            target,
            tls: Some(self.tls()?),
            timeouts: Timeouts {
                connect: LONG_CONNECT_TIMEOUT,
                read: Some(READ_TIMEOUT),
            },
            body_limit: MAX_XML_RESPONSE_BYTES,
        };
        transport::send(&request, &self.cancel, body)
    }

    /// The host's sleep offer for this client. `token` is set only when sleep is supported,
    /// allowed and the host is idle; spend it with [`HostClient::sleep`] within
    /// `token_lifetime_seconds`.
    pub fn power_capabilities(&self) -> Result<PowerReply, PowerError> {
        // No uniqueid or uuid: the routes need nothing but the client certificate.
        let response = self.power(CAPABILITIES_PATH, None)?;
        decode(response, CAPABILITIES_PATH, 200)
    }

    /// Asks the host to sleep. Ok means the host accepted (202); it does not confirm that the
    /// PC suspended. The target carries the token and nothing else, or the host answers 400.
    pub fn sleep(&self, token: &str) -> Result<PowerReply, PowerError> {
        if !is_token(token) {
            return Err(
                Error::InvalidArgument("a sleep token is 64 lowercase hex digits".into()).into(),
            );
        }
        let target = format!("{SLEEP_PATH}?token={token}");
        let response = self.power(&target, Some(Body::empty()))?;
        decode(response, SLEEP_PATH, 202)
    }
}

#[cfg(test)]
mod tests;
