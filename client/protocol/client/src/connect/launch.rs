//! Shared app resolution, resume checks, launch/quit/410-confirm flow, and
//! `never_replace` race guards unified between Desktop and Android.

use nightjar_policy::continue_here::{self, ResumeRefusal};
use nightjar_policy::http::{
    App, AppControl, AppRole, Feature, HostCapabilities, PERMISSION_LIST, ServerInfo,
};

use super::{ConnectError, LocalLaunchFailure, http_cause_key};
use crate::http::{
    self, HostClient, LaunchRequest, Launched, StreamMove, asks_to_confirm_replace,
    asks_to_confirm_terminate, is_role_completed, is_terminate_completed,
};

/// Outcome of the user confirmation prompt for 410 replace/terminate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfirmChoice {
    Confirmed,
    Rejected,
    TimedOut,
    Cancelled,
    Interrupted,
}

/// Platform confirmation callback when a launch hits 410 replace/terminate.
pub trait ConfirmReplace {
    fn confirm_replace(&self, terminate_only: bool) -> ConfirmChoice;
}

/// Target application being launched or resumed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchTarget {
    pub app_id: i32,
    pub app_uuid: String,
    pub app_name: String,
    pub control: AppControl,
    pub role: AppRole,
    pub monitor_resume: bool,
    pub small_stream_input_only: bool,
    pub mic_only: bool,
}

impl LaunchTarget {
    pub fn matches_running_app(&self, running_id: i32, running_uuid: Option<&str>) -> bool {
        if let Some(running_uuid) = running_uuid.filter(|u| !u.is_empty())
            && !self.app_uuid.is_empty()
        {
            return self.app_uuid.eq_ignore_ascii_case(running_uuid);
        }
        self.app_id > 0 && self.app_id == running_id
    }

    pub fn is_control_action(&self) -> bool {
        self.control != AppControl::None
    }

    pub fn is_initialized(&self) -> bool {
        self.app_id > 0 || !self.app_uuid.is_empty()
    }

    pub fn effective_role(&self) -> AppRole {
        if self.mic_only {
            AppRole::InputOnly
        } else if self.small_stream_input_only {
            AppRole::Stream
        } else {
            self.role
        }
    }
}

/// Transport abstraction allowing shared launch orchestration over either a direct
/// [`HostClient`] (Desktop & Rust tests) or an Android `NvHTTP` callback facade.
pub trait LaunchTransport {
    fn server_info(&self, fast: bool) -> Result<ServerInfo, http::Error>;
    fn app_list(&self, caps: &HostCapabilities) -> Result<Vec<App>, http::Error>;
    fn stream_move(&self) -> Result<StreamMove, http::Error>;
    fn has_ended_session(&self) -> bool;
    fn launch(
        &self,
        verb: &str,
        app_id: i32,
        hdr: bool,
        confirm_replace: bool,
    ) -> Result<Option<Launched>, http::Error>;
    fn quit_app(&self) -> Result<bool, http::Error>;
    fn is_stopped(&self) -> bool {
        false
    }
}

/// Direct [`HostClient`] adapter for [`LaunchTransport`].
pub struct HostClientLaunch<'a> {
    pub client: &'a HostClient,
    pub request: LaunchRequest,
    pub info: &'a ServerInfo,
}

impl LaunchTransport for HostClientLaunch<'_> {
    fn server_info(&self, fast: bool) -> Result<ServerInfo, http::Error> {
        self.client.server_info(fast)
    }

    fn app_list(&self, caps: &HostCapabilities) -> Result<Vec<App>, http::Error> {
        self.client.app_list(caps)
    }

    fn stream_move(&self) -> Result<StreamMove, http::Error> {
        self.client.stream_move()
    }

    fn has_ended_session(&self) -> bool {
        false
    }

    fn launch(
        &self,
        verb: &str,
        app_id: i32,
        hdr: bool,
        confirm_replace: bool,
    ) -> Result<Option<Launched>, http::Error> {
        let mut req = self.request.clone();
        req.app_id = app_id;
        req.hdr = hdr;
        req.confirm_replace = confirm_replace;
        if verb == "resume" {
            self.client.resume(&req, self.info)
        } else {
            self.client.launch(&req, self.info)
        }
    }

    fn quit_app(&self) -> Result<bool, http::Error> {
        self.client.quit_app()
    }
}

