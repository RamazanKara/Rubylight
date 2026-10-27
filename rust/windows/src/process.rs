//! User-session process creation with ownership of the complete Windows job.

use anyhow::{Context, Result, bail};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    mem::size_of,
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::Path,
    time::Duration,
};
use windows::{
    Win32::{
        Foundation::*,
        Security::*,
        System::{Environment::*, JobObjects::*, RemoteDesktop::*, Threading::*},
    },
    core::{BOOL, PCWSTR, PWSTR},
};
fn owned(handle: HANDLE) -> OwnedHandle {
    // SAFETY: callers pass a freshly created handle they own and that nothing else closes.
    unsafe { OwnedHandle::from_raw_handle(handle.0) }
}
fn raw(handle: &OwnedHandle) -> HANDLE {
    HANDLE(handle.as_raw_handle())
}
fn wide(s: &std::ffi::OsStr) -> Result<Vec<u16>> {
    let mut value: Vec<_> = s.encode_wide().collect();
    if value.contains(&0) {
        bail!("Windows process string contains NUL");
    }
    value.push(0);
    Ok(value)
}
pub(crate) fn quote(s: &std::ffi::OsStr) -> Result<String> {
    let s = s.to_string_lossy();
    if s.contains('\0') {
        bail!("process argument contains NUL");
    }
    let mut result = String::from("\"");
    let mut slashes = 0;
    for ch in s.chars() {
        if ch == '\\' {
            slashes += 1;
            continue;
        }
        if ch == '"' {
            result.extend(std::iter::repeat_n('\\', slashes * 2 + 1));
        } else {
            result.extend(std::iter::repeat_n('\\', slashes));
        }
        slashes = 0;
        result.push(ch);
    }
    result.extend(std::iter::repeat_n('\\', slashes * 2));
    result.push('"');
    Ok(result)
}
fn token() -> Result<OwnedHandle> {
    // SAFETY: `token` is a valid out-pointer and the handle it receives is owned by the caller.
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_QUERY | TOKEN_DUPLICATE | TOKEN_ASSIGN_PRIMARY | TOKEN_ADJUST_SESSIONID,
            &mut token,
        )?;
        Ok(owned(token))
    }
}
pub fn is_system() -> bool {
    let Ok(token) = token() else { return false };
    // SAFETY: `buffer` is usize-aligned and writable for the size passed, and holds a TOKEN_USER
    // whose SID points into it once the call succeeds.
    unsafe {
        let mut buffer = [0usize; 64];
        let mut needed = 0;
        if GetTokenInformation(
            raw(&token),
            TokenUser,
            Some(buffer.as_mut_ptr().cast()),
            size_of::<[usize; 64]>() as u32,
            &mut needed,
        )
        .is_err()
        {
            return false;
        }
        let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
        IsWellKnownSid(user.User.Sid, WinLocalSystemSid).as_bool()
    }
}
/// The signed-in user's SID (`S-1-5-21-...`), whose registry hive and
/// folders hold per-user program settings. Without the service this is the
/// current user.
pub fn user_sid() -> Option<String> {
    let user_token = if is_system() {
        target_token(&Target::User { elevated: false }).ok()?
    } else {
        None
    };
    let own = token().ok()?;
    let token = user_token.as_ref().unwrap_or(&own);
    token_user_sid(token).ok()
}
/// For an elevated administrator host that is not SYSTEM: the desktop
/// shell's standard-rights token of the same user, to act as that user
/// without administrator rights. None when this host is not elevated, or
/// when the shell is elevated too (User Account Control is off). An error
/// when the shell is not running or belongs to another user, as after an
/// over-the-shoulder elevation.
pub fn standard_user_token() -> Result<Option<OwnedHandle>> {
    let own = token()?;
    if !token_elevated(&own)? {
        return Ok(None);
    }
    shell_token(&own)
}
/// The SID of the account this process runs as (`S-1-5-18` for SYSTEM), unlike [`user_sid`], which
/// names the signed-in user when this process is SYSTEM.
pub fn own_sid() -> Result<String> {
    token_user_sid(&token()?)
}
fn token_user_sid(token: &OwnedHandle) -> Result<String> {
    use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
    // SAFETY: `buffer` holds a TOKEN_USER once the call succeeds, and the string from
    // ConvertSidToStringSidW is read before it is freed once with LocalFree.
    unsafe {
        let mut buffer = [0usize; 64];
        let mut needed = 0;
        GetTokenInformation(
            raw(token),
            TokenUser,
            Some(buffer.as_mut_ptr().cast()),
            size_of::<[usize; 64]>() as u32,
            &mut needed,
        )?;
        anyhow::ensure!(
            needed as usize >= size_of::<TOKEN_USER>(),
            "Windows returned an invalid token owner"
        );
        let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
        let mut text = PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut text)?;
        let sid = text.to_string();
        let _ = LocalFree(Some(HLOCAL(text.0.cast())));
        sid.context("Windows returned an invalid user SID")
    }
}
/// Give only SYSTEM and Administrators access to `folder` and the files in
/// it, replacing inherited permissions, as Vibepollo does for the service's
/// credentials.
pub fn restrict_to_administrators(folder: &Path) -> Result<()> {
    use windows::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SE_FILE_OBJECT, SetNamedSecurityInfoW,
    };
    let sddl = wide(std::ffi::OsStr::new(
        "O:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)",
    ))?;
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: `sddl` and `path` are NUL-terminated and outlive the calls, and `dacl` points into
    // `descriptor`, which is freed once after its last use.
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(sddl.as_ptr()),
            1,
            &mut descriptor,
            None,
        )?;
        let result = (|| -> Result<()> {
            let mut present = BOOL(0);
            let mut defaulted = BOOL(0);
            let mut dacl: *mut ACL = std::ptr::null_mut();
            let mut owner = PSID::default();
            GetSecurityDescriptorOwner(descriptor, &mut owner, &mut defaulted)?;
            GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted)?;
            let mut pending = vec![folder.to_path_buf()];
            while let Some(path) = pending.pop() {
                use std::os::windows::fs::MetadataExt;
                let metadata = std::fs::symlink_metadata(&path)?;
                if metadata.file_attributes() & 0x400 != 0 {
                    bail!("{} is a reparse point", path.display());
                }
                let name = wide(path.as_os_str())?;
                SetNamedSecurityInfoW(
                    PCWSTR(name.as_ptr()),
                    SE_FILE_OBJECT,
                    OWNER_SECURITY_INFORMATION
                        | DACL_SECURITY_INFORMATION
                        | PROTECTED_DACL_SECURITY_INFORMATION,
                    Some(owner),
                    None,
                    Some(dacl),
                    None,
                )
                .ok()
                .with_context(|| format!("setting permissions on {}", path.display()))?;
                if metadata.is_dir() {
                    for entry in std::fs::read_dir(&path)? {
                        pending.push(entry?.path());
                    }
                }
            }
            Ok(())
        })();
        let _ = LocalFree(Some(HLOCAL(descriptor.0)));
        result
    }
}
pub enum Target {
    User { elevated: bool },
    SystemSession(u32),
}
pub(crate) fn token_elevated(token: &OwnedHandle) -> Result<bool> {
    let mut info = TOKEN_ELEVATION::default();
    let mut bytes = 0;
    // SAFETY: `info` is a writable TOKEN_ELEVATION and the size passed is its size.
    unsafe {
        GetTokenInformation(
            raw(token),
            TokenElevation,
            Some((&mut info as *mut TOKEN_ELEVATION).cast()),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut bytes,
        )?;
    }
    Ok(info.TokenIsElevated != 0)
}
/// The Windows desktop shell is not running, so an elevated administrator
/// host has no standard-rights token of its user. Callers match it with
/// `downcast_ref` to say what the user can do.
#[derive(Debug)]
pub struct NoShell;
impl std::fmt::Display for NoShell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Cannot start a non-elevated program from this administrator host: the Windows desktop shell is not running. Start Rubylight without administrator rights, or use the installed Windows service")
    }
}
impl std::error::Error for NoShell {}
/// The desktop shell's primary token, for starting a non-elevated program
/// from an elevated administrator process that is not SYSTEM. It must belong
/// to the same user. None when the shell is elevated itself (User Account
/// Control is off): there is no standard-rights token, and the program runs
/// with the rights the user's own desktop gives it.
fn shell_token(own: &OwnedHandle) -> Result<Option<OwnedHandle>> {
    use windows::Win32::UI::WindowsAndMessaging::{GetShellWindow, GetWindowThreadProcessId};
    // SAFETY: the shell window is only used to read its process ID. The
    // process and token handles are owned once and closed on drop, and the
    // out-pointers are live locals.
    let primary = unsafe {
        let shell = GetShellWindow();
        if shell.is_invalid() {
            return Err(NoShell.into());
        }
        let mut pid = 0;
        GetWindowThreadProcessId(shell, Some(&mut pid));
        if pid == 0 {
            return Err(NoShell.into());
        }
        let process = owned(
            OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)
                .context("Cannot open the Windows desktop shell")?,
        );
        let mut token = HANDLE::default();
        OpenProcessToken(raw(&process), TOKEN_DUPLICATE, &mut token)
            .context("Cannot read the Windows desktop shell's user token")?;
        let token = owned(token);
        let mut primary = HANDLE::default();
        DuplicateTokenEx(
            raw(&token),
            TOKEN_QUERY
                | TOKEN_DUPLICATE
                | TOKEN_IMPERSONATE
                | TOKEN_ASSIGN_PRIMARY
                | TOKEN_ADJUST_DEFAULT
                | TOKEN_ADJUST_SESSIONID,
            None,
            SecurityImpersonation,
            TokenPrimary,
            &mut primary,
        )?;
        owned(primary)
    };
    anyhow::ensure!(
        token_user_sid(&primary)? == token_user_sid(own)?,
        "Cannot start a non-elevated program: the Windows desktop shell belongs to another user"
    );
    Ok((!token_elevated(&primary)?).then_some(primary))
}
/// The user a `Target::User` program runs as: the one signed in to this
/// host's own session, never the console's. A host inside a child session
/// starts its capture worker (and games) on the child session's desktop, not
/// on the main desktop the user is working on.
fn own_session_user<T>(
    current: impl FnOnce() -> Result<u32>,
    user: impl FnOnce(u32) -> Result<T>,
) -> Result<T> {
    user(current()?)
}
fn target_token(target: &Target) -> Result<Option<OwnedHandle>> {
    if !is_system() {
        if matches!(target, Target::SystemSession(_)) {
            bail!("SYSTEM session creation requires the Windows service");
        }
        let token = token()?;
        let elevated = token_elevated(&token)?;
        if matches!(target, Target::User { elevated: true }) && !elevated {
            bail!("this elevated command requires the installed Windows service");
        }
        if matches!(target, Target::User { elevated: false }) && elevated {
            // The elevation-linked token is only an identification token for
            // a caller without SeTcbPrivilege, which Windows refuses for
            // process creation. The desktop shell holds the usable one.
            return shell_token(&token);
        }
        return Ok(None);
    }
    // SAFETY: every out-pointer is a live local whose size matches the one passed, and each
    // returned token handle is wrapped in OwnedHandle exactly once.
    unsafe {
        let source = match target {
            Target::SystemSession(_) => token()?,
            Target::User { .. } => {
                let mut user = own_session_user(current_session, |session| {
                    let mut user = HANDLE::default();
                    WTSQueryUserToken(session, &mut user)
                        .context("no signed-in user in the host session")?;
                    Ok(owned(user))
                })?;
                if matches!(target, Target::User { elevated: true }) {
                    let mut linked = TOKEN_LINKED_TOKEN::default();
                    let mut bytes = 0;
                    if GetTokenInformation(
                        raw(&user),
                        TokenLinkedToken,
                        Some((&mut linked as *mut TOKEN_LINKED_TOKEN).cast()),
                        size_of::<TOKEN_LINKED_TOKEN>() as u32,
                        &mut bytes,
                    )
                    .is_ok()
                        && !linked.LinkedToken.is_invalid()
                    {
                        user = owned(linked.LinkedToken);
                    }
                }
                user
            }
        };
        let mut primary = HANDLE::default();
        DuplicateTokenEx(
            raw(&source),
            TOKEN_ALL_ACCESS,
            None,
            SecurityImpersonation,
            TokenPrimary,
            &mut primary,
        )?;
        let primary = owned(primary);
        if let Target::SystemSession(session) = target {
            SetTokenInformation(
                raw(&primary),
                TokenSessionId,
                (session as *const u32).cast(),
                4,
            )?;
        }
        Ok(Some(primary))
    }
}
struct Environment(*mut std::ffi::c_void);
impl Drop for Environment {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: `self.0` is non-null and came from CreateEnvironmentBlock, and Drop runs
            // once.
            unsafe {
                let _ = DestroyEnvironmentBlock(self.0);
            }
        }
    }
}
/// An attribute list that restricts which handles a new process inherits.
struct AttributeList(Vec<usize>);
impl AttributeList {
    /// Make the list name exactly `handles`. The slice must outlive the process creation call.
    fn handle_list(&mut self, handles: &[HANDLE; 3]) -> Result<LPPROC_THREAD_ATTRIBUTE_LIST> {
        // SAFETY: the buffer is sized by the first call and aligned as usize, as the API needs;
        // `handles` is read during the process creation call that the caller makes while it is
        // alive, and the list is deleted once on drop.
        unsafe {
            let mut size = 0usize;
            // Fails with ERROR_INSUFFICIENT_BUFFER and the size needed; the size is what matters.
            let _ = InitializeProcThreadAttributeList(None, 1, None, &mut size);
            self.0 = vec![0usize; size.div_ceil(size_of::<usize>())];
            let list = LPPROC_THREAD_ATTRIBUTE_LIST(self.0.as_mut_ptr().cast());
            InitializeProcThreadAttributeList(Some(list), 1, None, &mut size)?;
            if let Err(e) = UpdateProcThreadAttribute(
                list,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                Some(handles.as_ptr().cast()),
                size_of::<[HANDLE; 3]>(),
                None,
                None,
            ) {
                DeleteProcThreadAttributeList(list);
                self.0.clear();
                return Err(e.into());
            }
            Ok(list)
        }
    }
}
impl Drop for AttributeList {
    fn drop(&mut self) {
        if !self.0.is_empty() {
            // SAFETY: the buffer holds a list initialized by `handle_list`, deleted here once.
            unsafe {
                DeleteProcThreadAttributeList(LPPROC_THREAD_ATTRIBUTE_LIST(
                    self.0.as_mut_ptr().cast(),
                ))
            };
        }
    }
}
/// Handles marked inheritable for as short a time as possible: made so on creation, made
/// private again on drop (the handles must outlive the guard).
struct InheritableHandles([HANDLE; 3]);
impl InheritableHandles {
    fn new(handles: [HANDLE; 3]) -> Result<Self> {
        let guard = Self(handles);
        for handle in handles {
            // SAFETY: the caller owns these handles and keeps them open while this guard lives.
            unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT.0, HANDLE_FLAG_INHERIT)? };
        }
        Ok(guard)
    }
}
impl Drop for InheritableHandles {
    fn drop(&mut self) {
        for handle in self.0 {
            // SAFETY: as in `new`; a handle that cannot be changed was closed, which is as good.
            let _ = unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT.0, HANDLE_FLAGS(0)) };
        }
    }
}
enum PipeEnd {
    /// The child reads (its stdin).
    Read,
    /// The child writes (its stdout and stderr).
    Write,
}
/// An anonymous pipe: (the end the child will use, the end this process keeps). Neither is
/// inheritable yet: `spawn_command_with` makes the child's end inheritable right before it creates
/// the process, so a process another thread starts in between cannot inherit a copy and keep the
/// pipe open. (The protocol also does not depend on the pipes closing.)
fn pipe_pair(child: PipeEnd) -> Result<(OwnedHandle, OwnedHandle)> {
    // SAFETY: both out-pointers are live locals, and each new handle is owned once.
    unsafe {
        let (mut read, mut write) = (HANDLE::default(), HANDLE::default());
        windows::Win32::System::Pipes::CreatePipe(&mut read, &mut write, None, 0)?;
        let (read, write) = (owned(read), owned(write));
        Ok(match child {
            PipeEnd::Read => (read, write),
            PipeEnd::Write => (write, read),
        })
    }
}
/// A process started by [`Process::spawn_piped`]. Dropping the `process` ends it and everything
/// it started.
pub struct PipedChild {
    pub process: Process,
    pub stdin: Option<std::fs::File>,
    pub stdout: Option<std::fs::File>,
    pub stderr: Option<std::fs::File>,
}
pub struct Process {
    handle: OwnedHandle,
    job: OwnedHandle,
    host_stop: Option<HostStop>,
    pub pid: u32,
}
/// Read-only process identity held open throughout audio capture. Keeping the
/// process object alive prevents PID reuse between verification and activation.
#[derive(Clone)]
pub struct AudioProcess {
    pub identity: butterpollo_core::audio_scope::Identity,
    handle: std::sync::Arc<OwnedHandle>,
}
impl AudioProcess {
    pub fn open(identity: butterpollo_core::audio_scope::Identity) -> Result<Self> {
        // SAFETY: a newly opened query/synchronize-only handle is owned once;
        // the out-pointers are live FILETIMEs and no process is modified.
        unsafe {
            let handle = owned(OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                false,
                identity.pid,
            )?);
            let mut created = FILETIME::default();
            let (mut exited, mut kernel, mut user) = (created, created, created);
            GetProcessTimes(
                raw(&handle),
                &mut created,
                &mut exited,
                &mut kernel,
                &mut user,
            )?;
            let started =
                u64::from(created.dwHighDateTime) << 32 | u64::from(created.dwLowDateTime);
            anyhow::ensure!(
                started == identity.started && started != 0,
                "game process identity changed"
            );
            anyhow::ensure!(
                process_session(identity.pid)? == current_session()?,
                "game process belongs to another Windows session"
            );
            let target = Self {
                identity,
                handle: std::sync::Arc::new(handle),
            };
            anyhow::ensure!(target.alive(), "game process exited before audio capture");
            Ok(target)
        }
    }
    pub fn alive(&self) -> bool {
        // SAFETY: the read-only process handle stays open; zero timeout never blocks.
        unsafe { WaitForSingleObject(raw(&self.handle), 0) == WAIT_TIMEOUT }
    }
}
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
fn ensure_streaming_session_with(
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
fn disconnected_desktops(
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
fn validate_streaming_session(state: WTS_CONNECTSTATE_CLASS, console: bool) -> Result<()> {
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

fn same_disconnected_owner(
    state: Option<WTS_CONNECTSTATE_CLASS>,
    source_sid: Option<&str>,
    target_sid: Option<&str>,
) -> bool {
    state == Some(WTSDisconnected)
        && source_sid
            .zip(target_sid)
            .is_some_and(|(source, target)| !source.is_empty() && source == target)
}

fn verify_disconnected_console_owner(
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

fn reattach_blocker(
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
pub fn user_environment() -> Result<BTreeMap<String, String>> {
    environment(target_token(&Target::User { elevated: false })?.as_ref())
}
/// The environment of a program started with `token`: that user's variables, as the desktop
/// gives them. Without a token the program runs as this host's own user, and gets this host's
/// environment as the host started with it, like a program a launcher starts (a variable the
/// user changes later reaches it after a host restart). CreateEnvironmentBlock without a token
/// would return the system's variables
/// instead (USERPROFILE `C:\Users\Default`, USERNAME SYSTEM, TEMP `C:\Windows\Temp`), and they
/// replace the inherited ones; the program then misses its user's caches and settings, such as
/// the AMD shader cache that made the PyroWave probe compile every pipeline again and time out.
fn environment(token: Option<&OwnedHandle>) -> Result<BTreeMap<String, String>> {
    let Some(token) = token else {
        return Ok(std::env::vars_os()
            .map(|(name, value)| {
                (
                    name.to_string_lossy().to_uppercase(),
                    value.to_string_lossy().into_owned(),
                )
            })
            .collect());
    };
    // SAFETY: the block from CreateEnvironmentBlock is NUL-terminated UTF-16 strings ending in an
    // empty one, so reads stop at the terminators, and `native` frees it on drop.
    unsafe {
        let mut native = Environment(std::ptr::null_mut());
        CreateEnvironmentBlock(&mut native.0, Some(raw(token)), false)?;
        let mut environment = BTreeMap::new();
        let mut offset = 0;
        while !native.0.is_null() && offset < 1024 * 1024 {
            let p = native.0.cast::<u16>().add(offset);
            if *p == 0 {
                break;
            }
            let mut len = 0;
            while offset + len < 1024 * 1024 && *p.add(len) != 0 {
                len += 1;
            }
            if offset + len >= 1024 * 1024 {
                bail!("process environment exceeds the limit");
            }
            let entry = String::from_utf16_lossy(std::slice::from_raw_parts(p, len));
            let split = if let Some(rest) = entry.strip_prefix('=') {
                rest.find('=').map(|i| i + 1)
            } else {
                entry.find('=')
            };
            if let Some(i) = split {
                environment.insert(entry[..i].to_uppercase(), entry[i + 1..].to_owned());
            }
            offset += len + 1;
        }
        Ok(environment)
    }
}
impl Process {
    pub(crate) fn handle(&self) -> HANDLE {
        raw(&self.handle)
    }
    /// Import an unnamed resource from this owned child. The child never gets
    /// a handle to the service process or its credentials.
    pub(crate) fn duplicate_resource(&self, handle: usize) -> Result<OwnedHandle> {
        anyhow::ensure!(
            handle != 0 && handle != usize::MAX,
            "invalid child resource handle"
        );
        // SAFETY: `self.handle` is the child's owned process handle and the duplicate is owned
        // once.
        unsafe {
            let mut duplicate = HANDLE::default();
            DuplicateHandle(
                raw(&self.handle),
                HANDLE(handle as *mut _),
                GetCurrentProcess(),
                &mut duplicate,
                0,
                false,
                DUPLICATE_SAME_ACCESS,
            )?;
            Ok(owned(duplicate))
        }
    }
    /// A handle of this process duplicated into the child, as the value the
    /// child uses; the child's handle closes when it exits.
    pub(crate) fn duplicate_into(&self, handle: HANDLE) -> Result<u64> {
        // SAFETY: `self.handle` is the child's owned process handle and `handle` is valid in this
        // process.
        unsafe {
            let mut duplicate = HANDLE::default();
            DuplicateHandle(
                GetCurrentProcess(),
                handle,
                raw(&self.handle),
                &mut duplicate,
                0,
                false,
                DUPLICATE_SAME_ACCESS,
            )?;
            Ok(duplicate.0 as u64)
        }
    }
    /// Keep the supervisor's unnamed stop event alive for the entire child.
    /// The child duplicates a wait-only handle; handle inheritance cannot cross
    /// Windows sessions, and session-local object names cannot reach the service.
    pub fn spawn_host(
        program: &Path,
        args: &[OsString],
        directory: Option<&Path>,
        target: Target,
    ) -> Result<Self> {
        let stop = HostStop::new()?;
        let mut args = args.to_vec();
        args.extend(["--service-stop-source".into(), stop.source().into()]);
        let mut process = Self::spawn(program, &args, directory, target, &BTreeMap::new(), true)?;
        process.host_stop = Some(stop);
        Ok(process)
    }
    pub fn spawn(
        program: &Path,
        args: &[OsString],
        directory: Option<&Path>,
        target: Target,
        extras: &BTreeMap<String, String>,
        hidden: bool,
    ) -> Result<Self> {
        let mut command = quote(program.as_os_str())?;
        for arg in args {
            command.push(' ');
            command.push_str(&quote(arg)?);
        }
        Self::spawn_command(program, &command, directory, target, extras, hidden, false)
    }
    pub fn spawn_detached(program: &Path, args: &[OsString], target: Target) -> Result<()> {
        let mut command = quote(program.as_os_str())?;
        for arg in args {
            command.push(' ');
            command.push_str(&quote(arg)?);
        }
        Self::spawn_command(
            program,
            &command,
            None,
            target,
            &BTreeMap::new(),
            true,
            true,
        )?;
        Ok(())
    }
    pub fn shell(
        command: &str,
        directory: Option<&Path>,
        elevated: bool,
        extras: &BTreeMap<String, String>,
    ) -> Result<Self> {
        Self::shell_inner(command, directory, elevated, extras, false)
    }
    pub fn shell_detached(
        command: &str,
        directory: Option<&Path>,
        elevated: bool,
        extras: &BTreeMap<String, String>,
    ) -> Result<()> {
        Self::shell_inner(command, directory, elevated, extras, true)?;
        Ok(())
    }
    fn shell_inner(
        command: &str,
        directory: Option<&Path>,
        elevated: bool,
        extras: &BTreeMap<String, String>,
        detached: bool,
    ) -> Result<Self> {
        if command.contains('\0') {
            bail!("shell command contains NUL");
        }
        let executable = std::path::PathBuf::from(
            std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into()),
        )
        .join("System32")
        .join("cmd.exe");
        let command = shell_command(command);
        let line = format!("{} /D /S /C \"{command}\"", quote(executable.as_os_str())?);
        Self::spawn_command(
            &executable,
            &line,
            directory,
            Target::User { elevated },
            extras,
            true,
            detached,
        )
    }
    /// Start `program` as `target` (inside the usual job, so it dies with the owner) with its
    /// standard input, output and error on pipes. A SYSTEM host uses this to run a program as the
    /// signed-in user: the child holds the user's token from the start, nothing in it runs as SYSTEM.
    /// It cannot start a program through the desktop shell's token (an elevated administrator host
    /// that is not SYSTEM), because that path cannot hand over pipes.
    pub fn spawn_piped(program: &Path, args: &[OsString], target: Target) -> Result<PipedChild> {
        let mut command = quote(program.as_os_str())?;
        for arg in args {
            command.push(' ');
            command.push_str(&quote(arg)?);
        }
        let (stdin_read, stdin_write) = pipe_pair(PipeEnd::Read)?;
        let (stdout_write, stdout_read) = pipe_pair(PipeEnd::Write)?;
        let (stderr_write, stderr_read) = pipe_pair(PipeEnd::Write)?;
        let process = Self::spawn_command_with(
            program,
            &command,
            // The working directory a SYSTEM parent has (its profile) is not the user's to open.
            program.parent(),
            target,
            &BTreeMap::new(),
            true,
            false,
            Some([raw(&stdin_read), raw(&stdout_write), raw(&stderr_write)]),
        )?;
        // The child holds its ends now; the parent's copies would keep the pipes open forever.
        drop((stdin_read, stdout_write, stderr_write));
        Ok(PipedChild {
            process,
            stdin: Some(std::fs::File::from(stdin_write)),
            stdout: Some(std::fs::File::from(stdout_read)),
            stderr: Some(std::fs::File::from(stderr_read)),
        })
    }
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn spawn_command(
        program: &Path,
        line: &str,
        directory: Option<&Path>,
        target: Target,
        extras: &BTreeMap<String, String>,
        hidden: bool,
        detached: bool,
    ) -> Result<Self> {
        Self::spawn_command_with(
            program, line, directory, target, extras, hidden, detached, None,
        )
    }
    /// `stdio` is the child's (stdin, stdout, stderr), inheritable handles that only the child
    /// inherits.
    #[allow(clippy::too_many_arguments)]
    fn spawn_command_with(
        program: &Path,
        line: &str,
        directory: Option<&Path>,
        target: Target,
        extras: &BTreeMap<String, String>,
        hidden: bool,
        detached: bool,
        stdio: Option<[HANDLE; 3]>,
    ) -> Result<Self> {
        let token = target_token(&target)?;
        // Only SYSTEM assigns primary tokens; an elevated administrator gets
        // the desktop shell's token for non-elevated programs.
        let shell_token = token.is_some() && !is_system();
        if shell_token && stdio.is_some() {
            bail!("cannot give pipes to a program started through the desktop shell's token");
        }
        let program = wide(program.as_os_str())?;
        let directory = directory.map(|p| wide(p.as_os_str())).transpose()?;
        let mut line = wide(std::ffi::OsStr::new(line))?;
        let mut environment = environment(token.as_ref())?;
        // SAFETY: the strings given to CreateProcess are NUL-terminated locals, and each new
        // handle is owned once.
        unsafe {
            for (key, value) in extras {
                if key.is_empty() || key.contains(['=', '\0']) || value.contains('\0') {
                    bail!("invalid process environment variable");
                }
                environment.insert(key.to_uppercase(), value.clone());
            }
            let mut env: Vec<u16> = environment
                .iter()
                .flat_map(|(k, v)| format!("{k}={v}\0").encode_utf16().collect::<Vec<_>>())
                .collect();
            env.extend([0, 0]);
            let job = owned(CreateJobObjectW(None, PCWSTR::null())?);
            let limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
                BasicLimitInformation: JOBOBJECT_BASIC_LIMIT_INFORMATION {
                    LimitFlags: JOB_OBJECT_LIMIT_BREAKAWAY_OK
                        | if detached {
                            JOB_OBJECT_LIMIT(0)
                        } else {
                            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
                        },
                    ..Default::default()
                },
                ..Default::default()
            };
            SetInformationJobObject(
                raw(&job),
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )?;
            let mut desktop: Vec<u16> = "winsta0\\default\0".encode_utf16().collect();
            let mut startup = STARTUPINFOEXW {
                StartupInfo: STARTUPINFOW {
                    cb: if stdio.is_some() {
                        size_of::<STARTUPINFOEXW>()
                    } else {
                        size_of::<STARTUPINFOW>()
                    } as u32,
                    lpDesktop: PWSTR(desktop.as_mut_ptr()),
                    dwFlags: if hidden {
                        STARTF_USESHOWWINDOW
                    } else {
                        STARTUPINFOW_FLAGS(0)
                    },
                    wShowWindow: if hidden { 0 } else { 1 },
                    ..Default::default()
                },
                lpAttributeList: LPPROC_THREAD_ATTRIBUTE_LIST(std::ptr::null_mut()),
            };
            // Only the three standard handles are inherited, not every inheritable handle of this
            // process.
            let inherited = stdio.unwrap_or_default();
            let mut attributes = AttributeList(Vec::new());
            if let Some(stdio) = stdio {
                startup.StartupInfo.dwFlags |= STARTF_USESTDHANDLES;
                startup.StartupInfo.hStdInput = stdio[0];
                startup.StartupInfo.hStdOutput = stdio[1];
                startup.StartupInfo.hStdError = stdio[2];
                startup.lpAttributeList = attributes.handle_list(&inherited)?;
            }
            let startup = startup;
            let mut info = PROCESS_INFORMATION::default();
            let flags = CREATE_SUSPENDED
                | CREATE_UNICODE_ENVIRONMENT
                | if stdio.is_some() {
                    EXTENDED_STARTUPINFO_PRESENT
                } else {
                    PROCESS_CREATION_FLAGS(0)
                }
                | if detached {
                    CREATE_BREAKAWAY_FROM_JOB
                } else {
                    PROCESS_CREATION_FLAGS(0)
                }
                | if hidden {
                    CREATE_NO_WINDOW
                } else {
                    PROCESS_CREATION_FLAGS(0)
                };
            let current = directory
                .as_ref()
                .map_or(PCWSTR::null(), |d| PCWSTR(d.as_ptr()));
            let mut create = |flags: PROCESS_CREATION_FLAGS| -> windows::core::Result<()> {
                if let Some(token) = &token
                    && shell_token
                {
                    // Administrators lack SeAssignPrimaryTokenPrivilege, so
                    // the secondary logon service starts the program. It is
                    // never created inside this job; breakaway cannot apply.
                    CreateProcessWithTokenW(
                        raw(token),
                        CREATE_PROCESS_LOGON_FLAGS(0),
                        PCWSTR(program.as_ptr()),
                        Some(PWSTR(line.as_mut_ptr())),
                        PROCESS_CREATION_FLAGS(flags.0 & !CREATE_BREAKAWAY_FROM_JOB.0),
                        Some(env.as_ptr().cast()),
                        current,
                        &startup.StartupInfo,
                        &mut info,
                    )?;
                } else if let Some(token) = &token {
                    CreateProcessAsUserW(
                        Some(raw(token)),
                        PCWSTR(program.as_ptr()),
                        Some(PWSTR(line.as_mut_ptr())),
                        None,
                        None,
                        stdio.is_some(),
                        flags,
                        Some(env.as_ptr().cast()),
                        current,
                        &startup.StartupInfo,
                        &mut info,
                    )?;
                } else {
                    CreateProcessW(
                        PCWSTR(program.as_ptr()),
                        Some(PWSTR(line.as_mut_ptr())),
                        None,
                        None,
                        stdio.is_some(),
                        flags,
                        Some(env.as_ptr().cast()),
                        current,
                        &startup.StartupInfo,
                        &mut info,
                    )?;
                }
                Ok(())
            };
            // The child's pipe ends are inheritable only while the process is being created.
            let inheritable = stdio.map(InheritableHandles::new).transpose()?;
            match create(flags) {
                Err(e)
                    if detached
                        && e.code()
                            == windows::core::HRESULT::from_win32(ERROR_ACCESS_DENIED.0) =>
                {
                    create(PROCESS_CREATION_FLAGS(
                        flags.0 & !CREATE_BREAKAWAY_FROM_JOB.0,
                    ))?;
                }
                result => result?,
            }
            drop(inheritable);
            let process = Self {
                handle: owned(info.hProcess),
                job,
                host_stop: None,
                pid: info.dwProcessId,
            };
            let thread = owned(info.hThread);
            if let Err(e) = AssignProcessToJobObject(raw(&process.job), raw(&process.handle)) {
                let _ = TerminateProcess(raw(&process.handle), 1);
                return Err(e.into());
            }
            if ResumeThread(raw(&thread)) == u32::MAX {
                let _ = TerminateProcess(raw(&process.handle), 1);
                bail!("cannot resume the application process");
            }
            Ok(process)
        }
    }
    pub fn exit_code(&self) -> Result<Option<u32>> {
        // SAFETY: `self.handle` is an owned process handle that stays open while `self` lives.
        unsafe {
            if WaitForSingleObject(raw(&self.handle), 0) == WAIT_TIMEOUT {
                return Ok(None);
            }
            let mut code = 0;
            GetExitCodeProcess(raw(&self.handle), &mut code)?;
            Ok(Some(code))
        }
    }
    pub fn active_processes(&self) -> Result<u32> {
        // SAFETY: `self.job` is an owned job handle and `info` is writable for the size passed.
        unsafe {
            let mut info = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
            QueryInformationJobObject(
                Some(raw(&self.job)),
                JobObjectBasicAccountingInformation,
                (&mut info as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                None,
            )?;
            Ok(info.ActiveProcesses)
        }
    }
    pub fn process_ids(&self) -> Result<Vec<u32>> {
        let mut words = vec![0usize; 4098];
        let list = words.as_mut_ptr().cast::<JOBOBJECT_BASIC_PROCESS_ID_LIST>();
        // SAFETY: `words` is usize-aligned, outlives the call and its byte size is passed, and
        // `count` is checked against the 4096 ids it can hold before the slice is made.
        unsafe {
            QueryInformationJobObject(
                Some(raw(&self.job)),
                JobObjectBasicProcessIdList,
                list.cast(),
                (words.len() * size_of::<usize>()) as u32,
                None,
            )?;
            let count = (*list).NumberOfProcessIdsInList as usize;
            if count > 4096 {
                bail!("application process group exceeds its limit");
            }
            Ok(
                std::slice::from_raw_parts((*list).ProcessIdList.as_ptr(), count)
                    .iter()
                    .filter_map(|id| u32::try_from(*id).ok())
                    .collect(),
            )
        }
    }
    pub fn wait(&self, timeout: Duration) -> Result<u32> {
        // SAFETY: `self.handle` is an owned process handle that stays open while `self` lives.
        unsafe {
            match WaitForSingleObject(
                raw(&self.handle),
                timeout.as_millis().min(u32::MAX as u128) as u32,
            ) {
                WAIT_OBJECT_0 => {}
                WAIT_TIMEOUT => bail!("process timed out"),
                _ => return Err(windows::core::Error::from_thread().into()),
            }
        }
        self.exit_code()?.context("process still running")
    }
    /// Wait without polling; cancellation terminates this command's entire
    /// job before the owner can run its undo commands.
    pub fn wait_cancellable(
        &self,
        timeout: Duration,
        cancel: &CommandCancellation,
    ) -> Result<Option<u32>> {
        // SAFETY: Both owned handles remain live for the synchronous wait.
        let result = unsafe {
            WaitForMultipleObjects(
                &[raw(&cancel.0.0), raw(&self.handle)],
                false,
                timeout.as_millis().min(u32::MAX as u128) as u32,
            )
        };
        if result == WAIT_OBJECT_0 {
            self.stop()?;
            Ok(None)
        } else if result == WAIT_EVENT(WAIT_OBJECT_0.0 + 1) {
            self.exit_code()?.context("process still running").map(Some)
        } else if result == WAIT_TIMEOUT {
            self.stop()?;
            bail!("process timed out")
        } else {
            Err(windows::core::Error::from_thread().into())
        }
    }
    /// Waits, up to `timeout`, until a GUI process started here is pumping
    /// messages. False when it timed out or the process has no message loop.
    pub fn wait_input_idle(&self, timeout: Duration) -> bool {
        // SAFETY: `self.handle` is an owned process handle that stays open while `self` lives.
        unsafe {
            WaitForInputIdle(
                raw(&self.handle),
                timeout.as_millis().min(u32::MAX as u128) as u32,
            ) == 0
        }
    }
    /// Ask Windows to end the whole job, without waiting for it to be gone.
    pub fn terminate(&self) -> Result<()> {
        // SAFETY: `self.job` is an owned job handle that stays open while `self` lives.
        unsafe { TerminateJobObject(raw(&self.job), 0)? };
        Ok(())
    }
    pub fn stop(&self) -> Result<()> {
        // SAFETY: `self.job` is an owned job handle that stays open while `self` lives.
        unsafe {
            TerminateJobObject(raw(&self.job), 0)?;
        }
        self.wait(Duration::from_secs(10))?;
        Ok(())
    }
    pub fn stop_graceful(&self, timeout: Duration) -> Result<()> {
        use windows::Win32::UI::WindowsAndMessaging::*;
        struct Closing {
            ids: std::collections::BTreeSet<u32>,
            sent: bool,
        }
        unsafe extern "system" fn close_window(window: HWND, data: LPARAM) -> BOOL {
            // SAFETY: EnumWindows passes the `&mut Closing` from stop_graceful as `data`, which
            // outlives the enumeration and is not otherwise used during it.
            unsafe {
                let closing = &mut *(data.0 as *mut Closing);
                let mut pid = 0;
                GetWindowThreadProcessId(window, Some(&mut pid));
                if closing.ids.contains(&pid)
                    && PostMessageW(Some(window), WM_CLOSE, WPARAM(0), LPARAM(0)).is_ok()
                {
                    closing.sent = true;
                }
                BOOL(1)
            }
        }
        if timeout.is_zero() || self.active_processes()? == 0 {
            return self.stop();
        }
        let mut words = vec![0usize; 4098];
        let list = words.as_mut_ptr().cast::<JOBOBJECT_BASIC_PROCESS_ID_LIST>();
        // SAFETY: `words` is usize-aligned with its byte size passed, `count` is checked before the
        // slice, and `closing` outlives the synchronous EnumWindows call.
        unsafe {
            QueryInformationJobObject(
                Some(raw(&self.job)),
                JobObjectBasicProcessIdList,
                list.cast(),
                (words.len() * size_of::<usize>()) as u32,
                None,
            )?;
            let count = (*list).NumberOfProcessIdsInList as usize;
            if count > 4096 {
                bail!("application process group exceeds its limit");
            }
            let ids = std::slice::from_raw_parts((*list).ProcessIdList.as_ptr(), count)
                .iter()
                .filter_map(|id| u32::try_from(*id).ok())
                .collect();
            let mut closing = Closing { ids, sent: false };
            EnumWindows(
                Some(close_window),
                LPARAM((&mut closing as *mut Closing) as isize),
            )?;
            if closing.sent {
                let deadline = std::time::Instant::now() + timeout;
                while self.active_processes()? != 0 && std::time::Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        }
        self.stop()
    }
    pub fn shutdown_host(&self) -> Result<HostShutdown> {
        if let Some(code) = self.exit_code()? {
            return Ok(HostShutdown::Graceful(code));
        }
        self.host_stop
            .as_ref()
            .context("host has no supervisor stop channel")?
            .request()?;
        match self.wait(Duration::from_secs(20)) {
            Ok(code) => Ok(HostShutdown::Graceful(code)),
            Err(error) => {
                tracing::warn!(pid=self.pid, %error, "host shutdown timed out; terminating its owned job");
                self.stop()?;
                Ok(HostShutdown::Forced)
            }
        }
    }
}
#[derive(Debug, PartialEq, Eq)]
pub enum HostShutdown {
    Graceful(u32),
    Forced,
}
/// A shared, manual-reset event for cancelling owned application commands.
pub struct CommandCancellation(HostStop);
impl CommandCancellation {
    pub fn new() -> Result<Self> {
        Ok(Self(HostStop::new()?))
    }
    pub fn cancel(&self) -> Result<()> {
        self.0.request()
    }
    pub fn requested(&self) -> bool {
        // SAFETY: The event is owned for this nonblocking state check.
        unsafe { WaitForSingleObject(raw(&self.0.0), 0) == WAIT_OBJECT_0 }
    }
}

struct HostStop(OwnedHandle);
impl HostStop {
    fn new() -> Result<Self> {
        // SAFETY: CreateEventW returns a new event handle that this struct then owns.
        Ok(Self(owned(unsafe {
            CreateEventW(None, true, false, PCWSTR::null())?
        })))
    }
    fn source(&self) -> String {
        format!(
            "{}:{:x}",
            // SAFETY: GetCurrentProcessId has no preconditions.
            unsafe { GetCurrentProcessId() },
            self.0.as_raw_handle() as usize
        )
    }
    fn request(&self) -> Result<()> {
        // SAFETY: `self.0` is the owned event handle, open while `self` lives.
        unsafe {
            SetEvent(raw(&self.0))?;
        }
        Ok(())
    }
}
fn parse_stop_source(source: &str) -> Result<(u32, usize)> {
    let (pid, handle) = source
        .split_once(':')
        .context("invalid supervisor stop source")?;
    if pid.is_empty()
        || !pid.bytes().all(|b| b.is_ascii_digit())
        || handle.is_empty()
        || !handle.bytes().all(|b| b.is_ascii_hexdigit())
    {
        bail!("invalid supervisor stop source");
    }
    let pid: u32 = pid.parse()?;
    let handle = usize::from_str_radix(handle, 16)?;
    if pid == 0 || handle == 0 || handle == usize::MAX {
        bail!("invalid supervisor stop source");
    }
    Ok((pid, handle))
}
fn parent_pid() -> Result<u32> {
    use windows::Win32::System::Diagnostics::ToolHelp::*;
    // SAFETY: `entry` has dwSize set to its size and the owned snapshot handle is open for every
    // call.
    unsafe {
        let snapshot = owned(CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)?);
        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        Process32FirstW(raw(&snapshot), &mut entry)?;
        loop {
            if entry.th32ProcessID == GetCurrentProcessId() {
                return Ok(entry.th32ParentProcessID);
            }
            Process32NextW(raw(&snapshot), &mut entry)
                .context("host parent process unavailable")?;
        }
    }
}
fn duplicate_stop_event(pid: u32, handle: usize) -> Result<OwnedHandle> {
    // SAFETY: the parent process handle is owned for the call, DuplicateHandle validates `handle`
    // in that process, and the copy is owned once.
    unsafe {
        let parent = owned(OpenProcess(PROCESS_DUP_HANDLE, false, pid)?);
        let mut duplicate = HANDLE::default();
        DuplicateHandle(
            raw(&parent),
            HANDLE(handle as *mut _),
            GetCurrentProcess(),
            &mut duplicate,
            SYNCHRONIZATION_SYNCHRONIZE.0,
            false,
            DUPLICATE_HANDLE_OPTIONS(0),
        )?;
        Ok(owned(duplicate))
    }
}
pub struct StopSignal(Option<OwnedHandle>);
impl StopSignal {
    pub fn new(source: Option<&str>) -> Result<Self> {
        let Some(source) = source else {
            return Ok(Self(None));
        };
        let (pid, handle) = parse_stop_source(source)?;
        if pid != parent_pid()? {
            bail!("stop source is not this host's supervisor");
        }
        Ok(Self(Some(
            duplicate_stop_event(pid, handle).context("connecting supervisor stop event")?,
        )))
    }
    pub fn requested(&self) -> bool {
        self.0
            .as_ref()
            // SAFETY: `event` is an owned handle borrowed from `self`, open for the wait.
            .is_some_and(|event| unsafe { WaitForSingleObject(raw(event), 0) == WAIT_OBJECT_0 })
    }
}

/// The program a command line starts: its first argument, without quotes.
pub fn command_target(command: &str) -> &str {
    let command = command.trim_start();
    match command.strip_prefix('"') {
        Some(rest) => rest.split('"').next().unwrap_or(rest),
        None => command.split(char::is_whitespace).next().unwrap_or(""),
    }
}
/// Whether Windows must open the target through its association: a URL
/// (`steam://`, `ms-settings:`) or a file that is not a program. cmd.exe
/// cannot start a URL, so Steam Big Picture and store links failed.
fn opens_through_shell(target: &str) -> bool {
    if let Some((scheme, _)) = target.split_once(':')
        && scheme.len() > 1
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    {
        return true;
    }
    let name = target.rsplit(['\\', '/']).next().unwrap_or(target);
    match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => !matches!(
            extension.to_ascii_lowercase().as_str(),
            "exe" | "com" | "bat" | "cmd"
        ),
        _ => false,
    }
}
/// The command for cmd.exe. URLs and documents are opened as Windows opens
/// them (as Vibepollo resolves associations), waiting for the program they
/// start, which stays in the app's job.
pub fn shell_command(command: &str) -> String {
    let target = command_target(command);
    if target.is_empty() || !opens_through_shell(target) {
        return command.to_owned();
    }
    let trimmed = command.trim_start();
    let rest = if trimmed.starts_with('"') {
        &trimmed[(target.len() + 2).min(trimmed.len())..]
    } else {
        &trimmed[target.len()..]
    };
    format!("start \"\" /wait \"{target}\"{rest}")
}
/// The current time on the clock of [`creation_time`] (100 ns since 1601).
pub fn now() -> u64 {
    // SAFETY: GetSystemTimeAsFileTime has no preconditions.
    let time = unsafe { windows::Win32::System::SystemInformation::GetSystemTimeAsFileTime() };
    u64::from(time.dwHighDateTime) << 32 | u64::from(time.dwLowDateTime)
}
/// A process's creation time (100 ns since 1601), 0 when it cannot be read.
pub fn creation_time(pid: u32) -> u64 {
    // SAFETY: the process handle is owned and open for the call, and each out-pointer is a local
    // FILETIME.
    unsafe {
        let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return 0;
        };
        let process = owned(process);
        let mut created = FILETIME::default();
        let (mut exited, mut kernel, mut user) = (created, created, created);
        if GetProcessTimes(
            raw(&process),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        )
        .is_err()
        {
            return 0;
        }
        u64::from(created.dwHighDateTime) << 32 | u64::from(created.dwLowDateTime)
    }
}
/// A process's sampled CPU and resident memory usage.
pub struct Usage {
    pub started: u64,
    /// Kernel and user CPU time in 100 ns units.
    pub cpu_time: u64,
    pub working_set: u64,
}
/// CPU time and resident memory, with the creation time to reject reused IDs.
pub fn usage(pid: u32) -> Option<Usage> {
    use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    // SAFETY: the process handle is owned, and the time and memory buffers stay valid for each
    // call. The memory buffer's size matches the type passed to GetProcessMemoryInfo.
    unsafe {
        let process = owned(
            OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, false, pid)
                .or_else(|_| OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid))
                .ok()?,
        );
        let mut created = FILETIME::default();
        let (mut exited, mut kernel, mut user) = (created, created, created);
        GetProcessTimes(
            raw(&process),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        )
        .ok()?;
        let mut memory = PROCESS_MEMORY_COUNTERS::default();
        let got_memory = GetProcessMemoryInfo(
            raw(&process),
            &mut memory,
            size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        )
        .is_ok();
        let ticks =
            |time: FILETIME| u64::from(time.dwHighDateTime) << 32 | u64::from(time.dwLowDateTime);
        Some(Usage {
            started: ticks(created),
            cpu_time: ticks(kernel) + ticks(user),
            working_set: if got_memory {
                memory.WorkingSetSize as u64
            } else {
                0
            },
        })
    }
}
/// Every running process with its parent, creation time and program name.
pub fn processes() -> Result<Vec<butterpollo_core::steam::Process>> {
    use windows::Win32::System::Diagnostics::ToolHelp::*;
    let mut list = vec![];
    // SAFETY: `entry` has dwSize set to its size and the owned snapshot handle stays open for the
    // loop.
    unsafe {
        let snapshot = owned(CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)?);
        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut more = Process32FirstW(raw(&snapshot), &mut entry).is_ok();
        while more {
            let length = entry
                .szExeFile
                .iter()
                .position(|c| *c == 0)
                .unwrap_or(entry.szExeFile.len());
            if entry.th32ProcessID != 0 {
                list.push(butterpollo_core::steam::Process {
                    pid: entry.th32ProcessID,
                    parent: entry.th32ParentProcessID,
                    started: creation_time(entry.th32ProcessID),
                    name: String::from_utf16_lossy(&entry.szExeFile[..length]),
                });
            }
            more = Process32NextW(raw(&snapshot), &mut entry).is_ok();
        }
    }
    Ok(list)
}
/// A process's full program path.
pub fn image_path(pid: u32) -> Option<String> {
    // SAFETY: `buffer` outlives the call and `size` is its capacity in UTF-16 units, updated to the
    // length written.
    unsafe {
        let process = owned(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?);
        let mut buffer = [0u16; 32768];
        let mut size = buffer.len() as u32;
        QueryFullProcessImageNameW(
            raw(&process),
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut size,
        )
        .ok()?;
        Some(String::from_utf16_lossy(&buffer[..size as usize]))
    }
}
/// Ask the windows of `processes` (pid → creation time) to close, wait up to
/// `timeout` for them to exit, then end the rest. An id that now belongs to
/// another process is left alone.
pub fn stop_processes(processes: &BTreeMap<u32, u64>, timeout: Duration) {
    use windows::Win32::UI::WindowsAndMessaging::*;
    let alive = |processes: &BTreeMap<u32, u64>| -> Vec<u32> {
        processes
            .iter()
            .filter(|(pid, started)| creation_time(**pid) == **started && **started != 0)
            .map(|(pid, _)| *pid)
            .collect()
    };
    unsafe extern "system" fn close_window(window: HWND, data: LPARAM) -> BOOL {
        // SAFETY: EnumWindows passes the `&Vec<u32>` from stop_processes as `data`, which outlives
        // the enumeration.
        unsafe {
            let ids = &*(data.0 as *const Vec<u32>);
            let mut pid = 0;
            GetWindowThreadProcessId(window, Some(&mut pid));
            if ids.contains(&pid) {
                let _ = PostMessageW(Some(window), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
            BOOL(1)
        }
    }
    let ids = alive(processes);
    if ids.is_empty() {
        return;
    }
    // SAFETY: `ids` outlives the synchronous EnumWindows call that reads it through `close_window`.
    unsafe {
        let _ = EnumWindows(
            Some(close_window),
            LPARAM((&ids as *const Vec<u32>) as isize),
        );
    }
    let deadline = std::time::Instant::now() + timeout;
    while !alive(processes).is_empty() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    for pid in alive(processes) {
        // SAFETY: the handle OpenProcess returns is owned and closed once on drop.
        unsafe {
            if let Ok(process) = OpenProcess(PROCESS_TERMINATE, false, pid) {
                let process = owned(process);
                let _ = TerminateProcess(raw(&process), 1);
            }
        }
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn a_pipe_end_can_be_used_by_its_creator() {
        use super::*;
        use std::io::{Read, Write};
        let (child_end, parent_end) = pipe_pair(PipeEnd::Write).unwrap();
        let mut writer = std::fs::File::from(child_end);
        let mut reader = std::fs::File::from(parent_end);
        writer.write_all(b"ping").unwrap();
        drop(writer);
        let mut text = String::new();
        reader.read_to_string(&mut text).unwrap();
        assert_eq!(text, "ping");
    }
    #[test]
    fn a_piped_child_reads_stdin_and_answers_on_stdout_and_stderr_inside_the_job() {
        use super::*;
        use std::io::{Read, Write};
        let cmd =
            std::path::PathBuf::from(std::env::var_os("WINDIR").unwrap()).join("System32\\cmd.exe");
        if !is_system()
            && target_token(&Target::User { elevated: false })
                .unwrap()
                .is_some()
        {
            // An elevated administrator host starts through the shell token, which cannot hand
            // over pipes; it must say so rather than start the program without them.
            let err = Process::spawn_piped(&cmd, &[], Target::User { elevated: false });
            assert!(format!("{:#}", err.err().unwrap()).contains("pipes"));
            return;
        }
        let system32 = cmd.parent().unwrap().to_path_buf();
        let target = || Target::User { elevated: false };
        // What goes to the child's stdin comes back on its stdout.
        let args = [OsString::from("^")];
        let mut echo =
            Process::spawn_piped(&system32.join("findstr.exe"), &args, target()).unwrap();
        echo.stdin.take().unwrap().write_all(b"hello\r\n").unwrap();
        let mut out = String::new();
        echo.stdout
            .take()
            .unwrap()
            .read_to_string(&mut out)
            .unwrap();
        assert_eq!(out.trim(), "hello");
        assert_eq!(echo.process.wait(Duration::from_secs(10)).unwrap(), 0);
        // What the child writes to stderr arrives on stderr, and its exit code is its own.
        let args = [OsString::from("nothing-by-this-name")];
        let mut failing =
            Process::spawn_piped(&system32.join("where.exe"), &args, target()).unwrap();
        let (mut out, mut err) = (String::new(), String::new());
        failing
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut out)
            .unwrap();
        failing
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut err)
            .unwrap();
        assert!(
            out.is_empty() && !err.is_empty(),
            "stdout {out:?} stderr {err:?}"
        );
        assert_eq!(failing.process.wait(Duration::from_secs(10)).unwrap(), 1);
        // The child runs as the signed-in user, never as the SYSTEM parent.
        if is_system() {
            let mut who =
                Process::spawn_piped(&system32.join("whoami.exe"), &[], target()).unwrap();
            let mut name = String::new();
            who.stdout
                .take()
                .unwrap()
                .read_to_string(&mut name)
                .unwrap();
            assert!(
                !name.trim().is_empty() && !name.to_lowercase().contains("system"),
                "{name:?}"
            );
            println!("the child ran as {}", name.trim());
        }
        // A child that does not end is ended by terminate, and its pipes then close.
        let args = [
            OsString::from("-n"),
            OsString::from("30"),
            OsString::from("127.0.0.1"),
        ];
        let mut long = Process::spawn_piped(&system32.join("ping.exe"), &args, target()).unwrap();
        assert_eq!(long.process.exit_code().unwrap(), None);
        long.process.terminate().unwrap();
        let mut rest = String::new();
        long.stdout
            .take()
            .unwrap()
            .read_to_string(&mut rest)
            .unwrap();
        assert!(long.process.wait(Duration::from_secs(10)).is_ok());
    }
    #[test]
    fn console_reattachment_refuses_other_users_and_transitional_sessions() {
        use super::*;
        assert!(reattach_blocker(4, 1, WTSDisconnected, 2, true, false, false).is_none());
        assert!(reattach_blocker(4, 1, WTSActive, 2, true, false, false).is_none());
        for (source, console, state, protocol, user, occupied) in [
            (4, 1, WTSDisconnected, 2, true, true),
            (4, 1, WTSDisconnected, 2, false, false),
            (4, 1, WTSConnected, 2, true, false),
            (4, 1, WTSDisconnected, 0, true, false),
            (4, u32::MAX, WTSDisconnected, 2, true, false),
            (4, 4, WTSDisconnected, 2, true, false),
            (4, 4, WTSActive, 0, true, false),
            (0, 1, WTSDisconnected, 2, true, false),
        ] {
            assert!(
                reattach_blocker(source, console, state, protocol, user, occupied, false).is_some()
            );
        }
    }
    #[test]
    fn console_sign_in_host_explains_why_return_to_console_is_unavailable() {
        use super::*;
        // The installed service follows the console: after Remote Desktop it
        // runs at the sign-in screen, and the reason must say what to do.
        let reason = reattach_blocker(4, 4, WTSConnected, 0, false, false, false).unwrap();
        for text in [
            "sign-in screen",
            "started inside a Remote Desktop session",
            "reconnect Moonlight",
        ] {
            assert!(reason.contains(text), "missing {text} in {reason}");
        }
        // A signed-in console host keeps the existing explanation.
        assert_eq!(
            reattach_blocker(1, 1, WTSActive, 0, true, true, false),
            Some("This host is already attached to the physical console.")
        );
    }
    #[test]
    fn occupied_console_requires_a_disconnected_verified_matching_owner() {
        use super::*;
        const OWNER: &str = "S-1-5-21-100-200-300-1001";
        const OTHER: &str = "S-1-5-21-100-200-300-1002";
        for (state, source, target, allowed) in [
            (Some(WTSDisconnected), Some(OWNER), Some(OWNER), true),
            (Some(WTSActive), Some(OWNER), Some(OWNER), false),
            (Some(WTSConnected), Some(OWNER), Some(OWNER), false),
            (None, Some(OWNER), Some(OWNER), false),
            (Some(WTSDisconnected), Some(OWNER), Some(OTHER), false),
            (Some(WTSDisconnected), None, Some(OWNER), false),
            (Some(WTSDisconnected), Some(OWNER), None, false),
            (Some(WTSDisconnected), None, None, false),
            (Some(WTSDisconnected), Some(""), Some(""), false),
        ] {
            let verified = same_disconnected_owner(state, source, target);
            for host_state in [WTSActive, WTSDisconnected] {
                assert_eq!(
                    reattach_blocker(4, 1, host_state, 2, true, true, verified).is_none(),
                    allowed,
                    "target state {state:?}, source {source:?}, target {target:?}"
                );
            }
        }
        // An owner match cannot authorize a different source or a changing console.
        for (source, console, state, protocol, signed_in) in [
            (0, 1, WTSDisconnected, 2, true),
            (4, u32::MAX, WTSDisconnected, 2, true),
            (4, 4, WTSDisconnected, 2, true),
            (4, 1, WTSConnected, 2, true),
            (4, 1, WTSDisconnected, 0, true),
            (4, 1, WTSDisconnected, 2, false),
        ] {
            assert!(
                reattach_blocker(source, console, state, protocol, signed_in, true, true).is_some()
            );
        }
    }
    #[test]
    fn console_owner_verification_rechecks_the_target_after_reading_both_tokens() {
        use super::*;
        use std::cell::Cell;
        for (final_state, final_console, allowed) in [
            (WTSDisconnected, 1, true),
            (WTSActive, 1, false),
            (WTSDisconnected, 2, false),
        ] {
            let state_reads = Cell::new(0);
            let sid_reads = Cell::new(0);
            let verified = verify_disconnected_console_owner(
                4,
                1,
                |session| {
                    assert_eq!(session, 1);
                    let read = state_reads.get();
                    state_reads.set(read + 1);
                    assert_eq!(sid_reads.get(), if read == 0 { 0 } else { 2 });
                    Ok(if read == 0 {
                        WTSDisconnected
                    } else {
                        final_state
                    })
                },
                |session| {
                    assert_eq!(state_reads.get(), 1);
                    assert_eq!(session, if sid_reads.get() == 0 { 4 } else { 1 });
                    sid_reads.set(sid_reads.get() + 1);
                    Ok("S-1-5-21-100-200-300-1001".into())
                },
                || {
                    assert_eq!(state_reads.get(), 2);
                    assert_eq!(sid_reads.get(), 2);
                    final_console
                },
            )
            .unwrap();
            assert_eq!(verified, allowed);
            assert_eq!(state_reads.get(), 2);
            assert_eq!(sid_reads.get(), 2);
        }
    }
    #[test]
    fn console_owner_verification_fails_closed_when_either_token_query_fails() {
        use super::*;
        use std::cell::Cell;
        for failed_session in [4, 1] {
            let state_reads = Cell::new(0);
            let sid_reads = Cell::new(0);
            let error = verify_disconnected_console_owner(
                4,
                1,
                |session| {
                    assert_eq!(session, 1);
                    state_reads.set(state_reads.get() + 1);
                    Ok(WTSDisconnected)
                },
                |session| {
                    sid_reads.set(sid_reads.get() + 1);
                    anyhow::ensure!(session != failed_session, "session token unavailable");
                    Ok("S-1-5-21-100-200-300-1001".into())
                },
                || panic!("do not accept a console after an identity query failed"),
            )
            .unwrap_err();
            assert!(error.to_string().contains("session token unavailable"));
            assert_eq!(state_reads.get(), 1);
            assert_eq!(sid_reads.get(), if failed_session == 4 { 1 } else { 2 });
        }
    }
    #[test]
    fn command_cancellation_ends_the_owned_process_before_returning() -> anyhow::Result<()> {
        use super::*;
        let cancel = CommandCancellation::new()?;
        let elevated = user_environment().is_err();
        let child = Process::shell(
            "ping -n 120 127.0.0.1 >nul",
            None,
            elevated,
            &BTreeMap::new(),
        )?;
        cancel.cancel()?;
        let started = std::time::Instant::now();
        assert_eq!(
            child.wait_cancellable(Duration::from_secs(120), &cancel)?,
            None
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(child.exit_code()?.is_some());
        assert_eq!(child.active_processes()?, 0);
        Ok(())
    }
    #[test]
    fn streaming_session_rejects_disconnected_rdp_and_session_transitions() {
        for state in [
            WTSConnected,
            WTSDisconnected,
            WTSConnectQuery,
            WTSReset,
            WTSDown,
            WTSInit,
        ] {
            let error = validate_streaming_session(state, false).unwrap_err();
            assert!(error.to_string().contains("disconnected or changing"));
            assert!(error.to_string().contains("RDP"));
        }
        for state in [WTSConnectQuery, WTSReset, WTSDown, WTSInit] {
            assert!(validate_streaming_session(state, true).is_err());
        }
    }
    #[test]
    fn streaming_session_preserves_active_sessions_and_console_sign_in() {
        assert!(validate_streaming_session(WTSActive, false).is_ok());
        for state in [WTSActive, WTSConnected, WTSDisconnected] {
            assert!(validate_streaming_session(state, true).is_ok());
        }
    }
    #[test]
    fn streaming_gate_queries_the_host_session_instead_of_the_console() {
        use super::*;
        for (state, allowed) in [(WTSActive, true), (WTSDisconnected, false)] {
            let result = ensure_streaming_session_with(
                || Ok(4),
                || 1,
                |session| {
                    assert_eq!(session, 4, "query the host's own RDP session");
                    Ok((state, 2))
                },
                |_| panic!("only the console exception lists other sessions"),
            );
            assert_eq!(result.is_ok(), allowed);
        }
        for state in [WTSConnected, WTSDisconnected] {
            assert!(
                ensure_streaming_session_with(|| Ok(1), || 1, |_| Ok((state, 0)), |_| Ok(vec![]))
                    .is_ok()
            );
            assert!(
                ensure_streaming_session_with(
                    || Ok(1),
                    || 1,
                    |_| Ok((state, 2)),
                    |_| panic!("a refused session needs no session list")
                )
                .is_err()
            );
        }
    }
    #[test]
    fn user_programs_start_in_the_hosts_own_child_session_not_the_consoles() {
        // A host inside child session 3 while the console (session 1) is the
        // user's main desktop: its capture worker gets session 3's user.
        let asked = super::own_session_user(|| Ok(3), Ok).unwrap();
        assert_eq!(asked, 3);
        let error = super::own_session_user::<u32>(
            || Err(anyhow::anyhow!("no session")),
            |_| panic!("no session, no user token"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("no session"));
    }
    fn warnings(check: impl FnOnce()) -> String {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("session.log");
        let file = std::fs::File::create(&path).unwrap();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_target(false)
            .with_max_level(tracing::Level::WARN)
            .with_writer(move || file.try_clone().unwrap())
            .finish();
        tracing::subscriber::with_default(subscriber, check);
        std::fs::read_to_string(path).unwrap()
    }
    #[test]
    fn refused_streaming_session_is_logged_at_warn_with_the_checked_session() {
        use super::*;
        let log = warnings(|| {
            let error = ensure_streaming_session_with(
                || Ok(4),
                || 1,
                |_| Ok((WTSDisconnected, 2)),
                |_| panic!("a refused session needs no session list"),
            )
            .unwrap_err();
            assert!(error.to_string().contains("disconnected or changing"));
        });
        for field in [
            "WARN",
            "session_id=4",
            "console_session_id=1",
            "state=4",
            "protocol=2",
            "allowed=false",
            "decision=\"disconnected_or_changing\"",
        ] {
            assert!(log.contains(field), "missing {field} in {log}");
        }
        // Accepted active sessions stay out of the warning log.
        let log = warnings(|| {
            ensure_streaming_session_with(
                || Ok(1),
                || 1,
                |_| Ok((WTSActive, 0)),
                |_| panic!("an active session needs no session list"),
            )
            .unwrap();
        });
        assert!(log.is_empty(), "{log}");
    }
    #[test]
    fn sign_in_screen_launch_warns_about_a_disconnected_desktop_without_blocking() {
        use super::*;
        // Reporter's host: the service runs on console session 4 at the
        // sign-in screen while the RDP desktop waits as disconnected session 1.
        let log = warnings(|| {
            ensure_streaming_session_with(
                || Ok(4),
                || 4,
                |_| Ok((WTSConnected, 0)),
                |host| {
                    assert_eq!(host, 4);
                    Ok(vec![1])
                },
            )
            .unwrap();
        });
        for field in [
            "WARN",
            "session_id=4",
            "state=1",
            "disconnected_session_ids=[1]",
            "reconnect Moonlight",
        ] {
            assert!(log.contains(field), "missing {field} in {log}");
        }
        // No disconnected desktop, or no session list, keeps the existing
        // sign-in support silent and allowed.
        for list in [Ok(vec![]), Err(anyhow::anyhow!("enumeration failed"))] {
            let log = warnings(|| {
                ensure_streaming_session_with(|| Ok(4), || 4, |_| Ok((WTSConnected, 0)), |_| list)
                    .unwrap();
            });
            assert!(log.is_empty(), "{log}");
        }
    }
    #[test]
    fn disconnected_desktops_exclude_services_the_host_and_sessions_without_a_user() {
        use super::*;
        let sessions = [
            (0, WTSDisconnected),
            (1, WTSDisconnected),
            (2, WTSDisconnected),
            (3, WTSActive),
            (4, WTSConnected),
            (5, WTSDisconnected),
            (6, WTSDisconnected),
        ];
        let found = disconnected_desktops(6, &sessions, |session| match session {
            1 => Ok(true),
            2 => Ok(false),
            5 => anyhow::bail!("user name unavailable"),
            0 | 6 => panic!("session {session} is never a candidate"),
            _ => panic!("only disconnected sessions are checked, not {session}"),
        });
        assert_eq!(found, vec![1]);
    }
    #[test]
    fn streaming_gate_fails_closed_when_the_host_session_cannot_be_queried() {
        use super::*;
        let error = ensure_streaming_session_with(
            || anyhow::bail!("process session unavailable"),
            || panic!("do not fall back to the console session"),
            |_| panic!("do not query a substitute session"),
            |_| panic!("do not list sessions without the host session"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("process session unavailable"));
        let error = ensure_streaming_session_with(
            || Ok(4),
            || 1,
            |session| {
                assert_eq!(session, 4);
                anyhow::bail!("connection state unavailable")
            },
            |_| panic!("do not list sessions without a connection state"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("connection state unavailable"));
    }
    #[test]
    fn the_process_list_names_this_test_and_its_program() {
        let me = std::process::id();
        let list = super::processes().unwrap();
        let this = list.iter().find(|p| p.pid == me).unwrap();
        assert!(this.started > 0);
        assert!(this.name.to_ascii_lowercase().ends_with(".exe"));
        let image = super::image_path(me).unwrap();
        assert!(image.ends_with(&this.name), "{image}");
        // Stopping a process list whose start times do not match leaves it alone.
        super::stop_processes(
            &std::collections::BTreeMap::from([(me, this.started + 1)]),
            std::time::Duration::ZERO,
        );
    }
    #[test]
    fn urls_and_documents_open_through_their_association() {
        use super::{command_target, shell_command};
        assert_eq!(
            shell_command("steam://open/bigpicture"),
            "start \"\" /wait \"steam://open/bigpicture\""
        );
        assert_eq!(
            shell_command("ms-settings:display"),
            "start \"\" /wait \"ms-settings:display\""
        );
        assert_eq!(
            shell_command("\"C:\\Games\\My Game.lnk\" -windowed"),
            "start \"\" /wait \"C:\\Games\\My Game.lnk\" -windowed"
        );
        for unchanged in [
            "\"C:\\Program Files\\Game\\game.exe\" -fullscreen",
            "C:\\tools\\setup.bat",
            "powershell -NoProfile -Command Get-Date",
            "start \"\" steam://run/570",
            "",
        ] {
            assert_eq!(shell_command(unchanged), unchanged);
        }
        assert_eq!(command_target("\"C:\\a b\\c.exe\" x"), "C:\\a b\\c.exe");
    }
    #[test]
    fn supervisor_event_is_wait_only_and_outlives_the_source_handle() -> Result<()> {
        let source = HostStop::new()?;
        let (pid, handle) = parse_stop_source(&source.source())?;
        let signal = StopSignal(Some(duplicate_stop_event(pid, handle)?));
        assert!(!signal.requested());
        let event = raw(signal.0.as_ref().unwrap());
        assert_eq!(
            // SAFETY: `event` is a handle owned by `signal`, which is alive here.
            unsafe { SetEvent(event) }.unwrap_err().code(),
            windows::core::HRESULT::from_win32(ERROR_ACCESS_DENIED.0)
        );
        source.request()?;
        drop(source);
        assert!(signal.requested());
        // SAFETY: `event` is a handle owned by `signal`, which is alive here.
        assert!(unsafe { ResetEvent(event) }.is_err());
        Ok(())
    }
    #[test]
    fn shutdown_source_rejects_invalid_handles_and_foreign_parents() -> Result<()> {
        for source in [
            "",
            "1",
            "0:1",
            "1:0",
            "1:-1",
            "1:1:2",
            "+1:2",
            "1:xyz",
            "4294967296:1",
            "1:ffffffffffffffffffffffff",
        ] {
            assert!(parse_stop_source(source).is_err(), "{source}");
        }
        assert!(StopSignal::new(Some(&HostStop::new()?.source())).is_err());
        assert!(!StopSignal::new(None)?.requested());
        Ok(())
    }
    #[test]
    fn shell_runs_grouped_commands_with_quoted_output_paths() {
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("quoted output.log");
        let command = format!("(echo NATIVE_SHELL) 1>\"{}\" 2>&1", output.display());
        let child = super::Process::shell(
            &command,
            Some(directory.path()),
            target_token(&Target::User { elevated: true }).is_ok(),
            &Default::default(),
        )
        .unwrap();
        assert_eq!(child.wait(std::time::Duration::from_secs(5)).unwrap(), 0);
        assert_eq!(
            std::fs::read_to_string(output).unwrap().trim(),
            "NATIVE_SHELL"
        );
    }
    #[test]
    fn portable_non_elevated_targets_never_reuse_an_elevated_token() -> Result<()> {
        if is_system() {
            return Ok(());
        }
        let selected = match target_token(&Target::User { elevated: false }) {
            Ok(selected) => selected,
            Err(_) => {
                assert!(Process::shell("exit /b 0", None, false, &BTreeMap::new()).is_err());
                return Ok(());
            }
        };
        let current = token()?;
        // With User Account Control off the desktop shell is elevated too and
        // no standard-rights token exists: the program gets the desktop's
        // rights, never more than the user's own desktop has.
        if token_elevated(&current)? && shell_token(&current)?.is_none() {
            assert!(selected.is_none());
            return Ok(());
        }
        let mut info = TOKEN_ELEVATION::default();
        let mut bytes = 0;
        // SAFETY: the selected/current token and TOKEN_ELEVATION output live through the call.
        unsafe {
            GetTokenInformation(
                raw(selected.as_ref().unwrap_or(&current)),
                TokenElevation,
                Some((&mut info as *mut TOKEN_ELEVATION).cast()),
                size_of::<TOKEN_ELEVATION>() as u32,
                &mut bytes,
            )?;
        }
        assert_eq!(info.TokenIsElevated, 0);
        Ok(())
    }
    #[test]
    fn administrator_hosts_start_non_elevated_programs_with_standard_rights() {
        if is_system() {
            return;
        }
        // An elevated portable host previously failed every non-elevated
        // program with ERROR_BAD_IMPERSONATION_LEVEL.
        let environment = user_environment().unwrap();
        assert!(environment.contains_key("USERPROFILE"));
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("groups.txt");
        // Windows' own whoami: a PATH with Git's usr\bin first finds one without /groups.
        let windir = std::path::PathBuf::from(std::env::var_os("WINDIR").unwrap());
        let whoami = windir.join("System32\\whoami.exe");
        let child = Process::shell(
            // Unquoted: cmd /c would strip the first and last of several quotes.
            &format!(
                "{} /groups 1>\"{}\" 2>&1",
                whoami.display(),
                output.display()
            ),
            None,
            false,
            &BTreeMap::new(),
        )
        .unwrap();
        // whoami /groups took 12 s on a PC busy with parallel builds: the limit only
        // catches a hang, not slowness.
        let code = child.wait(Duration::from_secs(60)).unwrap();
        // Localized output uses the OEM code page; the SIDs are ASCII.
        let groups =
            String::from_utf8_lossy(&std::fs::read(output).unwrap_or_default()).into_owned();
        assert_eq!(code, 0, "{groups}");
        // Medium mandatory level, never High, unless User Account Control is
        // off (as on CI runners): then the desktop itself is elevated and the
        // program gets what the desktop would give it rather than failing.
        let own = token().unwrap();
        let standard = !token_elevated(&own).unwrap() || shell_token(&own).unwrap().is_some();
        assert_eq!(groups.contains("S-1-16-8192"), standard, "{groups}");
        assert_eq!(groups.contains("S-1-16-12288"), !standard, "{groups}");
    }
    #[test]
    fn programs_started_without_a_token_get_this_hosts_environment() {
        // A non-elevated host gave its programs the system's variables, and the PyroWave probe
        // worker missed the AMD shader cache under C:\Users\Default, compiled every pipeline
        // again and timed out.
        let environment = environment(None).unwrap();
        for name in ["USERPROFILE", "USERNAME", "TEMP", "PATH"] {
            assert_eq!(
                environment.get(name).map(String::as_str),
                std::env::var(name).ok().as_deref(),
                "{name}"
            );
        }
        if !is_system() {
            // Apps and client commands get this one, from whichever token the host uses.
            let user = user_environment().unwrap();
            for name in ["USERPROFILE", "USERNAME"] {
                assert_eq!(user.get(name), environment.get(name), "{name}");
            }
        }
    }
    #[test]
    fn programs_started_for_the_user_keep_the_users_profile() {
        if is_system() {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("user.txt");
        // `/u` writes UTF-16, so a profile name outside the OEM code page reads back intact.
        let child = Process::shell(
            &format!("cmd /d /u /c set USER 1>\"{}\" 2>&1", output.display()),
            None,
            false,
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(child.wait(Duration::from_secs(10)).unwrap(), 0);
        let bytes = std::fs::read(output).unwrap();
        let wide: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .collect();
        let text = String::from_utf16_lossy(&wide);
        let seen: BTreeMap<_, _> = text
            .lines()
            .filter_map(|line| line.split_once('='))
            .map(|(name, value)| (name.to_uppercase(), value.to_owned()))
            .collect();
        for name in ["USERPROFILE", "USERNAME"] {
            assert_eq!(
                seen.get(name),
                std::env::var(name).ok().as_ref(),
                "{name}: {text}"
            );
        }
    }
    #[test]
    fn portable_user_identity_does_not_require_a_non_elevated_token() {
        if !is_system() {
            assert!(user_sid().is_some_and(|sid| !sid.is_empty()));
        }
    }
    use super::*;
    #[test]
    fn dropping_the_job_terminates_the_shell_and_its_child() {
        let process = Process::shell(
            "ping -n 30 127.0.0.1 >nul",
            None,
            target_token(&Target::User { elevated: true }).is_ok(),
            &BTreeMap::new(),
        )
        .unwrap();
        let mut ids = [0usize; 34];
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let count = loop {
            // SAFETY: `ids` outlives the call and its byte size is passed.
            unsafe {
                QueryInformationJobObject(
                    Some(raw(&process.job)),
                    JobObjectBasicProcessIdList,
                    ids.as_mut_ptr().cast(),
                    std::mem::size_of_val(&ids) as u32,
                    None,
                )
                .unwrap();
            }
            // SAFETY: `ids` holds a JOBOBJECT_BASIC_PROCESS_ID_LIST whose second u32, inside the
            // array, is NumberOfProcessIdsInList.
            let count = unsafe { *ids.as_ptr().cast::<u32>().add(1) } as usize;
            if count >= 2 {
                break count;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "child never joined the owned Windows job"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(count <= 32);
        let children: Vec<_> = ids[1..=count]
            .iter()
            .map(|id| {
                // SAFETY: OpenProcess has no memory preconditions and the returned handle is owned
                // once.
                owned(unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, *id as u32).unwrap() })
            })
            .collect();
        drop(process);
        for child in children {
            assert_eq!(
                // SAFETY: `child` is an owned process handle, open for the wait.
                unsafe { WaitForSingleObject(raw(&child), 5000) },
                WAIT_OBJECT_0
            );
        }
    }
}
