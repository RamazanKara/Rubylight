//! Session action execution and permission checks shared by `couch_menu`, `native_api`, and
//! `reconnect_tests`, decoupled from `axum` response types.

use crate::state::Shared;
use butterpollo_session::error_catalog;
use butterpollo_session::{
    native::{self, LAUNCH, VIEW},
    session_state::Refusal,
    state::Client,
};
use rubylight_native_api::error::{ApiError, ErrorCode};

/// How soon a client retries after `session_busy`.
const BUSY_RETRY_MS: u64 = 1000;

/// A refused or failed request, answered with the error body of the native API spec.
#[derive(Debug)]
pub struct Failure {
    pub code: ErrorCode,
    pub message: String,
    pub setting: Option<String>,
    pub retry_after_ms: Option<u64>,
    /// Boxed, so the many `Result<_, Failure>` stay small.
    explained: Option<Box<Explained>>,
}

/// What the cause adds to the error body.
#[derive(Debug, Default)]
struct Explained {
    /// The cause's own headline; without one the body takes `error_catalog`'s.
    title: Option<String>,
    next_step: Option<String>,
    /// Codes and error chains, already redacted and cut by [`error_catalog::detail`].
    technical_detail: Option<String>,
}

impl Failure {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            setting: None,
            retry_after_ms: None,
            explained: None,
        }
    }

    /// The error chain goes in the technical detail, redacted; the host log has it whole.
    pub fn internal(what: &str, error: impl std::fmt::Display) -> Self {
        let detail = format!("{error:#}");
        tracing::error!(%error, "native API: {what}");
        Self::new(ErrorCode::Internal, format!("The host failed to {what}.")).with_detail(detail)
    }

    /// A headline and next step this cause knows better than its code; `None` leaves the next
    /// step out rather than taking the code's.
    pub fn explained(mut self, title: &str, next_step: Option<&str>) -> Self {
        let explained = self.explained.get_or_insert_with(Box::default);
        explained.title = Some(title.to_owned());
        explained.next_step = next_step.map(str::to_owned);
        self
    }

    /// The codes or error chain behind the failure, for a bug report.
    pub fn with_detail(mut self, detail: impl AsRef<str>) -> Self {
        self.explained
            .get_or_insert_with(Box::default)
            .technical_detail = Some(error_catalog::detail(detail.as_ref()));
        self
    }

    /// The error body: the message as it was, with the title and next step of the cause, else
    /// of the known failure in the technical detail, else of the code.
    pub fn into_api_error(self) -> ApiError {
        let mut error = ApiError::new(self.code, self.message);
        error.setting = self.setting;
        error.retry_after_ms = self.retry_after_ms;
        if let Some(explained) = self.explained {
            let Explained {
                title,
                next_step,
                technical_detail,
            } = *explained;
            error.title = title;
            error.next_step = next_step;
            error.technical_detail = technical_detail;
        }
        error_catalog::explain(error)
    }

    pub fn code(&self) -> ErrorCode {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    /// Name the setting the error is about.
    pub fn for_setting(mut self, id: &str) -> Self {
        self.setting = Some(id.to_owned());
        self
    }

    pub fn retry_after(mut self, milliseconds: u64) -> Self {
        self.retry_after_ms = Some(milliseconds);
        self
    }
}

impl From<crate::continue_here::Refused> for Failure {
    fn from(refused: crate::continue_here::Refused) -> Self {
        match refused {
            crate::continue_here::Refused::Session(refusal) => refusal.into(),
            crate::continue_here::Refused::GuestsWatching => Self::new(
                ErrorCode::HostInUse,
                "Guests are watching the other device's stream.",
            ),
            crate::continue_here::Refused::Launch(message) => {
                Self::new(ErrorCode::HostInUse, message)
            }
        }
    }
}

impl From<Refusal> for Failure {
    fn from(refusal: Refusal) -> Self {
        match refusal {
            Refusal::NoSession => Self::new(ErrorCode::NoSession, "There is no session."),
            Refusal::Busy => Self::new(
                ErrorCode::SessionBusy,
                "A launch or a stop is in progress. Try again in a moment.",
            )
            .retry_after(BUSY_RETRY_MS),
            Refusal::NotOwner => Self::new(
                ErrorCode::NotSessionOwner,
                "This session belongs to another device.",
            ),
        }
    }
}

