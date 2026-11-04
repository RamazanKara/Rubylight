//! Sleep PC: whether Windows can sleep, and the suspend itself, as
//! Vibepollo does it (`src/power_action_native.cpp`). It asks Windows for
//! sleep with wake events enabled, and only on a PC with S1, S2 or S3: a
//! Modern Standby laptop with hibernate on hibernated instead.
use anyhow::{Context, Result, bail};
use butterpollo_core::power_action::SleepStates;
use windows::{
    Win32::{
        Foundation::{
            CloseHandle, ERROR_NO_TOKEN, ERROR_SUCCESS, GetLastError, HANDLE, LUID, SetLastError,
        },
        Security::{
            AdjustTokenPrivileges, ImpersonateSelf, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW,
            RevertToSelf, SE_PRIVILEGE_ENABLED, SecurityImpersonation, TOKEN_ADJUST_PRIVILEGES,
            TOKEN_PRIVILEGES, TOKEN_QUERY,
        },
        System::{
            Power::{GetPwrCapabilities, SYSTEM_POWER_CAPABILITIES, SetSuspendState},
            Threading::{GetCurrentThread, OpenThreadToken},
        },
    },
    core::{HRESULT, w},
};

fn states(caps: &SYSTEM_POWER_CAPABILITIES) -> SleepStates {
    SleepStates {
        s1: caps.SystemS1,
        s2: caps.SystemS2,
        s3: caps.SystemS3,
        modern_standby: caps.AoAc,
        hibernate: caps.SystemS4 && caps.HiberFilePresent,
    }
}

/// The sleep states this PC reports.
pub fn sleep_states() -> Result<SleepStates> {
    let mut caps = SYSTEM_POWER_CAPABILITIES::default();
    // SAFETY: caps is a writable, correctly sized structure for this call.
    if !unsafe { GetPwrCapabilities(&mut caps) } {
        bail!(
            "could not read this PC's power capabilities: {}",
            windows::core::Error::from_thread()
        );
    }
    Ok(states(&caps))
}

/// The calling thread's token with shutdown privilege enabled, restored
/// (and the impersonation ended) on drop. Only this thread's token changes,
/// not the whole host process's.
pub(crate) struct ShutdownPrivilege {
    token: HANDLE,
    impersonated: bool,
    previous: Option<TOKEN_PRIVILEGES>,
}
impl Drop for ShutdownPrivilege {
    fn drop(&mut self) {
        // SAFETY: token is this thread's open token, owned by this guard and
        // closed once; previous is the state AdjustTokenPrivileges returned.
        unsafe {
            if let Some(previous) = &self.previous {
                let _ = AdjustTokenPrivileges(self.token, false, Some(previous), 0, None, None);
            }
            let _ = CloseHandle(self.token);
            if self.impersonated {
                let _ = RevertToSelf();
            }
        }
    }
}
fn thread_token() -> windows::core::Result<HANDLE> {
    let mut token = HANDLE::default();
    // SAFETY: token is a writable handle slot; the pseudo-handle needs no close.
    unsafe {
        OpenThreadToken(
            GetCurrentThread(),
            TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
            true,
            &mut token,
        )?
    };
    Ok(token)
}
/// `SeShutdownPrivilege` on this thread until the guard drops; sleep after
/// idle (`wake::suspend`) uses it too.
pub(crate) fn shutdown_privilege() -> Result<ShutdownPrivilege> {
    let (token, impersonated) = match thread_token() {
        Ok(token) => (token, false),
        Err(e) if e.code() == HRESULT::from_win32(ERROR_NO_TOKEN.0) => {
            // SAFETY: impersonates on this thread only; reverted on any failure
            // here and otherwise by the guard.
            unsafe { ImpersonateSelf(SecurityImpersonation)? };
            match thread_token() {
                Ok(token) => (token, true),
                Err(e) => {
                    // SAFETY: ends the impersonation started above.
                    let _ = unsafe { RevertToSelf() };
                    return Err(e.into());
                }
            }
        }
        Err(e) => return Err(e.into()),
    };
    let mut guard = ShutdownPrivilege {
        token,
        impersonated,
        previous: None,
    };
    let mut luid = LUID::default();
    // SAFETY: the name is a static terminated string and luid is writable.
    unsafe { LookupPrivilegeValueW(None, w!("SeShutdownPrivilege"), &mut luid)? };
    let privileges = TOKEN_PRIVILEGES {
        PrivilegeCount: 1,
        Privileges: [LUID_AND_ATTRIBUTES {
            Luid: luid,
            Attributes: SE_PRIVILEGE_ENABLED,
        }],
    };
    let mut previous = TOKEN_PRIVILEGES::default();
    let mut size = 0;
    // SAFETY: the token is open with TOKEN_ADJUST_PRIVILEGES; previous and size
    // are writable and previous is as large as the buffer length given.
    unsafe {
        SetLastError(ERROR_SUCCESS);
        AdjustTokenPrivileges(
            guard.token,
            false,
            Some(&privileges),
            size_of::<TOKEN_PRIVILEGES>() as u32,
            Some(&mut previous),
            Some(&mut size),
        )?;
        // Success without the privilege reports ERROR_NOT_ALL_ASSIGNED.
        let error = GetLastError();
        if error != ERROR_SUCCESS {
            bail!(
                "could not enable the shutdown privilege: {}",
                windows::core::Error::from_hresult(HRESULT::from_win32(error.0))
            );
        }
    }
    guard.previous = Some(previous);
    Ok(guard)
}

