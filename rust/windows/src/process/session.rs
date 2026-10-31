//! Windows sessions: the console, the user's, and reattaching a disconnected console.

use super::*;

/// The session attached to the physical console.
pub fn console_session() -> u32 {
    // SAFETY: WTSGetActiveConsoleSessionId takes no arguments and has no preconditions.
    unsafe { windows::Win32::System::RemoteDesktop::WTSGetActiveConsoleSessionId() }
}
/// This process's own session; the service starts the host in the user's.
pub fn current_session() -> Result<u32> {
    // SAFETY: GetCurrentProcessId has no preconditions.
    process_session(unsafe { GetCurrentProcessId() })
}
pub fn process_session(pid: u32) -> Result<u32> {
    let mut session = 0;
    // SAFETY: `session` is a live local the call writes once.
    unsafe { ProcessIdToSessionId(pid, &mut session)? };
    Ok(session)
}
pub fn ensure_streaming_session() -> Result<()> {
    ensure_streaming_session_with(
        current_session,
        console_session,
        session_connection,
        disconnected_user_sessions,
    )
}
pub(super) fn ensure_streaming_session_with(
    current: impl FnOnce() -> Result<u32>,
    console: impl FnOnce() -> u32,
    connection: impl FnOnce(u32) -> Result<(WTS_CONNECTSTATE_CLASS, u16)>,
    disconnected: impl FnOnce(u32) -> Result<Vec<u32>>,
) -> Result<()> {
    let session = current()?;
    let (state, protocol) = match connection(session) {
        Ok(connection) => connection,
        Err(error) => {
            tracing::warn!(session_id = session, %error, "Windows session query failed");
            return Err(error);
        }
    };
    let console = console();
    let result = validate_streaming_session(state, session == console && protocol == 0);
    let console_exception = state != WTSActive && result.is_ok();
    let decision = if state == WTSActive {
        "active"
    } else if console_exception {
        "physical_console_sign_in_or_lock"
    } else {
        "disconnected_or_changing"
    };
    if result.is_ok() {
        tracing::info!(
            session_id = session,
            console_session_id = console,
            state = state.0,
            protocol,
            console_exception,
            allowed = true,
            decision,
            "Windows session streaming check"
        );
    } else {
        // A refused launch must be visible in a default log, with the
        // session it actually checked.
        tracing::warn!(
            session_id = session,
            console_session_id = console,
            state = state.0,
            protocol,
            console_exception,
            allowed = false,
            decision,
            "Windows session streaming check refused the launch"
        );
    }
    if console_exception {
        // Diagnostic only; it never blocks a launch. The installed service
        // runs on the console, so after Remote Desktop the host streams the
        // sign-in screen while the user's desktop waits in another session.
        match disconnected(session) {
            Ok(sessions) if !sessions.is_empty() => tracing::warn!(
                session_id = session,
                state = state.0,
                disconnected_session_ids = ?sessions,
                "Streaming the Windows sign-in or lock screen while another desktop session is disconnected. Signing in as that desktop's user makes Windows reconnect it to the console, which ends this stream; reconnect Moonlight afterwards"
            ),
            Ok(_) => {}
            Err(error) => {
                tracing::debug!(session_id = session, %error, "Cannot list disconnected Windows sessions")
            }
        }
    }
    result
}
/// Other sessions that are disconnected but still have a signed-in user.
/// Session 0 and the host's own session are never candidates.
pub(super) fn disconnected_desktops(
    host: u32,
    sessions: &[(u32, WTS_CONNECTSTATE_CLASS)],
    mut has_user: impl FnMut(u32) -> Result<bool>,
) -> Vec<u32> {
    sessions
        .iter()
        .filter(|(id, state)| *id != 0 && *id != host && *state == WTSDisconnected)
        .filter(|(id, _)| has_user(*id).unwrap_or(false))
        .map(|(id, _)| *id)
        .collect()
}
fn disconnected_user_sessions(host: u32) -> Result<Vec<u32>> {
    let mut info: *mut WTS_SESSION_INFOW = std::ptr::null_mut();
    let mut count = 0;
    // SAFETY: WTS allocates `info` with `count` entries. Only the ID and state
    // are copied out, and the buffer is freed exactly once afterwards.
    let sessions = unsafe {
        WTSEnumerateSessionsW(None, 0, 1, &mut info, &mut count)?;
        let sessions = if info.is_null() {
            Vec::new()
        } else {
            std::slice::from_raw_parts(info, count as usize)
                .iter()
                .map(|session| (session.SessionId, session.State))
                .collect::<Vec<_>>()
        };
        WTSFreeMemory(info.cast());
        sessions
    };
    Ok(disconnected_desktops(host, &sessions, session_has_user))
}
fn session_connection(session: u32) -> Result<(WTS_CONNECTSTATE_CLASS, u16)> {
    let mut buffer = PWSTR::null();
    let mut bytes = 0;
    // SAFETY: WTS allocates each output buffer. Its size is checked before reading and it is
    // freed once after copying the value, including when the returned size is invalid.
    let (state, protocol) = unsafe {
        WTSQuerySessionInformationW(None, session, WTSConnectState, &mut buffer, &mut bytes)
            .context("Cannot check the host Windows session; retry after Windows finishes switching sessions")?;
        let state = if !buffer.is_null() && bytes as usize >= size_of::<WTS_CONNECTSTATE_CLASS>() {
            Some(buffer.0.cast::<WTS_CONNECTSTATE_CLASS>().read_unaligned())
        } else {
            None
        };
        WTSFreeMemory(buffer.0.cast());
        let state = state.context("Windows returned an invalid session connection state")?;
        buffer = PWSTR::null();
        bytes = 0;
        WTSQuerySessionInformationW(None, session, WTSClientProtocolType, &mut buffer, &mut bytes)
            .context("Cannot check the host Windows session protocol; retry after Windows finishes switching sessions")?;
        let protocol = if !buffer.is_null() && bytes as usize >= size_of::<u16>() {
            Some(buffer.0.read_unaligned())
        } else {
            None
        };
        WTSFreeMemory(buffer.0.cast());
        (
            state,
            protocol.context("Windows returned an invalid session protocol")?,
        )
    };
    Ok((state, protocol))
}
pub(super) fn validate_streaming_session(
    state: WTS_CONNECTSTATE_CLASS,
    console: bool,
) -> Result<()> {
    // The physical console can be connected before sign-in or disconnected while locked.
    // Rejecting every state except WTSActive would break streaming its sign-in screen.
    if state == WTSActive || (console && (state == WTSConnected || state == WTSDisconnected)) {
        return Ok(());
    }
    bail!(
        "The host Windows session is disconnected or changing. Retry in a moment. After using RDP, use Maintenance > Windows session > Return to console, or sign in at the PC."
    )
}