impl HostClient {
    /// Convenience alias for [`HostClient::cancel`] matching the launch flow vocabulary.
    pub fn quit_app(&self) -> Result<bool, http::Error> {
        self.cancel()
    }
}

/// Resolves `target` against `info` and `/applist` (UUID priority, `LAUNCH_BY_UUID`,
/// name fallback, and `Control::Resume` role resolution).
pub fn resolve_launch_target(
    transport: &dyn LaunchTransport,
    info: &ServerInfo,
    target: &mut LaunchTarget,
    join: bool,
) -> Result<(), ResolveError> {
    let mut cached_apps: Option<Vec<App>> = None;
    let mut load_apps = |caps: &HostCapabilities| -> Result<Vec<App>, http::Error> {
        if let Some(apps) = &cached_apps {
            return Ok(apps.clone());
        }
        let apps = transport.app_list(caps)?;
        cached_apps = Some(apps.clone());
        Ok(apps)
    };

    if !join && !target.app_uuid.is_empty() {
        if info.current_game != 0
            && target.matches_running_app(info.current_game, info.current_game_uuid.as_deref())
        {
            target.app_id = info.current_game;
        } else {
            let found = if info.capabilities.has_permission(PERMISSION_LIST) {
                load_apps(&info.capabilities)?
                    .into_iter()
                    .find(|a| a.uuid.eq_ignore_ascii_case(&target.app_uuid))
            } else {
                None
            };
            if let Some(app) = found {
                target.app_id = app.id;
                target.monitor_resume = app.role(false) == AppRole::RemoteMonitor;
            } else if info.capabilities.has(Feature::LaunchByUuid) {
                target.app_id = 0;
            }
        }
    }

    if !target.is_initialized() {
        let found = load_apps(&info.capabilities)?
            .into_iter()
            .find(|a| a.name == target.app_name)
            .ok_or(ResolveError::AppMissing)?;
        target.app_id = found.id;
        target.monitor_resume = found.role(false) == AppRole::RemoteMonitor;
        target.app_uuid = found.uuid;
    }

    if target.control == AppControl::Resume {
        let found = if target.app_uuid.is_empty() {
            load_apps(&info.capabilities)?
                .into_iter()
                .find(|a| a.id == target.app_id)
        } else {
            load_apps(&info.capabilities)?
                .into_iter()
                .find(|a| a.uuid.eq_ignore_ascii_case(&target.app_uuid))
        }
        .ok_or(ResolveError::ResumeMissing)?;
        target.monitor_resume = found.role(false) == AppRole::RemoteMonitor;
    }

    Ok(())
}

#[derive(Debug)]
pub enum ResolveError {
    AppMissing,
    ResumeMissing,
    Http(http::Error),
}

impl From<http::Error> for ResolveError {
    fn from(value: http::Error) -> Self {
        Self::Http(value)
    }
}

/// Checks whether a resume target moved to another device before `/resume` is called.
pub fn resume_moved_to(
    paired: bool,
    running: bool,
    offered: bool,
    read: impl FnOnce() -> Result<StreamMove, http::Error>,
) -> Result<Option<String>, ConnectError> {
    if !paired || running || !offered {
        return Ok(None);
    }
    match read() {
        Ok(read) => Ok(read.moved_to.map(|moved| moved.device)),
        Err(error) => {
            eprintln!("nightjar: recovery: could not ask the PC whether the stream moved: {error}");
            Err(ConnectError {
                http_cause: http_cause_key(&error),
                message: format!("Checking the running app failed: {error}"),
                retryable: true,
                asks_replace: false,
                moved_to: None,
                packed_refused: false,
                start_cause: None,
                http_code: error.http_status().map(i32::from),
                local_failure: None,
                session_ended: false,
            })
        }
    }
}

