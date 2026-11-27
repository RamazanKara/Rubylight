//! Shared post-connect session actions: isolated cancellable hardware metrics
//! polling, throttled server commands, clipboard text/image, bitrate, quit,
//! native session actions, and stream-move queries.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use nightjar_policy::http::{AppRole, Feature, HostCapabilities, ServerInfo};
use nightjar_policy::overlay::{HardwareAnswer, HardwarePoll};
use nightjar_policy::permissions::{
    StreamPermissionContext, can_change_bitrate, can_disconnect_remote_session,
    can_quit_or_control_session, can_read_clipboard, can_run_server_command, can_write_clipboard,
    server_command_throttle_allows,
};

use crate::http::{
    self, HardwareMetrics, HostApiError, HostClient, HostClientCanceller, ImageClipboardError,
    native_api::session::{ActionId, Session as NativeSession},
};

/// Isolated, cancellable hardware-metrics poller driven by [`HardwarePoll`].
///
/// Stopping or hiding the overlay cancels only the telemetry [`HostClientCanceller`],
/// never touching the command [`HostClient`]. Terminal host refusals (403, 404, 409,
/// or unsupported schema version) permanently disable polling for this session.
pub struct HardwareMetricsPoller {
    poll: Mutex<HardwarePoll>,
    canceller: Mutex<Option<HostClientCanceller>>,
    permanently_unavailable: AtomicBool,
    stopped: AtomicBool,
}

impl Default for HardwareMetricsPoller {
    fn default() -> Self {
        Self::new()
    }
}

impl HardwareMetricsPoller {
    pub fn new() -> Self {
        Self {
            poll: Mutex::new(HardwarePoll::default()),
            canceller: Mutex::new(None),
            permanently_unavailable: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
        }
    }

