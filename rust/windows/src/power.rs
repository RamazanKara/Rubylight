//! Sleep PC: whether Windows can sleep, and the suspend itself, as
//! Vibepollo does it (`src/power_action_native.cpp`). It asks Windows for
//! sleep with wake events enabled, and only on a PC with S1, S2 or S3: a
//! Modern Standby laptop with hibernate on hibernated instead.
use anyhow::{Result, bail};
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
}
