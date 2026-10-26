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
mod tests {
    use super::*;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    /// The document Vibepollo's Boost writer prints, and Rubylight's copy of it.
    fn document(status: u16, message: &str, flags: [u8; 3], token: &str) -> String {
        let token = if token.is_empty() {
            "<SleepToken/>".to_owned()
        } else {
            format!("<SleepToken>{token}</SleepToken>")
        };
        format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<root status_code=\"{status}\" \
             status_message=\"{message}\"><PowerActionVersion>1</PowerActionVersion>\
             <SleepSupported>{}</SleepSupported><SleepAllowed>{}</SleepAllowed>\
             <SleepAvailable>{}</SleepAvailable>\
             <SleepActiveSessionPolicy>idle-only</SleepActiveSessionPolicy>{token}\
             <SleepTokenLifetimeSeconds>30</SleepTokenLifetimeSeconds>\
             <SleepAccepted>{}</SleepAccepted></root>",
            flags[0],
            flags[1],
            flags[2],
            u8::from(status == 202)
        )
    }

    fn response(status: u16, body: &str) -> transport::Response {
        transport::Response {
            status,
            reason: "Reason".into(),
            content_type: Some("application/xml".into()),
            body: body.as_bytes().to_vec(),
        }
    }

    #[test]
    fn an_offer_carries_the_token() {
        let reply = decode(
            response(200, &document(200, "", [1, 1, 1], TOKEN)),
            CAPABILITIES_PATH,
            200,
        )
        .unwrap();
        assert_eq!(
            reply,
            PowerReply {
                status: 200,
                message: String::new(),
                version: 1,
                sleep_supported: true,
                sleep_allowed: true,
                sleep_available: true,
                active_session_policy: "idle-only".into(),
                token: Some(TOKEN.into()),
                token_lifetime_seconds: 30,
                accepted: false,
            }
        );
        // Busy, not allowed or unsupported: still a 200, with no token.
        for flags in [[1, 1, 0], [1, 0, 0], [0, 0, 0]] {
            let body =
                document(200, "", flags, "").replace("<SleepToken/>", "<SleepToken></SleepToken>");
            let reply = decode(response(200, &body), CAPABILITIES_PATH, 200).unwrap();
            assert_eq!((reply.token, reply.sleep_available), (None, false));
        }
    }

    #[test]
    fn an_accepted_sleep_is_the_only_post_success() {
        let message = "Sleep request accepted; native completion is not confirmed";
        let reply = decode(
            response(202, &document(202, message, [1, 1, 0], "")),
            SLEEP_PATH,
            202,
        )
        .unwrap();
        assert!(reply.accepted && reply.token.is_none());
        assert_eq!(reply.message, message);
        // A 200 to the POST is not an acceptance.
        assert!(matches!(
            decode(
                response(200, &document(200, "", [1, 1, 1], TOKEN)),
                SLEEP_PATH,
                202
            ),
            Err(PowerError::Host(Error::Status { code: 200, .. }))
        ));
    }

    #[test]
    fn every_documented_refusal_keeps_its_reply() {
        for (status, refusal, message) in [
            (
                400,
                Refusal::BadRequest,
                "Expected one sleep token and an empty body; browser origins are not accepted",
            ),
            (
                401,
                Refusal::Unauthorized,
                "A paired TLS client identity is required",
            ),
            (
                403,
                Refusal::Forbidden,
                "Sleep control is disabled for this paired client",
            ),
            (
                405,
                Refusal::MethodNotAllowed,
                "Use POST for a sleep request",
            ),
            (
                409,
                Refusal::Conflict,
                "End all applications and sessions before requesting sleep",
            ),
            (
                409,
                Refusal::Conflict,
                "Sleep token expired or already used; confirm the action again",
            ),
            (
                501,
                Refusal::NotImplemented,
                "Native sleep control is unavailable on this host",
            ),
            (
                503,
                Refusal::Unavailable,
                "Could not create a sleep request token",
            ),
        ] {
            let body = document(status, message, [1, 0, 0], "");
            for (path, success) in [(SLEEP_PATH, 202), (CAPABILITIES_PATH, 200)] {
                match decode(response(status, &body), path, success) {
                    Err(PowerError::Refused { refusal: r, reply }) => {
                        assert_eq!(
                            (r, reply.status, reply.message.as_str()),
                            (refusal, status, message)
                        );
                        assert!(!reply.accepted);
                    }
                    other => panic!("{status}: {other:?}"),
                }
            }
        }
    }

    #[test]
    fn unknown_answers_are_errors_not_successes() {
        let status = |result: Result<PowerReply, PowerError>| match result {
            Err(PowerError::Host(Error::Status { code, .. })) => code,
            other => panic!("{other:?}"),
        };
        // An undocumented status with a power document, and refusals without one.
        assert_eq!(
            status(decode(
                response(500, &document(500, "x", [0, 0, 0], "")),
                SLEEP_PATH,
                202
            )),
            500
        );
        assert_eq!(
            status(decode(
                response(401, "<root status_code=\"401\"/>"),
                SLEEP_PATH,
                202
            )),
            401
        );
        assert_eq!(
            status(decode(response(503, "busy"), CAPABILITIES_PATH, 200)),
            503
        );
        assert!(matches!(
            decode(response(404, "<html/>"), CAPABILITIES_PATH, 200),
            Err(PowerError::Host(Error::NotFound(p))) if p == "/power-actions/v1"
        ));
        let malformed = |status: u16, body: &str, success: u16| {
            let result = decode(response(status, body), SLEEP_PATH, success);
            assert!(
                matches!(result, Err(PowerError::Host(Error::Malformed(_)))),
                "{body}: {result:?}"
            );
        };
        // A success without a power document, or with one that does not hold.
        malformed(202, "", 202);
        malformed(202, "<root status_code=\"202\"/>", 202);
        malformed(202, &document(200, "", [1, 1, 1], ""), 202);
        malformed(
            200,
            &document(200, "", [1, 1, 1], "").replace("<SleepAccepted>0", "<SleepAccepted>1"),
            200,
        );
        malformed(
            202,
            &document(202, "", [1, 1, 0], "").replace("<SleepAccepted>1", "<SleepAccepted>0"),
            202,
        );
        malformed(200, &document(200, "", [2, 1, 1], ""), 200);
        malformed(200, &document(200, "", [1, 1, 1], "ABC"), 200);
        malformed(
            200,
            &document(200, "", [1, 1, 1], &TOKEN.to_uppercase()),
            200,
        );
        malformed(
            200,
            &document(200, "", [1, 1, 1], "")
                .replace("<PowerActionVersion>1", "<PowerActionVersion>0"),
            200,
        );
        malformed(
            200,
            &document(200, "", [1, 1, 1], "").replace(
                "<SleepTokenLifetimeSeconds>30",
                "<SleepTokenLifetimeSeconds>",
            ),
            200,
        );
        // A refusal whose document is broken is malformed too, not a refusal.
        malformed(409, &document(409, "x", [7, 0, 0], ""), 202);
    }

    #[test]
    fn a_bad_token_is_never_sent() {
        // Port 1 on loopback refuses connections: an Io error would mean a request went out.
        let identity = std::sync::Arc::new(crate::http::identity::tests::fixture());
        let client = HostClient::new("127.0.0.1", 1, identity)
            .unwrap()
            .with_https_port(1);
        for token in ["", "abc", &TOKEN.to_uppercase(), &format!("{TOKEN}&uuid=x")] {
            assert!(
                matches!(
                    client.sleep(token),
                    Err(PowerError::Host(Error::InvalidArgument(_)))
                ),
                "{token}"
            );
        }
    }
}