    /// Cancels any in-flight metrics HTTP call and resets the active poll generation.
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
        if let Some(cancel) = self
            .canceller
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
        {
            cancel.cancel();
        }
        self.poll.lock().unwrap_or_else(|p| p.into_inner()).stop();
    }

    pub fn is_unavailable(&self) -> bool {
        self.permanently_unavailable.load(Ordering::Acquire)
            || self
                .poll
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .unavailable()
    }

    pub fn set_canceller(&self, canceller: HostClientCanceller) {
        *self.canceller.lock().unwrap_or_else(|p| p.into_inner()) = Some(canceller);
    }

    /// Checks whether a request is due at `now_ms`. Returns `Some(generation)` when due.
    pub fn due(&self, now_ms: u64, wanted: bool) -> Option<u32> {
        if self.permanently_unavailable.load(Ordering::Acquire) {
            return None;
        }
        if wanted {
            self.stopped.store(false, Ordering::Release);
        }
        self.poll
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .due(now_ms, wanted)
    }

    /// Records a host answer (parsed metrics or classified failure) for `generation`.
    /// Returns `(should_show, next_delay_ms, permanently_unavailable)`.
    pub fn finish_step(
        &self,
        generation: u32,
        now_ms: u64,
        result: Result<HardwareMetrics, HardwareStepError>,
    ) -> (bool, u64, bool) {
        self.finish_step_sample(
            generation,
            now_ms,
            result.map(|m| (m.sampled_at_unix_ms, m.interval_ms)),
        )
    }

    /// Variant of [`HardwareMetricsPoller::finish_step`] taking `(sampled_at_unix_ms, interval_ms)`.
    pub fn finish_step_sample(
        &self,
        generation: u32,
        now_ms: u64,
        result: Result<(i64, u32), HardwareStepError>,
    ) -> (bool, u64, bool) {
        let mut poll = self.poll.lock().unwrap_or_else(|p| p.into_inner());
        let (answer, delay) = match result {
            Ok((sampled_at_unix_ms, interval_ms)) => (
                HardwareAnswer::Sample {
                    sampled_at_unix_ms,
                    interval_ms,
                },
                u64::from(interval_ms).max(HardwarePoll::MIN_INTERVAL_MS),
            ),
            Err(HardwareStepError::Unavailable) => {
                self.permanently_unavailable.store(true, Ordering::Release);
                (HardwareAnswer::Unavailable, HardwarePoll::RETRY_MS)
            }
            Err(HardwareStepError::Failed) => (HardwareAnswer::Failed, HardwarePoll::RETRY_MS),
        };
        let show = poll.answered(generation, now_ms, answer);
        let unavail = self.permanently_unavailable.load(Ordering::Acquire) || poll.unavailable();
        if unavail {
            self.permanently_unavailable.store(true, Ordering::Release);
        }
        (show, delay, unavail)
    }

    /// Executes one synchronous fetch against `client` and feeds [`finish_step`].
    pub fn poll_once(
        &self,
        client: &Arc<HostClient>,
        capabilities: &HostCapabilities,
        generation: u32,
        now_ms: u64,
    ) -> (Option<HardwareMetrics>, u64, bool) {
        if self.stopped.load(Ordering::Acquire) {
            return (
                None,
                HardwarePoll::RETRY_MS,
                self.is_unavailable(),
            );
        }
        self.set_canceller(client.canceller());
        if self.stopped.load(Ordering::Acquire) {
            *self.canceller.lock().unwrap_or_else(|p| p.into_inner()) = None;
            return (
                None,
                HardwarePoll::RETRY_MS,
                self.is_unavailable(),
            );
        }
        let fetched = client.hardware_metrics(capabilities);
        *self.canceller.lock().unwrap_or_else(|p| p.into_inner()) = None;
        match fetched {
            Ok(metrics) => {
                let (show, delay, unavail) =
                    self.finish_step(generation, now_ms, Ok(metrics.clone()));
                (show.then_some(metrics), delay, unavail)
            }
            Err(err) => {
                let kind = classify_hardware_error(&err);
                let (_, delay, unavail) = self.finish_step(generation, now_ms, Err(kind));
                (None, delay, unavail)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HardwareStepError {
    Failed,
    Unavailable,
}

/// Classifies a [`HostApiError`] from `/api/hardware-metrics` (403/404/409/UnsupportedVersion
/// are permanent; transient I/O errors retry).
pub fn classify_hardware_error(err: &HostApiError) -> HardwareStepError {
    match err {
        HostApiError::UnsupportedVersion => HardwareStepError::Unavailable,
        HostApiError::Host(http::Error::NotFound(_)) => HardwareStepError::Unavailable,
        HostApiError::Host(http::Error::Status {
            code: 403 | 404 | 409,
            ..
        }) => HardwareStepError::Unavailable,
        HostApiError::Host(http::Error::Api {
            status: 403 | 404 | 409,
            ..
        }) => HardwareStepError::Unavailable,
        _ => HardwareStepError::Failed,
    }
}

/// Checks whether an ended session is confirmed by a paired host's serverinfo.
pub fn serverinfo_confirms_ended_session(
    stop_requested: bool,
    role: AppRole,
    is_control_action: bool,
    info: &ServerInfo,
) -> bool {
    if stop_requested || role != AppRole::Stream || is_control_action {
        return false;
    }
    info.paired
        && info.capabilities.has_permission(
            nightjar_policy::http::PERMISSION_VIEW | nightjar_policy::http::PERMISSION_LAUNCH,
        )
        && info.session_idle
        && info.current_game == 0
        && info.current_game_uuid.as_deref().is_none_or(str::is_empty)
}

/// Validates and dispatches a gated host clipboard text write.
pub fn send_clipboard_text(
    client: &HostClient,
    ctx: &StreamPermissionContext,
    text: &str,
) -> Result<(), http::Error> {
    if !can_write_clipboard(ctx) {
        return Err(http::Error::InvalidArgument(
            "Host clipboard write permission denied".into(),
        ));
    }
    if ctx.is_monkey {
        return Ok(());
    }
    client.send_clipboard(text)
}

/// Validates and dispatches a gated host clipboard text read.
pub fn read_clipboard_text(
    client: &HostClient,
    ctx: &StreamPermissionContext,
) -> Result<String, http::Error> {
    if !can_read_clipboard(ctx) {
        return Err(http::Error::InvalidArgument(
            "Host clipboard read permission denied".into(),
        ));
    }
    client.clipboard()
}

/// Validates PNG header + permissions before sending a clipboard image.
pub fn send_clipboard_image_bytes(
    client: &HostClient,
    ctx: &StreamPermissionContext,
    native_image_clipboard: bool,
    png_bytes: &[u8],
) -> Result<(), ImageClipboardError> {
    if !native_image_clipboard {
        return Err(ImageClipboardError::Forbidden);
    }
    if !can_write_clipboard(ctx) {
        return Err(ImageClipboardError::Forbidden);
    }
    let _ = http::png_size(png_bytes)?;
    if ctx.is_monkey {
        return Ok(());
    }
    client.send_clipboard_image(png_bytes)
}

/// Validates permissions and changes runtime bitrate via `HostClient`.
pub fn set_runtime_bitrate(
    client: &HostClient,
    ctx: &StreamPermissionContext,
    supports_runtime_bitrate: bool,
    kbps: u32,
) -> Result<u32, http::Error> {
    if !can_change_bitrate(ctx, supports_runtime_bitrate) {
        return Err(http::Error::InvalidArgument(
            "Host does not allow runtime bitrate changes".into(),
        ));
    }
    client.set_bitrate(kbps)
}

/// Validates permissions and runs `/cancel` via `HostClient`.
pub fn quit_app(client: &HostClient, ctx: &StreamPermissionContext) -> Result<(), http::Error> {
    if !can_quit_or_control_session(ctx) {
        return Err(http::Error::InvalidArgument(
            "The host denied permission to quit this app.".into(),
        ));
    }
    if !client.cancel()? {
        return Err(http::Error::Malformed(
            "The host did not quit the app.".into(),
        ));
    }
    Ok(())
}

/// Validates permissions and disconnects a remote session role via `/launch`.
pub fn disconnect_remote_session(
    client: &HostClient,
    ctx: &StreamPermissionContext,
    info: &ServerInfo,
) -> Result<(), http::Error> {
    if !can_disconnect_remote_session(ctx) {
        return Err(http::Error::InvalidArgument(
            "The host denied permission to end this remote session.".into(),
        ));
    }
    if !info.capabilities.may_have(Feature::RemoteSessionTiles) {
        return Ok(());
    }
    let app_role = match ctx.role {
        nightjar_policy::launch::Role::Stream => AppRole::Stream,
        nightjar_policy::launch::Role::RemoteMonitor => AppRole::RemoteMonitor,
        nightjar_policy::launch::Role::InputOnly => AppRole::InputOnly,
    };
    if !client.disconnect_role(app_role)? {
        return Err(http::Error::Malformed(
            "The host did not end the remote session.".into(),
        ));
    }
    Ok(())
}

/// Runs a gated native session action (`Disconnect`, `End`, `CloseGame`).
pub fn run_native_session_action(
    client: &HostClient,
    ctx: &StreamPermissionContext,
    info: &ServerInfo,
    action: ActionId,
) -> Result<NativeSession, http::Error> {
    if !can_quit_or_control_session(ctx) {
        return Err(http::Error::InvalidArgument(
            "The host denied permission to change this session.".into(),
        ));
    }
    if !info.capabilities.has(Feature::NativeApi) {
        return Err(http::Error::InvalidArgument(
            "The host has no native session actions.".into(),
        ));
    }
    client.session_action(action)
}

/// Validates server-command permissions, command name parity, and 1-second throttle.
pub fn validate_server_command(
    ctx: &StreamPermissionContext,
    server_commands: &[String],
    index: usize,
    expected_name: &str,
    last_sent_ms: Option<u64>,
    now_ms: u64,
) -> Result<(), &'static str> {
    let matches_name = server_commands
        .get(index)
        .is_some_and(|actual| actual == expected_name);
    if !can_run_server_command(ctx, index, server_commands.len(), matches_name) {
        return Err("Server command changed or permission denied. Reopen the Control Center.");
    }
    if !server_command_throttle_allows(last_sent_ms, now_ms) {
        return Err("Wait one second between server commands");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use nightjar_policy::launch::Role;

    #[test]
    fn hardware_metrics_poller_cancels_stale_and_latches_403_404_409() {
        let poller = HardwareMetricsPoller::new();
        let gen1 = poller.due(1000, true).expect("first due");
        poller.stop();
        // Stale answer for gen1 must be ignored.
        let (show, _, unavail) = poller.finish_step(
            gen1,
            1100,
            Ok(HardwareMetrics {
                sampled_at_unix_ms: 500,
                interval_ms: 2000,
                cpu: Default::default(),
                gpus: Vec::new(),
            }),
        );
        assert!(!show);
        assert!(!unavail);

        // New generation receives 404 -> permanently unavailable.
        let gen2 = poller.due(2000, true).expect("second due");
        let (show2, _, unavail2) =
            poller.finish_step(gen2, 2100, Err(HardwareStepError::Unavailable));
        assert!(!show2);
        assert!(unavail2);
        assert!(poller.due(50_000, true).is_none());
    }

    #[test]
    fn server_command_validation_checks_name_and_one_second_throttle() {
        let ctx = StreamPermissionContext {
            stop_requested: false,
            is_monkey: false,
            role: Role::Stream,
            paired: true,
            permission: i64::from(nightjar_policy::http::PERMISSION_SERVER_COMMAND),
        };
        let cmds = vec!["Sleep".to_owned()];
        assert!(validate_server_command(&ctx, &cmds, 0, "Sleep", None, 1000).is_ok());
        assert!(validate_server_command(&ctx, &cmds, 0, "Other", None, 1000).is_err());
        assert!(validate_server_command(&ctx, &cmds, 0, "Sleep", Some(1000), 1500).is_err());
        assert!(validate_server_command(&ctx, &cmds, 0, "Sleep", Some(1000), 2000).is_ok());
    }
}
