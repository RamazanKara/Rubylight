//! WebRTC pairing request validation, pending PIN lifecycle and completion.

use super::connection::{Authenticated, Connection, Flow};
use butterpollo_session::webrtc::protocol::{ErrorCode, ServerMessage};
use butterpollo_stream::state::{PinEntry, pairing_notice};
use tokio::sync::oneshot;

pub(super) struct Pairing {
    pub(super) request_id: String,
    pub(super) entered: oneshot::Receiver<PinEntry>,
    pub(super) deadline: tokio::time::Instant,
}

/// What came of a pairing request.
pub(super) enum Outcome {
    Entered(Result<PinEntry, oneshot::error::RecvError>),
    Expired,
}

/// Unicode's format characters (Cf, as of Unicode 15.1) and the line and paragraph separators
/// (Zl, Zp): bidi overrides and isolates reorder the text around them, zero-width ones hide
/// in it, and a separator starts a line that can pass for the notice's own. Refusing U+200D
/// also refuses emoji joined with it.
fn invisible(c: char) -> bool {
    matches!(
        c,
        '\u{00ad}'
            | '\u{0600}'..='\u{0605}'
            | '\u{061c}'
            | '\u{06dd}'
            | '\u{070f}'
            | '\u{0890}'..='\u{0891}'
            | '\u{08e2}'
            | '\u{180e}'
            | '\u{200b}'..='\u{200f}'
            | '\u{2028}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206f}'
            | '\u{feff}'
            | '\u{fff9}'..='\u{fffb}'
            | '\u{110bd}'
            | '\u{110cd}'
            | '\u{13430}'..='\u{1343f}'
            | '\u{1bca0}'..='\u{1bca3}'
            | '\u{1d173}'..='\u{1d17a}'
            | '\u{e0001}'
            | '\u{e0020}'..='\u{e007f}'
    )
}

/// What is wrong with a pairing device's name, if anything: a format or separator character
/// ([`invisible`]), a digit other than ASCII 0-9
/// (fullwidth, circled, superscript, math bold, Arabic-Indic and the like, which can spell a PIN
/// the ASCII check misses), or ASCII digits that, read in order with anything between them
/// dropped, contain `pin`.
pub(super) fn name_problem(pin: &str, name: &str) -> Option<&'static str> {
    if name.chars().any(invisible) {
        return Some("clientName may not contain format or line separator characters");
    }
    if name.chars().any(|c| c.is_numeric() && !c.is_ascii_digit()) {
        return Some("clientName may use only the digits 0-9");
    }
    let digits: String = name.chars().filter(char::is_ascii_digit).collect();
    (!pin.is_empty() && digits.contains(pin)).then_some("clientName must not contain the PIN")
}

impl Connection {
    pub(super) fn request_pairing(&mut self, pin: &str, name: &str, out: &mut Vec<ServerMessage>) {
        if self.client.is_some() {
            out.push(Self::error(
                ErrorCode::AlreadyAuthenticated,
                "request-pairing",
                "This connection is already authenticated",
            ));
            return;
        }
        let name = if name.trim().is_empty() {
            "WebRTC client"
        } else {
            name.trim()
        };
        if butterpollo_crypto::pairing::validate_device_name(name).is_err() {
            out.push(Self::error(
                ErrorCode::InvalidMessage,
                "request-pairing",
                "clientName is not a valid device name",
            ));
            return;
        }
        // The console and the tray show the name: a name carrying the PIN would hand the owner
        // the PIN to type for whoever asks. Refused before the rate limit, so a name to fix
        // costs none of the device's requests.
        if let Some(problem) = name_problem(pin, name) {
            tracing::info!(peer = %self.peer.address, problem, "WebRTC pairing refused: the device name");
            out.push(Self::error(
                ErrorCode::InvalidMessage,
                "request-pairing",
                problem,
            ));
            return;
        }
        if !self.clients.may_pair(self.peer.address.ip().to_canonical()) {
            tracing::warn!(peer = %self.peer.address, "WebRTC pairing refused: too many pairing requests from this device in the last minute");
            out.push(Self::error(
                ErrorCode::PairingFailed,
                "request-pairing",
                "Too many pairing requests from this device; wait a minute and try again",
            ));
            return;
        }
        if !self
            .h
            .config
            .read()
            .unwrap()
            .boolean("enable_pairing", true)
        {
            tracing::info!(peer = %self.peer.address, "WebRTC pairing refused: pairing is turned off");
            out.push(Self::error(
                ErrorCode::PairingFailed,
                "request-pairing",
                "Pairing is turned off on this host",
            ));
            return;
        }
        // A new request replaces the connection's old one, and does not
        // raise another notification.
        let again = self.pairing.is_some();
        self.cancel_pairing();
        let address = self.peer.address.ip().to_canonical();
        let (request_id, entered) = self.clients.begin_pairing(&self.h, pin, name, address);
        let deadline = tokio::time::Instant::now() + self.limits.pairing;
        let notice_id = request_id.clone();
        self.pairing = Some(Pairing {
            request_id,
            entered,
            deadline,
        });
        // The socket may wait for the answer for as long as the request lives.
        self.unauthenticated_until = self.unauthenticated_until.max(deadline);
        tracing::info!(client = name, peer = %self.peer.address, "WebRTC pairing PIN required in the web interface");
        if !again && self.clients.may_notify(address) {
            let (title, text) = pairing_notice(name, address);
            butterpollo_win_io::tray::notify_pairing(&notice_id, &title, &text);
        }
    }

