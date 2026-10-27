//! The lock child-session setup, restore and the viewer's start take across
//! processes: every Rubylight host (the service, one started by hand) and the
//! uninstaller's `butterpollo.exe --child-session-restore` change the same
//! Windows settings, one at a time. A `Global\` named mutex, so it holds
//! across sessions too, that SYSTEM, elevated administrators and its creator
//! open.

use crate::text::to_wide;
use anyhow::{Context, Result, bail};
use std::{marker::PhantomData, time::Duration};
use windows::{
    Win32::{
        Foundation::{CloseHandle, HANDLE, WAIT_ABANDONED, WAIT_OBJECT_0, WAIT_TIMEOUT},
        Security::{
            Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW,
            PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
        },
        System::Threading::{
            CreateMutexExW, INFINITE, MUTEX_MODIFY_STATE, ReleaseMutex,
            SYNCHRONIZATION_SYNCHRONIZE, WaitForSingleObject,
        },
    },
    core::{BOOL, PCWSTR, w},
};

/// The mutex [`setup_lock`] takes.
pub const SETUP_LOCK: &str = r"Global\Rubylight.ChildSessionSetup";
/// How long the host waits for [`setup_lock`]: longer than setup or restore
/// take, which run the firewall rule script (60 s at most) once each.
pub const SETUP_LOCK_TIMEOUT: Duration = Duration::from_secs(120);

/// A named mutex this thread holds until the lock is dropped. Windows ties a
/// mutex to the thread that took it, so the lock never leaves that thread.
pub struct NamedLock {
    handle: HANDLE,
    abandoned: bool,
    _thread: PhantomData<*const ()>,
}
impl NamedLock {
    /// Take the mutex `name`, waiting up to `timeout`; None when another
    /// holder keeps it longer. A mutex its last holder never released (the process
    /// ended holding it) is taken, and said in the log: what that holder was
    /// changing may be half done.
    pub fn acquire(name: &str, timeout: Duration) -> Result<Option<Self>> {
        let wide = to_wide(name);
        let mut descriptor = crate::ipc::Descriptor(PSECURITY_DESCRIPTOR::default());
        // SAFETY: the strings are NUL-terminated and outlive the calls, `security` points at the
        // live descriptor, and the handle is closed once: here on failure, else by the lock.
        unsafe {
            // SYSTEM, administrators and the process that created it (OW: the owner, which for
            // SYSTEM and elevated administrators is one of those two). Applies when this call
            // creates the mutex; another user's program cannot open one it did not create.
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                w!("D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;OW)"),
                1,
                &mut descriptor.0,
                None,
            )
            .context("building the child-session lock's permissions")?;
            let security = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: descriptor.0.0,
                bInheritHandle: BOOL(0),
            };
            let handle = CreateMutexExW(
                Some(&security),
                PCWSTR(wide.as_ptr()),
                0,
                (SYNCHRONIZATION_SYNCHRONIZE | MUTEX_MODIFY_STATE).0,
            )
            .with_context(|| {
                format!("opening the lock {name}; Rubylight needs administrator rights here")
            })?;
            let millis = u32::try_from(timeout.as_millis())
                .unwrap_or(INFINITE - 1)
                .min(INFINITE - 1);
            let abandoned = match WaitForSingleObject(handle, millis) {
                WAIT_OBJECT_0 => false,
                WAIT_ABANDONED => true,
                result => {
                    let error = windows::core::Error::from_thread();
                    let _ = CloseHandle(handle);
                    if result == WAIT_TIMEOUT {
                        return Ok(None);
                    }
                    bail!("waiting for the lock {name} failed: {error}");
                }
            };
            if abandoned {
                tracing::warn!(
                    lock = name,
                    "the last holder of the lock ended without letting go of it; taking it, and what it was changing may be half done"
                );
            }
            Ok(Some(Self {
                handle,
                abandoned,
                _thread: PhantomData,
            }))
        }
    }
    /// Whether the last holder ended without letting go of it.
    pub fn abandoned(&self) -> bool {
        self.abandoned
    }
}
impl Drop for NamedLock {
    fn drop(&mut self) {
        // SAFETY: this thread took the mutex (the lock is not Send) and owns the handle, released
        // and closed once here.
        unsafe {
            let _ = ReleaseMutex(self.handle);
            let _ = CloseHandle(self.handle);
        }
    }
}

/// [`SETUP_LOCK`], for up to `timeout` ([`SETUP_LOCK_TIMEOUT`] outside
/// tests): setup, restore (the console's and the uninstaller's), the firewall
/// checks and the viewer's start in any Rubylight process wait for each other.
/// Err when the wait runs out, saying so.
pub fn setup_lock(timeout: Duration) -> Result<NamedLock> {
    NamedLock::acquire(SETUP_LOCK, timeout)?.with_context(|| {
        format!(
            "another Rubylight process (a host or the uninstaller) was still setting up or restoring child sessions after {timeout:?}; try again once it finishes"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    /// A `Global\` mutex of this test's own.
    fn name() -> String {
        format!(
            r"Global\Rubylight.Test.{}.{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )
    }

    #[test]
    fn two_holders_exclude_each_other() {
        let name = name();
        let held = NamedLock::acquire(&name, Duration::ZERO).unwrap().unwrap();
        assert!(!held.abandoned());
        // Another thread, as another process would: not taken within the timeout.
        let other = name.clone();
        let started = std::time::Instant::now();
        let refused = std::thread::spawn(move || {
            NamedLock::acquire(&other, Duration::from_millis(100))
                .unwrap()
                .is_none()
        })
        .join()
        .unwrap();
        assert!(refused && started.elapsed() >= Duration::from_millis(100));
        // Once it is released, a holder waiting for it gets it.
        let (waiting, wait) = mpsc::channel();
        let (taken, take) = mpsc::channel();
        let other = name.clone();
        let thread = std::thread::spawn(move || {
            waiting.send(()).unwrap();
            let lock = NamedLock::acquire(&other, Duration::from_secs(10))
                .unwrap()
                .unwrap();
            taken.send(lock.abandoned()).unwrap();
        });
        wait.recv().unwrap();
        assert!(
            take.recv_timeout(Duration::from_millis(200)).is_err(),
            "taken while held"
        );
        drop(held);
        assert!(!take.recv_timeout(Duration::from_secs(10)).unwrap());
        thread.join().unwrap();
    }

    #[test]
    fn an_abandoned_lock_is_taken() {
        let name = name();
        let other = name.clone();
        // The holder's thread ends holding it, as a process that crashed would. Its handle is
        // leaked, which keeps the mutex alive.
        std::thread::spawn(move || {
            std::mem::forget(NamedLock::acquire(&other, Duration::ZERO).unwrap().unwrap());
        })
        .join()
        .unwrap();
        let lock = NamedLock::acquire(&name, Duration::ZERO).unwrap().unwrap();
        assert!(lock.abandoned());
        drop(lock);
        // Released, it is an ordinary lock again.
        let lock = NamedLock::acquire(&name, Duration::ZERO).unwrap().unwrap();
        assert!(!lock.abandoned());
    }

    #[test]
    fn the_setup_lock_is_global_and_says_when_it_waited_too_long() {
        assert!(SETUP_LOCK.starts_with(r"Global\"));
        let held = setup_lock(SETUP_LOCK_TIMEOUT).unwrap();
        let error = std::thread::spawn(|| setup_lock(Duration::from_millis(100)).map(drop))
            .join()
            .unwrap()
            .unwrap_err();
        drop(held);
        assert!(
            format!("{error:#}")
                .contains("was still setting up or restoring child sessions after 100ms"),
            "{error:#}"
        );
    }
}
