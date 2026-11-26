//! User-session process creation with ownership of the complete Windows job.

#[allow(unused_imports)]
use anyhow::{Context, Result, bail};
#[allow(unused_imports)]
use std::{
    mem::size_of,
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::Path,
};
#[allow(unused_imports)]
use windows::{
    Win32::{
        Foundation::*,
        Security::*,
        System::{RemoteDesktop::*, Threading::*},
    },
    core::{BOOL, PCWSTR, PWSTR},
};

mod control;
mod inspect;
mod listing;
mod security;
mod session;
mod spawn;
pub mod suspend;
pub mod suspend_watch;
mod tokens;

use control::HostStop;
pub use control::{CommandCancellation, HostShutdown, StopSignal};
#[cfg(test)]
use inspect::snapshot_processes;
pub use inspect::{
    AudioProcess, Usage, command_arguments, command_target, creation_time, image_path, now,
    processes, shell_command, stop_processes, usage,
};
pub use security::*;
pub use session::*;
pub use spawn::{PipedChild, user_environment};
#[allow(unused_imports)]
use tokens::token;
#[cfg(test)]
pub(crate) use tokens::token_elevated;
pub use tokens::{
    NoShell, Target, elevated, is_system, own_sid, standard_user_token, user_account,
    user_runs_elevated, user_sid,
};

pub struct Process {
    handle: OwnedHandle,
    job: OwnedHandle,
    host_stop: Option<HostStop>,
    pub pid: u32,
}

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

#[cfg(test)]
mod tests;