    pub(super) fn cancel_pairing(&mut self) {
        if let Some(pairing) = self.pairing.take() {
            self.clients.cancel_pairing(&self.h, &pairing.request_id);
        }
    }

    /// Resolves when the administrator entered the PIN, the request was
    /// withdrawn, or it ran out; never without a request.
    pub(super) async fn pairing_outcome(&mut self) -> Outcome {
        match &mut self.pairing {
            None => std::future::pending().await,
            Some(pairing) => tokio::select! {
                entered = &mut pairing.entered => Outcome::Entered(entered),
                _ = tokio::time::sleep_until(pairing.deadline) => Outcome::Expired,
            },
        }
    }

    pub(super) fn finish_pairing(
        &mut self,
        outcome: Outcome,
        out: &mut Vec<ServerMessage>,
    ) -> Flow {
        let Some(pairing) = self.pairing.take() else {
            return Flow::Continue;
        };
        let failure = |code, text: &str| Self::error(code, "request-pairing", text);
        match outcome {
            Outcome::Expired => {
                self.clients.cancel_pairing(&self.h, &pairing.request_id);
                out.push(failure(
                    ErrorCode::PairingExpired,
                    "The pairing request expired",
                ));
            }
            // The console entry was removed without a PIN.
            Outcome::Entered(Err(_)) => {
                self.clients.cancel_pairing(&self.h, &pairing.request_id);
                out.push(failure(
                    ErrorCode::PairingCancelled,
                    "The pairing request was cancelled",
                ));
            }
            Outcome::Entered(Ok((entered, name, _, approver))) => {
                match self.clients.finish_pairing(
                    &self.h,
                    &pairing.request_id,
                    &entered,
                    &name,
                    approver.as_deref(),
                    self.peer.address.ip(),
                ) {
                    Err(code) => {
                        let text = match code {
                            ErrorCode::IncorrectPin => "The PIN does not match the one shown",
                            ErrorCode::PairingExpired => "The pairing request expired",
                            _ => "Pairing failed",
                        };
                        if code == ErrorCode::IncorrectPin {
                            tracing::warn!(peer = %self.peer.address, "WebRTC pairing failed: the PIN entered in the console is not the one the device shows");
                        } else {
                            tracing::info!(peer = %self.peer.address, %code, "WebRTC pairing did not complete");
                        }
                        out.push(failure(code, text));
                    }
                    Ok(credentials) => {
                        self.challenge.discard();
                        if self.peer.opened.plain() {
                            // Protocol v2 has no encryption of its own: the secret is visible to
                            // a passive observer on the local network during this one exchange.
                            tracing::warn!(device = %name, peer = %self.peer.address, "WebRTC device paired over plain ws://: its secret crossed the local network unencrypted once. Unpair it on Devices if anyone could watch this network.");
                        }
                        out.push(ServerMessage::paired(
                            credentials.client_id.clone(),
                            credentials.secret.clone(),
                        ));
                        if self
                            .clients
                            .authenticate(self.id, &credentials.client_id)
                            .is_ok()
                        {
                            self.client = Some(Authenticated {
                                client_id: credentials.client_id,
                                name,
                            });
                            self.startup(out);
                        }
                    }
                }
            }
        }
        Flow::Continue
    }
}