/// Ask Windows to sleep, on a PC with S1, S2 or S3 only. Windows may still
/// refuse; the call returns after the PC wakes.
pub fn suspend() -> Result<()> {
    let states = sleep_states()?;
    if let Some(reason) = states.unavailable() {
        bail!("Sleep PC is unavailable: {reason} ({})", states.describe());
    }
    let _privilege = shutdown_privilege()?;
    tracing::warn!("Sleep PC suspending: {}", states.describe());
    // SAFETY: plain call; hibernate not asked for, not forced, wake events kept.
    if !unsafe { SetSuspendState(false, false, false) } {
        bail!(
            "Windows refused to sleep: {}",
            windows::core::Error::from_thread()
        );
    }
    Ok(())
}

/// `InitiateShutdownW`'s flags: a full shutdown or a restart, never forced
/// and never hybrid, so apps with unsaved work can still say no and a wired
/// adapter's wake from off stays armed.
pub fn shutdown_flags(restart: bool) -> u32 {
    use windows::Win32::System::Shutdown::{SHUTDOWN_POWEROFF, SHUTDOWN_RESTART};
    if restart {
        SHUTDOWN_RESTART.0
    } else {
        SHUTDOWN_POWEROFF.0
    }
}

/// `shutdown.exe`'s arguments for [`power_off`] from the user's session:
/// `/t 0`, since any other timeout implies `/f`, and never `/f` or `/hybrid`.
pub fn shutdown_args(restart: bool) -> [&'static str; 3] {
    [if restart { "/r" } else { "/s" }, "/t", "0"]
}
/// How long `shutdown.exe` may take to hand the request to Windows.
const SHUTDOWN_EXE_WAIT: std::time::Duration = std::time::Duration::from_secs(15);

/// Restart or shut down this PC (Restart and shut down PC). Returns once
/// Windows took the request; its refusal (a shutdown already under way, no
/// shutdown right) is the error. A SYSTEM host asks from the signed-in
/// user's session: Windows refuses a SYSTEM caller while a user is signed
/// in (`ERROR_SHUTDOWN_USERS_LOGGED_ON`) unless forced, and it never
/// forces. With nobody signed in, or as a portable host running as the
/// user, it asks itself.
pub fn power_off(restart: bool) -> Result<()> {
    if crate::process::is_system() && crate::process::user_signed_in() {
        return power_off_as_user(restart);
    }
    initiate_shutdown(restart)
}

fn power_off_as_user(restart: bool) -> Result<()> {
    use crate::process::{Process, Target};
    use std::io::Read;
    let what = if restart { "restart" } else { "shut down" };
    let program = std::path::PathBuf::from(
        std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into()),
    )
    .join("System32")
    .join("shutdown.exe");
    let args: Vec<std::ffi::OsString> = shutdown_args(restart).iter().map(Into::into).collect();
    tracing::warn!("asking Windows to {what} the PC from the signed-in user's session");
    let mut child = Process::spawn_piped(&program, &args, Target::User { elevated: false })
        .with_context(|| format!("could not ask Windows to {what} the PC as the signed-in user"))?;
    drop(child.stdin.take());
    let code = match child.process.wait(SHUTDOWN_EXE_WAIT) {
        Ok(code) => code,
        Err(error) => {
            let _ = child.process.terminate();
            bail!("{}: {error:#}", unanswered(what));
        }
    };
    if code != 0 {
        // It has exited, so its pipes end.
        let mut said = Vec::new();
        for pipe in [child.stderr.take(), child.stdout.take()]
            .into_iter()
            .flatten()
        {
            let _ = { pipe }.read_to_end(&mut said);
        }
        bail!("{}", refusal(what, code, &said));
    }
    Ok(())
}
/// `shutdown.exe`'s timeout: Windows may have taken the request anyway.
fn unanswered(what: &str) -> String {
    format!("Windows did not confirm the request in time; the PC may still {what}")
}
/// `shutdown.exe`'s refusal. Its exit code is the Win32 error, whose text
/// comes in the UI language; its output is in the OEM code page, so it is
/// only the lossy fallback for a code without a text.
fn refusal(what: &str, code: u32, output: &[u8]) -> String {
    let mut said = windows::core::Error::from_hresult(HRESULT::from_win32(code)).message();
    if said.trim().is_empty() {
        said = String::from_utf8_lossy(output).into_owned();
    }
    let said = said.split_whitespace().collect::<Vec<_>>().join(" ");
    if said.is_empty() {
        format!("Windows refused to {what} the PC (exit code {code})")
    } else {
        format!("Windows refused to {what} the PC: {said} (exit code {code})")
    }
}