#[derive(serde::Serialize)]
pub struct ConsoleRecovery {
    pub session_id: u32,
    pub console_session_id: u32,
    pub can_reattach: bool,
    pub reason: String,
}

fn session_has_user(session: u32) -> Result<bool> {
    let mut buffer = PWSTR::null();
    let mut bytes = 0;
    // SAFETY: WTS owns the returned UTF-16 buffer. Inspect only a nonempty,
    // adequately sized response, then free it exactly once.
    unsafe {
        WTSQuerySessionInformationW(None, session, WTSUserName, &mut buffer, &mut bytes)?;
        let has_user = if !buffer.is_null() && bytes >= 2 {
            Some(buffer.0.read_unaligned() != 0)
        } else {
            None
        };
        WTSFreeMemory(buffer.0.cast());
        has_user.context("Windows returned an invalid session user name")
    }
}

pub(crate) fn session_user_sid(session: u32) -> Result<String> {
    let mut token = HANDLE::default();
    // SAFETY: WTS returns an owned token for this session's signed-in user.
    // OwnedHandle closes it after reading its SID; no token is exposed or used
    // to impersonate the user. Non-SYSTEM callers normally cannot query it.
    let token = unsafe {
        WTSQueryUserToken(session, &mut token)
            .context("Cannot verify the Windows session owner")?;
        owned(token)
    };
    token_user_sid(&token)
}

pub(super) fn same_disconnected_owner(
    state: Option<WTS_CONNECTSTATE_CLASS>,
    source_sid: Option<&str>,
    target_sid: Option<&str>,
) -> bool {
    state == Some(WTSDisconnected)
        && source_sid
            .zip(target_sid)
            .is_some_and(|(source, target)| !source.is_empty() && source == target)
}

pub(super) fn verify_disconnected_console_owner(
    session: u32,
    console: u32,
    mut state: impl FnMut(u32) -> Result<WTS_CONNECTSTATE_CLASS>,
    mut user_sid: impl FnMut(u32) -> Result<String>,
    current_console: impl FnOnce() -> u32,
) -> Result<bool> {
    if state(console)? != WTSDisconnected {
        return Ok(false);
    }
    let source_sid = user_sid(session)?;
    let target_sid = user_sid(console)?;
    // Check the destination again after querying the tokens: a user
    // reconnecting while status is read must not be treated as idle.
    let target_state = state(console)?;
    Ok(current_console() == console
        && same_disconnected_owner(Some(target_state), Some(&source_sid), Some(&target_sid)))
}