pub fn permission_required(name: &str) -> Failure {
    Failure::new(
        ErrorCode::PermissionRequired,
        format!("Turn on {name} for this device in the Rubylight console."),
    )
}

pub fn require(client: &Client, bits: u32, name: &str) -> Result<(), Failure> {
    client
        .allows(bits)
        .then_some(())
        .ok_or_else(|| permission_required(name))
}

/// View streams or Launch applications: what seeing and leaving a session needs.
pub fn require_view_or_launch(client: &Client) -> Result<(), Failure> {
    require(client, VIEW, "View streams")
        .or_else(|_| require(client, LAUNCH, "Launch applications"))
}

pub fn require_host_control(client: &Client) -> Result<(), Failure> {
    native::host_control(client).then_some(()).ok_or_else(|| {
        Failure::new(
            ErrorCode::HostControlRequired,
            "Turn on Host control for this device in the Rubylight console.",
        )
    })
}

#[derive(Clone, Copy, Debug)]
pub enum Action {
    Disconnect,
    End,
    CloseGame,
}

/// Checks `client`'s permissions, then runs the action. The native API endpoints
/// and the couch menu's rows both run session actions through here.
pub fn session_action(h: &Shared, client: &Client, action: Action) -> Result<(), Failure> {
    match action {
        Action::Disconnect => require_view_or_launch(client)?,
        Action::End => require(client, LAUNCH, "Launch applications")?,
        Action::CloseGame => {
            require(client, LAUNCH, "Launch applications")?;
            require_host_control(client)?;
        }
    }
    let device = client.uuid.as_str();
    let disconnect = matches!(action, Action::Disconnect);
    let stopped_launch = match action {
        Action::Disconnect => false,
        Action::End => h.stop_launch(|launching| launching == device),
        Action::CloseGame => {
            h.stop_launch(|launching| crate::nvhttp::same_device(launching, device))
        }
    };
    if crate::nvhttp::quit_nucleus_stream(h, device, disconnect)? {
        return Ok(());
    }
    let done = match action {
        Action::Disconnect => h.disconnect_session(Some(device)),
        Action::End => h.end_session(Some(device)),
        Action::CloseGame => h.close_game(device),
    };
    match done {
        Err(Refusal::NoSession) if stopped_launch => Ok(()),
        Err(Refusal::NotOwner)
            if device.ends_with(butterpollo_session::state::SECOND_SCREEN_SUFFIX)
                && !matches!(action, Action::CloseGame) =>
        {
            end_second_screen(h, device, matches!(action, Action::End));
            Ok(())
        }
        done => done.map(drop).map_err(Failure::from),
    }
}

fn end_second_screen(h: &Shared, screen: &str, end: bool) {
    let canceled = h.sessions.lock().unwrap().request_stop(Some(screen));
    drop(canceled);
    crate::remote_display::disconnect(h, Some(screen));
    if end {
        let display = h.app_display.lock().unwrap().remove(screen);
        drop(display);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_internal_failure_keeps_its_chain_in_the_redacted_detail_only() {
        let body = Failure::internal(
            "read the library",
            r"open C:\Users\Alice\library.json: Access is denied. (os error 5)",
        )
        .into_api_error();
        assert_eq!(body.code, ErrorCode::Internal);
        assert_eq!(body.message, "The host failed to read the library.");
        assert_eq!(
            body.title.as_deref(),
            Some("Something went wrong on the PC")
        );
        let next_step = body.next_step.as_deref().unwrap();
        assert!(!next_step.contains("os error"), "{next_step}");
        let detail = body.technical_detail.as_deref().unwrap();
        assert!(detail.contains("(os error 5)"), "{detail}");
        assert!(!detail.contains("Alice"), "{detail}");
    }

    #[test]
    fn a_refusal_without_a_known_cause_takes_its_codes_words() {
        let body = Failure::from(Refusal::NotOwner).into_api_error();
        assert_eq!(body.message, "This session belongs to another device.");
        assert_eq!(
            body.title.as_deref(),
            Some("Another device owns this session")
        );
        assert!(body.next_step.is_some());
        assert_eq!(body.technical_detail, None);
        let body = Failure::from(Refusal::Busy).into_api_error();
        assert_eq!(body.retry_after_ms, Some(BUSY_RETRY_MS));
        assert_eq!(body.next_step, None, "the message already says to retry");
    }
}