/// Checks resume ownership/state before sending `/resume` to the host.
pub fn check_resume(
    join: bool,
    paired: bool,
    running: bool,
    offered: bool,
    read: impl FnOnce() -> Result<StreamMove, http::Error>,
) -> Result<(), ConnectError> {
    if !join && let Some(device) = resume_moved_to(paired, running, offered, read)? {
        return Err(ConnectError {
            message: String::new(),
            retryable: false,
            asks_replace: false,
            moved_to: Some(device),
            packed_refused: false,
            http_cause: None,
            start_cause: None,
            http_code: None,
            local_failure: None,
            session_ended: false,
        });
    }
    if !paired || !(running || join) {
        return Err(ConnectError::local(LocalLaunchFailure::SessionChanged));
    }
    Ok(())
}

/// Parameters controlling [`execute_launch_or_resume`].
#[derive(Clone, Copy, Debug)]
pub struct LaunchFlowOptions {
    pub join_running: bool,
    pub resume_only: bool,
    pub allow_idle_session_end: bool,
    pub never_replace: bool,
    pub hdr: bool,
}

/// Outcome of [`execute_launch_or_resume`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaunchOutcome {
    Started {
        launched: Launched,
        used_verb: String,
    },
    ActionCompleted(String),
    StoppedAutomaticStart,
}

/// Failure modes specific to the shared launch/resume flow.
#[derive(Debug)]
pub enum LaunchFlowError {
    SessionEnded,
    MovedTo(String),
    Local(LocalLaunchFailure),
    HostStatus { code: i32, message: String },
    Http(http::Error),
    Declined { verb: String },
    QuitFailed,
    SessionOwnerDenied,
    AppMinimized,
    InputOnlyOtherGame,
}

fn is_confirmation_required(target: &LaunchTarget, error: &http::Error) -> bool {
    if target.control == AppControl::Terminate {
        asks_to_confirm_terminate(error)
    } else if target.control == AppControl::None && !target.mic_only {
        asks_to_confirm_replace(error)
    } else {
        false
    }
}

fn is_action_completed(target: &LaunchTarget, error: &http::Error) -> bool {
    match target.control {
        AppControl::DisconnectMonitor => is_role_completed(AppRole::RemoteMonitor, error),
        AppControl::DisconnectInput => is_role_completed(AppRole::InputOnly, error),
        AppControl::Terminate => is_terminate_completed(error),
        _ => false,
    }
}