pub(super) fn reattach_blocker(
    session: u32,
    console: u32,
    state: WTS_CONNECTSTATE_CLASS,
    protocol: u16,
    host_has_user: bool,
    console_has_user: bool,
    same_disconnected_user: bool,
) -> Option<&'static str> {
    if session == console && protocol == 0 && !host_has_user {
        // The installed service always runs on the console, so after Remote
        // Desktop it sits at the sign-in screen and this action cannot apply.
        Some(
            "This host runs in the PC's console session, which is at the Windows sign-in screen. Return to console only moves a host that was started inside a Remote Desktop session. Sign in through Moonlight or at the PC. If your desktop is in a disconnected Remote Desktop session, Windows reconnects it after sign-in, which ends that stream; then reconnect Moonlight.",
        )
    } else if session == console && protocol == 0 {
        Some("This host is already attached to the physical console.")
    } else if console == u32::MAX || console == 0 || session == 0 || session == console {
        Some(
            "The physical console is changing or unavailable. Retry after Windows finishes switching sessions.",
        )
    } else if protocol != 2 || (state != WTSActive && state != WTSDisconnected) || !host_has_user {
        Some("Only this host's signed-in RDP session can be returned to the console.")
    } else if console_has_user && !same_disconnected_user {
        Some(
            "The physical console has a signed-in user. Rubylight can return to it only if that session is disconnected and Windows verifies that both sessions belong to the same user. Active sessions, different users and unverifiable owners are not disconnected.",
        )
    } else {
        None
    }
}

/// Read-only eligibility; the write operation repeats every check immediately
/// before connecting. Never selects an arbitrary logged-in user's session.
pub fn console_recovery() -> Result<ConsoleRecovery> {
    let session = current_session()?;
    let console = console_session();
    let (state, protocol) = session_connection(session)?;
    let host_has_user = session_has_user(session)?;
    let console_has_user = if console == u32::MAX || console == 0 || console == session {
        true
    } else {
        session_has_user(console)?
    };
    // The existing unoccupied-console path does not need user-token access.
    // A signed-in destination gains an exception only after both owners are
    // verified from Windows tokens; matching display names are insufficient.
    let same_disconnected_user = if console_has_user
        && reattach_blocker(
            session,
            console,
            state,
            protocol,
            host_has_user,
            false,
            false,
        )
        .is_none()
    {
        let verified = verify_disconnected_console_owner(
            session,
            console,
            |session| Ok(session_connection(session)?.0),
            session_user_sid,
            console_session,
        );
        match verified {
            Ok(same) => same,
            Err(error) => {
                tracing::debug!(
                    session_id = session,
                    console_session_id = console,
                    %error,
                    "Cannot verify a disconnected console owner; keeping return to console blocked"
                );
                false
            }
        }
    } else {
        false
    };
    let reason = reattach_blocker(
        session,
        console,
        state,
        protocol,
        host_has_user,
        console_has_user,
        same_disconnected_user,
    );
    Ok(ConsoleRecovery {
        session_id: session,
        console_session_id: console,
        can_reattach: reason.is_none(),
        reason: reason.unwrap_or(if same_disconnected_user {
            "Return this RDP desktop to the PC's physical console. Its disconnected session belongs to the same Windows user. The desktop becomes visible at the PC and its RDP connection disconnects."
        } else {
            "Return this RDP desktop to the PC's unoccupied physical console. The desktop becomes visible at the PC and its RDP connection disconnects."
        }).into(),
    })
}

/// Explicit administrator action. Expected IDs bind confirmation to the
/// displayed host/console pair; no caller may supply another source session.
pub fn reattach_console(expected_session: u32, expected_console: u32) -> Result<()> {
    let status = console_recovery()?;
    if status.session_id != expected_session || status.console_session_id != expected_console {
        bail!("Windows sessions changed; refresh the session status and confirm again");
    }
    if !status.can_reattach {
        bail!("{}", status.reason);
    }
    // SAFETY: Both IDs were freshly queried; the source is this process's own
    // session and the destination has no signed-in user, or is disconnected
    // with a freshly verified matching user SID. The non-null empty password
    // asks Windows to authorize the caller, without handling credentials.
    // Return asynchronously so session-change notifications cannot deadlock
    // the service supervisor waiting for this host to finish an API request.
    unsafe {
        WTSConnectSessionW(status.session_id, status.console_session_id, windows::core::w!(""), false)
            .context("Windows refused the console connection. Run the host as the session owner or through the installed Windows service")?;
    }
    Ok(())
}
/// Whether a user is signed in to this process's session. Only a host
/// running as SYSTEM can tell; a portable host runs as the user.
pub fn user_signed_in() -> bool {
    // SAFETY: `session` and `token` are valid out-pointers, and the token returned is closed once
    // here.
    unsafe {
        let mut session = 0;
        if ProcessIdToSessionId(GetCurrentProcessId(), &mut session).is_err() {
            return false;
        }
        let mut token = HANDLE::default();
        if WTSQueryUserToken(session, &mut token).is_err() {
            return false;
        }
        let _ = CloseHandle(token);
        true
    }
}
