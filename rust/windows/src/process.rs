//! User-session process creation with ownership of the complete Windows job.

use anyhow::{Context, Result, bail};
use butterpollo_core::nucleus::elevation::Account;
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
mod security;
mod session;
pub use security::*;
pub use session::*;
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
pub enum Target {
    User {
        elevated: bool,
    },
    /// The signed-in user with their standard rights, only while they are
    /// the account of this SID: the one whose files a launch checked.
    Account(String),
    SystemSession(u32),
}
/// Whether this process runs with administrator rights (elevated, or as SYSTEM).
pub fn elevated() -> Result<bool> {
    token_elevated(&token()?)
}
/// What Windows gives the user signed in to this host's session to start a
/// program elevated with. As the service: their elevation-linked token when
/// it is elevated and theirs; one of another account is the separate
/// administrator of Windows' Administrator protection, which Rubylight never
/// starts a program as. Outside the service, the host's own rights.
pub fn user_account() -> Result<Account> {
    if !is_system() {
        return Ok(match user_runs_elevated()? {
            true => Account::Elevates,
            false => Account::Standard,
        });
    }
    let user = target_token(&Target::User { elevated: false })?
        .context("no signed-in user in the host session")?;
    let linked = match linked_token(&user) {
        Some(linked) => Some((
            token_elevated(&linked)?,
            token_user_sid(&linked)? == token_user_sid(&user)?,
        )),
        None => None,
    };
    Ok(account_of(
        linked,
        token_elevated(&user)?,
        token_in_administrators(&user)?,
    ))
}
/// The account behind a signed-in user's token: `linked`, its
/// elevation-linked token's elevation and whether it is the same account;
/// `elevated`, the token's own; `administrator`, whether its groups name
/// Administrators (for deny only, in an administrator's filtered token).
fn account_of(linked: Option<(bool, bool)>, elevated: bool, administrator: bool) -> Account {
    match linked {
        // User Account Control's split token.
        Some((true, true)) => Account::Elevates,
        // Administrator protection's separate administrator.
        Some((true, false)) => Account::Protected,
        // User Account Control off: the token is elevated itself.
        _ if elevated => Account::Elevates,
        // An administrator Windows links no elevated token of their own to.
        _ if administrator => Account::Protected,
        _ => Account::Standard,
    }
}
/// The token Windows links to `user`'s for elevation, if it links one.
fn linked_token(user: &OwnedHandle) -> Option<OwnedHandle> {
    let mut linked = TOKEN_LINKED_TOKEN::default();
    let mut bytes = 0;
    // SAFETY: `linked` is a writable TOKEN_LINKED_TOKEN and the size passed
    // is its size; the handle Windows returns is wrapped exactly once.
    unsafe {
        GetTokenInformation(
            raw(user),
            TokenLinkedToken,
            Some((&mut linked as *mut TOKEN_LINKED_TOKEN).cast()),
            size_of::<TOKEN_LINKED_TOKEN>() as u32,
            &mut bytes,
        )
        .ok()?;
    }
    (!linked.LinkedToken.is_invalid()).then(|| owned(linked.LinkedToken))
}
/// Whether `token`'s groups name the Administrators group, enabled or for
/// deny only.
fn token_in_administrators(token: &OwnedHandle) -> Result<bool> {
    let mut needed = 0;
    // SAFETY: the first call only asks for the size. The buffer is
    // usize-aligned and as large as Windows asked; the TOKEN_GROUPS in it,
    // its array and its SIDs are read only while it lives.
    unsafe {
        let _ = GetTokenInformation(raw(token), TokenGroups, None, 0, &mut needed);
        anyhow::ensure!(
            needed as usize >= size_of::<TOKEN_GROUPS>(),
            "Windows returned no token groups"
        );
        let mut buffer = vec![0usize; (needed as usize).div_ceil(size_of::<usize>())];
        GetTokenInformation(
            raw(token),
            TokenGroups,
            Some(buffer.as_mut_ptr().cast()),
            (buffer.len() * size_of::<usize>()) as u32,
            &mut needed,
        )?;
        let groups = &*buffer.as_ptr().cast::<TOKEN_GROUPS>();
        let groups = std::slice::from_raw_parts(groups.Groups.as_ptr(), groups.GroupCount as usize);
        Ok(groups
            .iter()
            .any(|group| IsWellKnownSid(group.Sid, WinBuiltinAdministratorsSid).as_bool()))
    }
}
/// Whether a program started as `Target::User { elevated: true }` runs
/// elevated. A standard user has no elevated token, so it gets the user's
/// standard rights.
pub fn user_runs_elevated() -> Result<bool> {
    match target_token(&Target::User { elevated: true })? {
        Some(token) => token_elevated(&token),
        None => elevated(),
    }
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
/// host's own session, never the console's. A host in another session
/// starts its capture worker (and games) on that session's desktop, not on
/// the main desktop the user is working on.
fn own_session_user<T>(
    current: impl FnOnce() -> Result<u32>,
    user: impl FnOnce(u32) -> Result<T>,
) -> Result<T> {
    user(current()?)
}
/// The token a program started as `target` runs with; None: this process's
/// own. Only ever the signed-in user's own account: a [`Target::Account`]
/// start is refused once another account is signed in.
fn target_token(target: &Target) -> Result<Option<OwnedHandle>> {
    let token = session_token(target)?;
    if let Target::Account(expected) = target {
        let found = match &token {
            Some(token) => token_user_sid(token)?,
            None => own_sid()?,
        };
        same_account(expected, &found)?;
    }
    Ok(token)
}
/// A start for the account `expected` ([`Target::Account`]) goes ahead only
/// as that account: `found` is the one whose token it would run with.
fn same_account(expected: &str, found: &str) -> Result<()> {
    anyhow::ensure!(
        expected == found,
        "another account signed in on this PC since the launch began: start it again"
    );
    Ok(())
}
/// An elevated start runs only as the signed-in user's own account: `user`
/// is the SID of their token, `linked` that of the elevation-linked token
/// Windows gives for it. Under Windows' Administrator protection that is a
/// separate administrator account, which Rubylight never starts a program
/// as.
fn own_linked_account(user: &str, linked: &str) -> Result<()> {
    anyhow::ensure!(
        user == linked,
        "Starting it elevated needs administrator rights, and {}",
        butterpollo_core::nucleus::elevation::PROTECTED
    );
    Ok(())
}
fn session_token(target: &Target) -> Result<Option<OwnedHandle>> {
    if !is_system() {
        if matches!(target, Target::SystemSession(_)) {
            bail!("SYSTEM session creation requires the Windows service");
        }
        let token = token()?;
        let elevated = token_elevated(&token)?;
        if matches!(target, Target::User { elevated: true }) && !elevated {
            bail!("this elevated command requires the installed Windows service");
        }
        if matches!(
            target,
            Target::User { elevated: false } | Target::Account(_)
        ) && elevated
        {
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
            Target::User { .. } | Target::Account(_) => {
                let mut user = own_session_user(current_session, |session| {
                    let mut user = HANDLE::default();
                    WTSQueryUserToken(session, &mut user)
                        .context("no signed-in user in the host session")?;
                    Ok(owned(user))
                })?;
                if matches!(target, Target::User { elevated: true })
                    && let Some(linked) = linked_token(&user)
                {
                    own_linked_account(&token_user_sid(&user)?, &token_user_sid(&linked)?)?;
                    user = linked;
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
    /// Whether Windows gave this process administrator rights: its token,
    /// which is not always what its [`Target`] asked for (an administrator
    /// with User Account Control off has only their full token).
    pub fn elevated(&self) -> Result<bool> {
        // SAFETY: `self.handle` is the child's owned process handle, and
        // `token` a valid out-pointer whose handle is owned once filled.
        let token = unsafe {
            let mut token = HANDLE::default();
            OpenProcessToken(raw(&self.handle), TOKEN_QUERY, &mut token)
                .context("opening the process's token")?;
            owned(token)
        };
        token_elevated(&token)
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
        if timeout.is_zero() || self.active_processes()? == 0 {
            return self.stop();
        }
        if self.close_windows(self.process_ids()?.into_iter().collect())? {
            let deadline = std::time::Instant::now() + timeout;
            while self.active_processes()? != 0 && std::time::Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        self.stop()
    }
    /// The job's processes whose image is `image`: the one it started, or
    /// one that restarted itself.
    fn image_ids(&self, image: &str) -> Result<std::collections::BTreeSet<u32>> {
        let job: std::collections::BTreeSet<u32> = self.process_ids()?.into_iter().collect();
        Ok(processes()?
            .into_iter()
            .filter(|p| job.contains(&p.pid) && p.name.eq_ignore_ascii_case(image))
            .map(|p| p.pid)
            .collect())
    }
    /// Whether a process of the job whose image is `image` still runs.
    pub fn running_image(&self, image: &str) -> Result<bool> {
        Ok(!self.image_ids(image)?.is_empty())
    }
    /// Ask the windows of the job's `image` processes to close and wait up to
    /// `timeout` for them to exit. Nothing is ended, and the job's other
    /// processes (a browser or a game they started) are left alone: whether
    /// they all exited.
    pub fn close_image(&self, image: &str, timeout: Duration) -> Result<bool> {
        let ids = self.image_ids(image)?;
        if ids.is_empty() {
            return Ok(true);
        }
        if self.close_windows(ids)? {
            let deadline = std::time::Instant::now() + timeout;
            while self.running_image(image)? && std::time::Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        Ok(!self.running_image(image)?)
    }
    /// Post WM_CLOSE to each top-level window of the processes `ids`: whether
    /// one was sent.
    fn close_windows(&self, ids: std::collections::BTreeSet<u32>) -> Result<bool> {
        use windows::Win32::UI::WindowsAndMessaging::*;
        struct Closing {
            ids: std::collections::BTreeSet<u32>,
            sent: bool,
        }
        unsafe extern "system" fn close_window(window: HWND, data: LPARAM) -> BOOL {
            // SAFETY: EnumWindows passes the `&mut Closing` from close_windows as `data`, which
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
        let mut closing = Closing { ids, sent: false };
        // SAFETY: `closing` outlives the synchronous EnumWindows call that reads it through
        // `close_window`.
        unsafe {
            EnumWindows(
                Some(close_window),
                LPARAM((&mut closing as *mut Closing) as isize),
            )?;
        }
        Ok(closing.sent)
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
/// What a command line passes its program: everything after
/// [`command_target`], trimmed.
pub fn command_arguments(command: &str) -> &str {
    let command = command.trim_start();
    let rest = match command.strip_prefix('"') {
        Some(rest) => rest.split_once('"').map_or("", |(_, rest)| rest),
        None => command
            .split_once(char::is_whitespace)
            .map_or("", |(_, rest)| rest),
    };
    rest.trim()
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
mod tests;