/// Executes `/launch` or `/resume` with 410 replace/terminate confirmation,
/// `never_replace` guard, and `ContinueHere` resume refusal checks.
pub fn launch_confirming(
    transport: &dyn LaunchTransport,
    target: &LaunchTarget,
    opts: LaunchFlowOptions,
    verb: &str,
    confirm: &dyn ConfirmReplace,
) -> Result<LaunchOutcome, LaunchFlowError> {
    if opts.resume_only && verb != "resume" {
        return Err(LaunchFlowError::Local(LocalLaunchFailure::RecoveryOnly));
    }
    let mut retried = false;
    let mut confirm_replace = false;
    loop {
        match transport.launch(verb, target.app_id, opts.hdr, confirm_replace) {
            Ok(Some(launched)) => {
                return Ok(LaunchOutcome::Started {
                    launched,
                    used_verb: verb.to_owned(),
                });
            }
            Ok(None) => {
                return Err(LaunchFlowError::Declined {
                    verb: verb.to_owned(),
                });
            }
            Err(err) => {
                if is_action_completed(target, &err) {
                    let msg = match &err {
                        http::Error::Status { message, .. } => message.clone(),
                        _ => String::new(),
                    };
                    return Ok(LaunchOutcome::ActionCompleted(msg));
                }
                if verb == "resume"
                    && let http::Error::Status { code, message } = &err
                    && let Some(refusal) =
                        continue_here::resume_refusal(opts.join_running, *code, message)
                {
                    let kind = match refusal {
                        ResumeRefusal::SessionChanged => LocalLaunchFailure::SessionChanged,
                        ResumeRefusal::BrowserStream => LocalLaunchFailure::BrowserStream,
                    };
                    return Err(LaunchFlowError::Local(kind));
                }
                if opts.resume_only || opts.join_running || !is_confirmation_required(target, &err)
                {
                    return Err(match err {
                        http::Error::Status { code, message } => {
                            LaunchFlowError::HostStatus { code, message }
                        }
                        other => LaunchFlowError::Http(other),
                    });
                }
                if opts.never_replace {
                    return Ok(LaunchOutcome::StoppedAutomaticStart);
                }
                if retried {
                    return Err(LaunchFlowError::Local(LocalLaunchFailure::ConfirmRejected));
                }
                match confirm.confirm_replace(target.control == AppControl::Terminate) {
                    ConfirmChoice::Confirmed => {}
                    ConfirmChoice::TimedOut => {
                        return Err(LaunchFlowError::Local(LocalLaunchFailure::ConfirmTimedOut));
                    }
                    ConfirmChoice::Rejected
                    | ConfirmChoice::Cancelled
                    | ConfirmChoice::Interrupted => {
                        return Err(LaunchFlowError::Local(LocalLaunchFailure::ConfirmCancelled));
                    }
                }
                confirm_replace = target.control != AppControl::Terminate;
                retried = true;
            }
        }
    }
}

/// Executes the full `launchOrResume` / `quitAndLaunch` orchestration matching `NvConnection`.
pub fn execute_launch_or_resume(
    transport: &dyn LaunchTransport,
    info: &ServerInfo,
    target: &LaunchTarget,
    opts: LaunchFlowOptions,
    confirm: &dyn ConfirmReplace,
) -> Result<LaunchOutcome, LaunchFlowError> {
    let beside_running = target.is_control_action() || target.mic_only;
    if !opts.join_running && beside_running {
        return launch_confirming(transport, target, opts, "launch", confirm);
    }

    if opts.join_running || info.current_game != 0 {
        if opts.join_running
            || target.matches_running_app(info.current_game, info.current_game_uuid.as_deref())
        {
            match launch_confirming(transport, target, opts, "resume", confirm) {
                Ok(outcome) => return Ok(outcome),
                Err(LaunchFlowError::Declined { .. }) => {
                    if opts.resume_only
                        && opts.allow_idle_session_end
                        && transport.has_ended_session()
                    {
                        return Err(LaunchFlowError::SessionEnded);
                    }
                    return Err(LaunchFlowError::Declined {
                        verb: "resume".to_owned(),
                    });
                }
                Err(LaunchFlowError::HostStatus { code: 503, message }) => {
                    if opts.resume_only
                        && opts.allow_idle_session_end
                        && transport.has_ended_session()
                    {
                        return Err(LaunchFlowError::SessionEnded);
                    }
                    return Err(LaunchFlowError::HostStatus { code: 503, message });
                }
                Err(LaunchFlowError::HostStatus { code: 470, .. }) => {
                    return Err(LaunchFlowError::SessionOwnerDenied);
                }
                Err(LaunchFlowError::HostStatus { code: 525, .. }) => {
                    return Err(LaunchFlowError::AppMinimized);
                }
                Err(err) => return Err(err),
            }
        } else if target.small_stream_input_only {
            return Err(LaunchFlowError::InputOnlyOtherGame);
        } else if info.capabilities.has(Feature::HostConfirmsReplacement) {
            return launch_confirming(transport, target, opts, "launch", confirm);
        } else if opts.never_replace && !opts.resume_only {
            return Ok(LaunchOutcome::StoppedAutomaticStart);
        } else {
            return quit_and_launch(transport, target, opts, confirm);
        }
    }

    launch_confirming(transport, target, opts, "launch", confirm)
}

