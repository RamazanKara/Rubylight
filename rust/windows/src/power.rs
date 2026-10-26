//! Sleep PC: whether Windows can sleep, and the suspend itself, as
//! Vibepollo does it (`src/power_action_native.cpp`). Sleep, never
//! hibernate; wake events stay enabled so Wake-on-LAN still works.
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

/// Whether the PC has a sleep state: S1 to S3, or Modern Standby.
pub fn sleep_supported() -> bool {
    let mut caps = SYSTEM_POWER_CAPABILITIES::default();
    // SAFETY: caps is a writable, correctly sized structure for this call.
    let ok = unsafe { GetPwrCapabilities(&mut caps) };
    ok && (caps.SystemS1 || caps.SystemS2 || caps.SystemS3 || caps.AoAc)
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

/// Put the PC to sleep. Windows may still refuse; the call returns after
/// the PC wakes.
pub fn suspend() -> Result<()> {
    if !sleep_supported() {
        bail!("this PC has no sleep state");
    }
    let _privilege = shutdown_privilege()?;
    // SAFETY: plain call; sleep, not hibernate, not forced, wake events kept.
    if !unsafe { SetSuspendState(false, false, false) } {
        bail!(
            "Windows refused to sleep: {}",
            windows::core::Error::from_thread()
        );
    }
    Ok(())
}