fn initiate_shutdown(restart: bool) -> Result<()> {
    use windows::Win32::System::Shutdown::{
        InitiateShutdownW, SHTDN_REASON_FLAG_PLANNED, SHTDN_REASON_MAJOR_OTHER,
        SHTDN_REASON_MINOR_OTHER, SHUTDOWN_FLAGS,
    };
    let _privilege = shutdown_privilege()?;
    let what = if restart { "restart" } else { "shut down" };
    tracing::warn!("asking Windows to {what} the PC");
    let reason = SHTDN_REASON_MAJOR_OTHER | SHTDN_REASON_MINOR_OTHER | SHTDN_REASON_FLAG_PLANNED;
    // SAFETY: no machine name (this PC) and no message; plain values otherwise.
    let error = unsafe {
        InitiateShutdownW(
            None,
            None,
            0,
            SHUTDOWN_FLAGS(shutdown_flags(restart)),
            reason,
        )
    };
    if error != 0 {
        bail!(
            "Windows refused to {what} the PC: {}",
            windows::core::Error::from_hresult(HRESULT::from_win32(error))
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_capabilities_become_sleep_states() {
        let caps = |s3, aoac, s4, file| SYSTEM_POWER_CAPABILITIES {
            SystemS3: s3,
            AoAc: aoac,
            SystemS4: s4,
            HiberFilePresent: file,
            ..Default::default()
        };
        let desktop = states(&caps(true, false, true, true));
        assert_eq!(desktop.describe(), "S3 available, hibernate on");
        assert_eq!(desktop.unavailable(), None);
        // The 780M laptop: Modern Standby only, hibernate on.
        let laptop = states(&caps(false, true, true, true));
        assert_eq!(laptop.describe(), "Modern Standby available, hibernate on");
        assert!(laptop.unavailable().is_some());
        // Hibernate needs its file.
        let standby = states(&caps(false, true, true, false));
        assert_eq!(
            standby.describe(),
            "Modern Standby available, hibernate off"
        );
        assert!(standby.unavailable().is_some());
        let all = SYSTEM_POWER_CAPABILITIES {
            SystemS1: true,
            SystemS2: true,
            ..caps(true, true, true, true)
        };
        assert_eq!(
            states(&all),
            SleepStates {
                s1: true,
                s2: true,
                s3: true,
                modern_standby: true,
                hibernate: true,
            }
        );
    }

    #[test]
    fn power_off_is_never_forced_or_hybrid() {
        use windows::Win32::System::Shutdown::{
            SHUTDOWN_FORCE_OTHERS, SHUTDOWN_FORCE_SELF, SHUTDOWN_HYBRID,
        };
        // SHUTDOWN_RESTART and SHUTDOWN_POWEROFF.
        assert_eq!((shutdown_flags(true), shutdown_flags(false)), (0x4, 0x8));
        for restart in [true, false] {
            let forbidden = SHUTDOWN_FORCE_OTHERS.0 | SHUTDOWN_FORCE_SELF.0 | SHUTDOWN_HYBRID.0;
            assert_eq!(shutdown_flags(restart) & forbidden, 0);
        }
        // From the user's session: no timeout, which would imply /f.
        assert_eq!(shutdown_args(true), ["/r", "/t", "0"]);
        assert_eq!(shutdown_args(false), ["/s", "/t", "0"]);
        for restart in [true, false] {
            let args = shutdown_args(restart).map(str::to_lowercase);
            assert!(
                !args
                    .iter()
                    .any(|a| a == "/f" || a == "/hybrid" || a == "/p")
            );
        }
    }

    #[test]
    fn a_refusal_says_why_whatever_the_code_page() {
        // German output in code page 850: not UTF-8.
        let oem: &[u8] = b"Zugriff verweigert \x84\x81\x94\r\n";
        assert!(std::str::from_utf8(std::hint::black_box(oem)).is_err());
        let said = refusal("restart", 1191, oem);
        let reason = said
            .strip_prefix("Windows refused to restart the PC: ")
            .and_then(|s| s.strip_suffix(" (exit code 1191)"))
            .unwrap_or_else(|| panic!("{said}"));
        let text = windows::core::Error::from_hresult(HRESULT::from_win32(1191)).message();
        assert!(!text.trim().is_empty());
        assert_eq!(
            reason,
            text.split_whitespace().collect::<Vec<_>>().join(" ")
        );
        // A code without a text falls back to the output, decoded lossily.
        let said = refusal("shut down", 0xFFF0, oem);
        assert!(
            said.starts_with("Windows refused to shut down the PC: Zugriff verweigert "),
            "{said}"
        );
        assert!(said.ends_with(" (exit code 65520)"), "{said}");
        assert_eq!(
            refusal("shut down", 0xFFF0, b""),
            "Windows refused to shut down the PC (exit code 65520)"
        );
    }

    #[test]
    fn a_timeout_is_not_a_refusal() {
        for what in ["restart", "shut down"] {
            let said = unanswered(what);
            assert!(said.ends_with(&format!("may still {what}")), "{said}");
            assert!(!said.contains("refused"), "{said}");
        }
    }
}
