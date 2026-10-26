//! Sleep PC: whether Windows can sleep, and the suspend itself, as
//! Vibepollo does it (`src/power_action_native.cpp`). It asks Windows for
//! sleep, not hibernate, with wake events enabled so Wake-on-LAN still
//! works. What a Modern Standby PC does with that request is not measured.
use anyhow::{Result, bail};
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

fn capabilities() -> Option<SYSTEM_POWER_CAPABILITIES> {
    let mut caps = SYSTEM_POWER_CAPABILITIES::default();
    // SAFETY: caps is a writable, correctly sized structure for this call.
    unsafe { GetPwrCapabilities(&mut caps) }.then_some(caps)
}
fn can_sleep(caps: &SYSTEM_POWER_CAPABILITIES) -> bool {
    caps.SystemS1 || caps.SystemS2 || caps.SystemS3 || caps.AoAc
}
/// The sleep states Windows reports, for the log: "S3, hibernate on".
fn describe(caps: &SYSTEM_POWER_CAPABILITIES) -> String {
    let states: Vec<_> = [
        (caps.SystemS1, "S1"),
        (caps.SystemS2, "S2"),
        (caps.SystemS3, "S3"),
        (caps.AoAc, "Modern Standby"),
    ]
    .into_iter()
    .filter_map(|(present, name)| present.then_some(name))
    .collect();
    let hibernate = caps.SystemS4 && caps.HiberFilePresent;
    format!(
        "{}, hibernate {}",
        if states.is_empty() {
            "no sleep state".to_owned()
        } else {
            states.join(", ")
        },
        if hibernate { "on" } else { "off" }
    )
}

/// Whether the PC has a sleep state: S1 to S3, or Modern Standby.
pub fn sleep_supported() -> bool {
    capabilities().is_some_and(|caps| can_sleep(&caps))
}

/// The calling thread's token with shutdown privilege enabled, restored
/// (and the impersonation ended) on drop. Only this thread's token changes,
/// not the whole host process's.
struct ShutdownPrivilege {
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
fn shutdown_privilege() -> Result<ShutdownPrivilege> {
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

/// Ask Windows to sleep, not hibernate. Windows may still refuse; the call
/// returns after the PC wakes.
pub fn suspend() -> Result<()> {
    let Some(caps) = capabilities() else {
        bail!(
            "could not read this PC's power capabilities: {}",
            windows::core::Error::from_thread()
        );
    };
    if !can_sleep(&caps) {
        bail!("this PC has no sleep state ({})", describe(&caps));
    }
    let _privilege = shutdown_privilege()?;
    tracing::warn!(states = %describe(&caps), "Sleep PC asks Windows for sleep, not hibernate");
    // SAFETY: plain call; sleep, not hibernate, not forced, wake events kept.
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
    fn the_log_names_the_sleep_states_and_hibernate() {
        let caps = |s3, aoac, s4, file| SYSTEM_POWER_CAPABILITIES {
            SystemS3: s3,
            AoAc: aoac,
            SystemS4: s4,
            HiberFilePresent: file,
            ..Default::default()
        };
        let desktop = caps(true, false, true, true);
        assert_eq!(describe(&desktop), "S3, hibernate on");
        assert!(can_sleep(&desktop));
        // Modern Standby only, still offered; hibernate needs its file.
        let standby = caps(false, true, true, false);
        assert_eq!(describe(&standby), "Modern Standby, hibernate off");
        assert!(can_sleep(&standby));
        let none = caps(false, false, false, false);
        assert_eq!(describe(&none), "no sleep state, hibernate off");
        assert!(!can_sleep(&none));
        let all = SYSTEM_POWER_CAPABILITIES {
            SystemS1: true,
            SystemS2: true,
            ..caps(true, true, true, true)
        };
        assert_eq!(describe(&all), "S1, S2, S3, Modern Standby, hibernate on");
    }
}