/// Quits the running app and launches `target` (matching `NvConnection.quitAndLaunch`).
pub fn quit_and_launch(
    transport: &dyn LaunchTransport,
    target: &LaunchTarget,
    opts: LaunchFlowOptions,
    confirm: &dyn ConfirmReplace,
) -> Result<LaunchOutcome, LaunchFlowError> {
    if opts.resume_only {
        return Err(LaunchFlowError::Local(LocalLaunchFailure::SessionChanged));
    }
    if target.control != AppControl::None {
        return launch_confirming(transport, target, opts, "launch", confirm);
    }
    match transport.quit_app() {
        Ok(true) => {}
        Ok(false) => return Err(LaunchFlowError::QuitFailed),
        Err(http::Error::Status { code: 599, .. }) => {
            return Err(LaunchFlowError::SessionOwnerDenied);
        }
        Err(http::Error::Status { code, message }) => {
            return Err(LaunchFlowError::HostStatus { code, message });
        }
        Err(other) => return Err(LaunchFlowError::Http(other)),
    }
    if transport.is_stopped() {
        return Err(LaunchFlowError::Declined {
            verb: "launch".to_owned(),
        });
    }
    launch_confirming(transport, target, opts, "launch", confirm)
}

/// Classifies an HTTP error from launch/resume as retryable for automatic recovery.
pub fn is_retryable_http_error(error: &http::Error) -> bool {
    use std::io::ErrorKind;
    matches!(
        error,
        http::Error::Io(e)
            if matches!(
                e.kind(),
                ErrorKind::TimedOut
                    | ErrorKind::WouldBlock
                    | ErrorKind::ConnectionReset
                    | ErrorKind::ConnectionAborted
                    | ErrorKind::ConnectionRefused
                    | ErrorKind::NotConnected
                    | ErrorKind::BrokenPipe
                    | ErrorKind::UnexpectedEof
                    | ErrorKind::NetworkDown
                    | ErrorKind::NetworkUnreachable
                    | ErrorKind::HostUnreachable
            )
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct FakeTransport {
        calls: RefCell<Vec<(String, bool)>>,
        results: RefCell<Vec<Result<Option<Launched>, http::Error>>>,
        quit_result: Result<bool, http::Error>,
        ended: bool,
    }

    impl LaunchTransport for FakeTransport {
        fn server_info(&self, _: bool) -> Result<ServerInfo, http::Error> {
            Err(http::Error::Cancelled)
        }
        fn app_list(&self, _: &HostCapabilities) -> Result<Vec<App>, http::Error> {
            Ok(Vec::new())
        }
        fn stream_move(&self) -> Result<StreamMove, http::Error> {
            Ok(StreamMove::default())
        }
        fn has_ended_session(&self) -> bool {
            self.ended
        }
        fn launch(
            &self,
            verb: &str,
            _: i32,
            _: bool,
            confirm_replace: bool,
        ) -> Result<Option<Launched>, http::Error> {
            self.calls
                .borrow_mut()
                .push((verb.to_owned(), confirm_replace));
            self.results.borrow_mut().remove(0)
        }
        fn quit_app(&self) -> Result<bool, http::Error> {
            match &self.quit_result {
                Ok(b) => Ok(*b),
                Err(_) => Err(http::Error::Status {
                    code: 599,
                    message: String::new(),
                }),
            }
        }
    }

    struct StubConfirm(ConfirmChoice);
    impl ConfirmReplace for StubConfirm {
        fn confirm_replace(&self, _: bool) -> ConfirmChoice {
            self.0
        }
    }

    fn sample_target() -> LaunchTarget {
        LaunchTarget {
            app_id: 73,
            app_uuid: "uuid-73".into(),
            app_name: "Game".into(),
            control: AppControl::None,
            role: AppRole::Stream,
            monitor_resume: false,
            small_stream_input_only: false,
            mic_only: false,
        }
    }

    fn parse_info(current_game: i32) -> ServerInfo {
        let state = if current_game != 0 {
            "SUNSHINE_SERVER_BUSY"
        } else {
            "SUNSHINE_SERVER_FREE"
        };
        let xml = format!(
            r#"<?xml version="1.0" encoding="utf-8"?><root status_code="200"><uniqueid>uid</uniqueid><state>{state}</state><PairStatus>1</PairStatus><currentgame>{current_game}</currentgame></root>"#
        );
        ServerInfo::parse(&xml, 47989).unwrap()
    }

    #[test]
    fn never_replace_stops_automatic_start_without_launching_or_quitting() {
        let transport = FakeTransport {
            calls: RefCell::new(Vec::new()),
            results: RefCell::new(Vec::new()),
            quit_result: Ok(true),
            ended: false,
        };
        let info = parse_info(99);
        let opts = LaunchFlowOptions {
            join_running: false,
            resume_only: false,
            allow_idle_session_end: false,
            never_replace: true,
            hdr: false,
        };
        let out = execute_launch_or_resume(
            &transport,
            &info,
            &sample_target(),
            opts,
            &StubConfirm(ConfirmChoice::Confirmed),
        )
        .unwrap();
        assert_eq!(out, LaunchOutcome::StoppedAutomaticStart);
        assert!(transport.calls.borrow().is_empty());
    }

    #[test]
    fn confirm_replace_410_retries_once_with_confirm_flag() {
        let replace_prompt = "An app is already running. Launch this app again within 60 seconds to confirm that you want to close it.";
        let transport = FakeTransport {
            calls: RefCell::new(Vec::new()),
            results: RefCell::new(vec![
                Err(http::Error::Status {
                    code: 410,
                    message: replace_prompt.into(),
                }),
                Ok(Some(Launched {
                    rtsp_session_url: Some("rtsp://127.0.0.1".into()),
                    warning: None,
                })),
            ]),
            quit_result: Ok(true),
            ended: false,
        };
        let info = parse_info(0);
        let opts = LaunchFlowOptions {
            join_running: false,
            resume_only: false,
            allow_idle_session_end: false,
            never_replace: false,
            hdr: false,
        };
        let out = execute_launch_or_resume(
            &transport,
            &info,
            &sample_target(),
            opts,
            &StubConfirm(ConfirmChoice::Confirmed),
        )
        .unwrap();
        assert!(matches!(out, LaunchOutcome::Started { .. }));
        assert_eq!(
            *transport.calls.borrow(),
            vec![("launch".into(), false), ("launch".into(), true)]
        );
    }

    #[test]
    fn cancelled_confirmation_is_local_499_failure() {
        let replace_prompt = "An app is already running. Launch this app again within 60 seconds to confirm that you want to close it.";
        let transport = FakeTransport {
            calls: RefCell::new(Vec::new()),
            results: RefCell::new(vec![Err(http::Error::Status {
                code: 410,
                message: replace_prompt.into(),
            })]),
            quit_result: Ok(true),
            ended: false,
        };
        let info = parse_info(0);
        let opts = LaunchFlowOptions {
            join_running: false,
            resume_only: false,
            allow_idle_session_end: false,
            never_replace: false,
            hdr: false,
        };
        let err = execute_launch_or_resume(
            &transport,
            &info,
            &sample_target(),
            opts,
            &StubConfirm(ConfirmChoice::Cancelled),
        )
        .unwrap_err();
        assert!(matches!(
            err,
            LaunchFlowError::Local(LocalLaunchFailure::ConfirmCancelled)
        ));
    }

    #[test]
    fn would_block_http_error_is_retryable() {
        let err = http::Error::Io(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "temporarily unavailable",
        ));
        assert!(is_retryable_http_error(&err));
    }
}
