//! Starting a program as SYSTEM in a session, inside a job that ends with it,
//! and stopping the host: the part of the host's process code
//! (rust/windows/src/process.rs, `Target::SystemSession`) the service needs.
//! The host reads `--service-stop-source` with that code's `StopSignal`.

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
        System::{Environment::*, JobObjects::*, Threading::*},
    },
    core::{PCWSTR, PWSTR},
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
fn quote(s: &std::ffi::OsStr) -> Result<String> {
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
/// `program` and `args` as one command line, each quoted.
fn command_line(program: &Path, args: &[OsString]) -> Result<String> {
    let mut command = quote(program.as_os_str())?;
    for arg in args {
        command.push(' ');
        command.push_str(&quote(arg)?);
    }
    Ok(command)
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
/// The session attached to the physical console.
pub fn console_session() -> u32 {
    // SAFETY: WTSGetActiveConsoleSessionId takes no arguments and has no preconditions.
    unsafe { windows::Win32::System::RemoteDesktop::WTSGetActiveConsoleSessionId() }
}
/// This process's SYSTEM token, as a primary token for `session`.
fn session_token(session: u32) -> Result<OwnedHandle> {
    if !is_system() {
        bail!("SYSTEM session creation requires the Windows service");
    }
    // SAFETY: every out-pointer is a live local whose size matches the one passed, and the
    // returned token handle is wrapped in OwnedHandle exactly once.
    unsafe {
        let source = token()?;
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
        SetTokenInformation(
            raw(&primary),
            TokenSessionId,
            (&session as *const u32).cast(),
            4,
        )?;
        Ok(primary)
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
/// The environment of a program started with `token`: the variables
/// CreateEnvironmentBlock gives that account, by upper-case name.
fn environment(token: &OwnedHandle) -> Result<BTreeMap<String, String>> {
    // SAFETY: the block from CreateEnvironmentBlock is NUL-terminated UTF-16 strings ending in an
    // empty one, so reads stop at the terminators, and `native` frees it on drop.
    unsafe {
        let mut native = Environment(std::ptr::null_mut());
        CreateEnvironmentBlock(&mut native.0, Some(raw(token)), false)?;
        let mut entries = Vec::new();
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
            entries.push(String::from_utf16_lossy(std::slice::from_raw_parts(p, len)));
            offset += len + 1;
        }
        Ok(environment_entries(entries))
    }
}
/// `NAME=value` entries by upper-case name; a leading `=` belongs to the
/// name (`=C:=C:\`).
fn environment_entries(entries: impl IntoIterator<Item = String>) -> BTreeMap<String, String> {
    let mut environment = BTreeMap::new();
    for entry in entries {
        let split = if let Some(rest) = entry.strip_prefix('=') {
            rest.find('=').map(|i| i + 1)
        } else {
            entry.find('=')
        };
        if let Some(i) = split {
            environment.insert(entry[..i].to_uppercase(), entry[i + 1..].to_owned());
        }
    }
    environment
}
/// An environment block for CreateProcess: `NAME=value` strings, each
/// NUL-terminated, then an empty one.
fn environment_block(environment: &BTreeMap<String, String>) -> Vec<u16> {
    let mut block: Vec<u16> = environment
        .iter()
        .flat_map(|(k, v)| format!("{k}={v}\0").encode_utf16().collect::<Vec<_>>())
        .collect();
    block.extend([0, 0]);
    block
}

/// A new job object that ends everything in it when its last handle closes,
/// with these limits besides.
fn job(limits: JOB_OBJECT_LIMIT) -> Result<OwnedHandle> {
    // SAFETY: the limits are a live local of the size passed, and the new handle is owned once.
    unsafe {
        let job = owned(CreateJobObjectW(None, PCWSTR::null())?);
        let limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
            BasicLimitInformation: JOBOBJECT_BASIC_LIMIT_INFORMATION {
                LimitFlags: limits | JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
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
        Ok(job)
    }
}

/// A job for a task ([`crate::tasks`]): what the task starts stays in it, and
/// all of it ends when the job is ended or dropped, or this service exits.
pub struct Job(OwnedHandle);
impl Job {
    pub fn new() -> Result<Self> {
        Ok(Self(job(JOB_OBJECT_LIMIT(0))?))
    }
    /// Take in `child`, started with CREATE_SUSPENDED, and let it run: it can
    /// start nothing outside the job.
    pub fn adopt(&self, child: &std::process::Child) -> Result<()> {
        // SAFETY: the child's handle stays open while `child` lives.
        unsafe { AssignProcessToJobObject(raw(&self.0), HANDLE(child.as_raw_handle()))? };
        resume(child.id())
    }
    /// End everything in the job.
    pub fn end(&self) -> Result<()> {
        // SAFETY: `self.0` is the owned job handle, open while `self` lives.
        unsafe { TerminateJobObject(raw(&self.0), 1)? };
        Ok(())
    }
}
/// Resume the threads of `pid`, a process started suspended: its main thread.
/// std's Child does not hand that thread out.
fn resume(pid: u32) -> Result<()> {
    use windows::Win32::System::Diagnostics::ToolHelp::*;
    // SAFETY: `entry` is a live local whose dwSize is its size, and every handle is owned once.
    unsafe {
        let snapshot = owned(CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0)?);
        let mut entry = THREADENTRY32 {
            dwSize: size_of::<THREADENTRY32>() as u32,
            ..Default::default()
        };
        let mut resumed = 0;
        let mut more = Thread32First(raw(&snapshot), &mut entry).is_ok();
        while more {
            if entry.th32OwnerProcessID == pid {
                let thread = owned(OpenThread(
                    THREAD_SUSPEND_RESUME,
                    false,
                    entry.th32ThreadID,
                )?);
                if ResumeThread(raw(&thread)) == u32::MAX {
                    return Err(windows::core::Error::from_thread().into());
                }
                resumed += 1;
            }
            more = Thread32Next(raw(&snapshot), &mut entry).is_ok();
        }
        anyhow::ensure!(resumed > 0, "process {pid} has no thread to resume");
    }
    Ok(())
}

/// A program this service started, in a job of its own that ends everything
/// in it when the process is dropped.
pub struct Process {
    handle: OwnedHandle,
    job: OwnedHandle,
    host_stop: Option<HostStop>,
    pub pid: u32,
}
impl Process {
    /// Keep the supervisor's unnamed stop event alive for the entire child.
    /// The child duplicates a wait-only handle; handle inheritance cannot cross
    /// Windows sessions, and session-local object names cannot reach the service.
    pub fn spawn_host(
        program: &Path,
        args: &[OsString],
        directory: Option<&Path>,
        session: u32,
    ) -> Result<Self> {
        let stop = HostStop::new()?;
        let mut args = args.to_vec();
        args.extend(["--service-stop-source".into(), stop.source().into()]);
        let mut process = Self::spawn(program, &args, directory, session)?;
        process.host_stop = Some(stop);
        Ok(process)
    }
    /// Start `program` as SYSTEM in `session`, without a window, on the
    /// interactive desktop.
    pub fn spawn(
        program: &Path,
        args: &[OsString],
        directory: Option<&Path>,
        session: u32,
    ) -> Result<Self> {
        let line = command_line(program, args)?;
        let token = session_token(session)?;
        let program = wide(program.as_os_str())?;
        let directory = directory.map(|p| wide(p.as_os_str())).transpose()?;
        let mut line = wide(std::ffi::OsStr::new(&line))?;
        let env = environment_block(&environment(&token)?);
        // SAFETY: the strings given to CreateProcess are NUL-terminated locals, and each new
        // handle is owned once.
        unsafe {
            let job = job(JOB_OBJECT_LIMIT_BREAKAWAY_OK)?;
            let mut desktop: Vec<u16> = "winsta0\\default\0".encode_utf16().collect();
            let startup = STARTUPINFOW {
                cb: size_of::<STARTUPINFOW>() as u32,
                lpDesktop: PWSTR(desktop.as_mut_ptr()),
                dwFlags: STARTF_USESHOWWINDOW,
                wShowWindow: 0,
                ..Default::default()
            };
            let mut info = PROCESS_INFORMATION::default();
            let current = directory
                .as_ref()
                .map_or(PCWSTR::null(), |d| PCWSTR(d.as_ptr()));
            CreateProcessAsUserW(
                Some(raw(&token)),
                PCWSTR(program.as_ptr()),
                Some(PWSTR(line.as_mut_ptr())),
                None,
                None,
                false,
                CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT | CREATE_NO_WINDOW,
                Some(env.as_ptr().cast()),
                current,
                &startup,
                &mut info,
            )?;
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
    pub fn handle(&self) -> HANDLE {
        raw(&self.handle)
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
    fn stop(&self) -> Result<()> {
        // SAFETY: `self.job` is an owned job handle that stays open while `self` lives.
        unsafe {
            TerminateJobObject(raw(&self.job), 0)?;
        }
        self.wait(Duration::from_secs(10))?;
        Ok(())
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

/// The manual-reset event the host waits on to stop.
struct HostStop(OwnedHandle);
impl HostStop {
    fn new() -> Result<Self> {
        // SAFETY: CreateEventW returns a new event handle that this struct then owns.
        Ok(Self(owned(unsafe {
            CreateEventW(None, true, false, PCWSTR::null())?
        })))
    }
    /// `--service-stop-source`: this process and the event's handle in it,
    /// `<pid>:<hex handle>`, which the host duplicates.
    fn source(&self) -> String {
        stop_source(
            // SAFETY: GetCurrentProcessId has no preconditions.
            unsafe { GetCurrentProcessId() },
            self.0.as_raw_handle() as usize,
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
fn stop_source(pid: u32, handle: usize) -> String {
    format!("{pid}:{handle:x}")
}

/// A manual-reset event the service control handler sets to wake the
/// supervisor.
pub struct Signal(OwnedHandle);
impl Signal {
    pub fn new() -> Result<Self> {
        // SAFETY: Null attributes and name create a new unnamed event that Signal owns.
        Ok(Self(owned(unsafe {
            CreateEventW(None, true, false, PCWSTR::null())?
        })))
    }
    pub fn set(&self) -> Result<()> {
        // SAFETY: `self.0` is the event this Signal owns, open while it lives.
        unsafe { SetEvent(raw(&self.0))? };
        Ok(())
    }
    pub fn reset(&self) -> Result<()> {
        // SAFETY: `self.0` is the event this Signal owns, open while it lives.
        unsafe { ResetEvent(raw(&self.0))? };
        Ok(())
    }
    pub fn raw(&self) -> HANDLE {
        raw(&self.0)
    }
    /// Wait up to `timeout` for the event; whether it is set.
    pub fn wait(&self, timeout: Duration) -> Result<bool> {
        // SAFETY: `self.0` is the event this Signal owns, open while it lives.
        match unsafe { WaitForSingleObject(raw(&self.0), millis(timeout)) } {
            WAIT_OBJECT_0 => Ok(true),
            WAIT_TIMEOUT => Ok(false),
            _ => Err(windows::core::Error::from_thread().into()),
        }
    }
}
/// A wait's timeout in milliseconds, short of INFINITE.
pub fn millis(timeout: Duration) -> u32 {
    timeout.as_millis().min(u128::from(u32::MAX - 1)) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn arguments_are_quoted_as_windows_splits_them() {
        let line = command_line(
            Path::new(r"C:\Program Files\Rubylight\butterpollo.exe"),
            &[
                "--config-dir".into(),
                r"C:\ProgramData\Butterpollo\config".into(),
                r#"say "hi""#.into(),
                r"ends in \".into(),
            ],
        )
        .unwrap();
        assert_eq!(
            line,
            r#""C:\Program Files\Rubylight\butterpollo.exe" "--config-dir" "C:\ProgramData\Butterpollo\config" "say \"hi\"" "ends in \\""#
        );
        assert!(quote(std::ffi::OsStr::new("a\0b")).is_err());
    }
    #[test]
    fn the_stop_source_is_what_the_host_parses() {
        // rust/windows/src/process.rs, parse_stop_source: decimal pid, hex handle.
        assert_eq!(stop_source(1234, 0x2bc), "1234:2bc");
        let stop = HostStop::new().unwrap();
        let source = stop.source();
        let (pid, handle) = source.split_once(':').unwrap();
        assert_eq!(pid.parse::<u32>().unwrap(), std::process::id());
        assert_eq!(
            usize::from_str_radix(handle, 16).unwrap(),
            stop.0.as_raw_handle() as usize
        );
    }
    #[test]
    fn the_environment_block_keeps_drive_variables_and_upper_cases_names() {
        let environment = environment_entries(
            [
                "Path=C:\\Windows",
                "=C:=C:\\",
                "windir=C:\\Windows",
                "broken",
            ]
            .map(String::from),
        );
        assert_eq!(
            environment,
            BTreeMap::from([
                ("=C:".into(), "C:\\".into()),
                ("PATH".into(), "C:\\Windows".into()),
                ("WINDIR".into(), "C:\\Windows".into()),
            ])
        );
        let block = environment_block(&environment);
        assert_eq!(
            String::from_utf16(&block).unwrap(),
            "=C:=C:\\\0PATH=C:\\Windows\0WINDIR=C:\\Windows\0\0\0"
        );
    }
    #[test]
    fn only_system_starts_programs_in_a_session() {
        if is_system() {
            return;
        }
        let error = Process::spawn(
            Path::new(r"C:\Windows\System32\cmd.exe"),
            &[],
            None,
            console_session(),
        )
        .err()
        .unwrap();
        assert_eq!(
            error.to_string(),
            "SYSTEM session creation requires the Windows service"
        );
    }
    #[test]
    fn a_set_signal_stays_set_until_reset() {
        let signal = Signal::new().unwrap();
        let state = || {
            // SAFETY: the event stays open for the zero-timeout wait.
            unsafe { WaitForSingleObject(signal.raw(), 0) }
        };
        assert_eq!(state(), WAIT_TIMEOUT);
        signal.set().unwrap();
        assert_eq!(state(), WAIT_OBJECT_0);
        assert_eq!(state(), WAIT_OBJECT_0);
        signal.reset().unwrap();
        assert_eq!(state(), WAIT_TIMEOUT);
    }
}
