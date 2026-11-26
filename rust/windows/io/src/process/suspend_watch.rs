//! The Quick Suspend watchdog and the recovery of a journal a host left behind.
//!
//! While a game is suspended the host keeps a helper process beside it
//! (`butterpollo.exe --quick-suspend-watch HOST_PID --config-dir PROFILE`),
//! started outside the host's job where Windows allows it, so that a service
//! stop that ends the host's job leaves it running. The helper waits for the
//! host to exit; if the host goes while the journal it wrote still names
//! processes, the helper resumes them. When the host resumes the game itself,
//! it tells the helper (a named event) and the helper exits.
//!
//! A host that starts recovers any journal a host that no longer runs left
//! ([`recover_at_start`]): after a reboot nothing it names still runs, and the
//! journal is simply cleared. The journal's reads and writes are serialised by
//! a named mutex, so the helper of a crashed host and the next host never act
//! on it at once.
use super::{inspect::created_at, owned, raw, suspend};
use anyhow::{Context, Result, bail};
use butterpollo_session::quick_suspend::{self, Journal, Recovery, journal};
use std::{
    os::windows::io::OwnedHandle,
    path::Path,
    process::{Child, Command},
    sync::Mutex,
};
use windows::Win32::{Foundation::*, System::Threading::*};

/// This host's helper and the event that tells it nothing is suspended.
static WATCH: Mutex<Option<(Child, OwnedHandle)>> = Mutex::new(None);

/// The event a host sets once nothing it suspended is left.
fn done_event(host_pid: u32) -> Result<OwnedHandle> {
    let name: Vec<u16> = format!("Local\\Butterpollo.QuickSuspendWatch.{host_pid}\0")
        .encode_utf16()
        .collect();
    // SAFETY: the name is NUL-terminated and outlives the call; the handle is owned once.
    let event = unsafe { CreateEventW(None, true, false, windows::core::PCWSTR(name.as_ptr()))? };
    Ok(owned(event))
}

/// The journal's lock: held while it is read and changed.
pub struct JournalLock(OwnedHandle);
impl Drop for JournalLock {
    fn drop(&mut self) {
        // SAFETY: `lock` acquired this mutex on this thread; the handle closes with `self.0`.
        unsafe {
            let _ = ReleaseMutex(raw(&self.0));
        }
    }
}

/// Take the lock of the journal in `profile`.
pub fn lock(profile: &Path) -> Result<JournalLock> {
    // FNV-1a over the lowercased path: one lock per profile.
    let hash = profile
        .to_string_lossy()
        .to_lowercase()
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
        });
    let name: Vec<u16> = format!("Local\\Butterpollo.QuickSuspendJournal.{hash:x}\0")
        .encode_utf16()
        .collect();
    // SAFETY: the name is NUL-terminated and outlives the call; the handle is owned at once
    // and released by the guard only after this thread acquired it.
    unsafe {
        let mutex = owned(CreateMutexW(
            None,
            false,
            windows::core::PCWSTR(name.as_ptr()),
        )?);
        let waited = WaitForSingleObject(raw(&mutex), 30_000);
        if waited != WAIT_OBJECT_0 && waited != WAIT_ABANDONED {
            bail!("the Quick Suspend journal is busy");
        }
        Ok(JournalLock(mutex))
    }
}

/// This process's identity: id and creation time.
pub fn me() -> (u32, u64) {
    let pid = std::process::id();
    (pid, super::creation_time(pid))
}

/// Start this host's helper for `profile` unless it runs. A suspend that
/// cannot start it does not suspend.
pub fn watch(profile: &Path) -> Result<()> {
    use std::os::windows::process::CommandExt;
    let mut slot = WATCH
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((child, _)) = slot.as_mut()
        && child.try_wait()?.is_none()
    {
        return Ok(());
    }
    let done = done_event(std::process::id())?;
    // SAFETY: the event is owned and open; a helper that ran before has exited.
    unsafe { ResetEvent(raw(&done))? };
    let spawn = |flags: u32| -> Result<Child> {
        Ok(Command::new(std::env::current_exe()?)
            .arg("--quick-suspend-watch")
            .arg(std::process::id().to_string())
            .arg("--config-dir")
            .arg(profile)
            .creation_flags(flags)
            .spawn()?)
    };
    // Out of the host's job, so that a service stop does not end it with the
    // host. A job that forbids that (a terminal, a launcher) refuses with
    // access denied; the helper then stays in it and still covers a crash.
    let child = spawn(CREATE_BREAKAWAY_FROM_JOB.0 | CREATE_NO_WINDOW.0)
        .or_else(|error| {
            tracing::debug!(%error, "the Quick Suspend watchdog cannot leave the host's job");
            spawn(CREATE_NO_WINDOW.0)
        })
        .context("starting the Quick Suspend watchdog")?;
    *slot = Some((child, done));
    Ok(())
}

/// Tell this host's helper that nothing is suspended any more, and let it go.
pub fn release() {
    let mut slot = WATCH
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some((mut child, done)) = slot.take() else {
        return;
    };
    // SAFETY: the event is owned and open.
    let _ = unsafe { SetEvent(raw(&done)) };
    // Hold the event until the helper saw it, so a new one cannot reset it first.
    for _ in 0..20 {
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => return,
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(25)),
        }
    }
    tracing::warn!("the Quick Suspend watchdog did not exit within 500 ms; stopping it");
    let _ = child.kill();
    let _ = child.wait();
}

/// The helper's work: wait for the host `host_pid` to exit or to say nothing
/// is suspended, and in the first case resume what its journal names.
pub fn wait_and_recover(host_pid: u32, profile: &Path) -> Result<Option<Recovery>> {
    let done = done_event(host_pid)?;
    let Some(owner) = journal::load(profile)? else {
        return Ok(None);
    };
    if owner.host_pid != host_pid {
        return Ok(None);
    }
    // SAFETY: OpenProcess has no preconditions; the handle is owned at once.
    let host = unsafe {
        OpenProcess(
            PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
            false,
            host_pid,
        )
    }
    .ok()
    .map(owned);
    if let Some(host) = &host
        && created_at(raw(host)) == owner.host_started
    {
        // SAFETY: both handles are owned and open for the wait.
        let waited = unsafe { WaitForMultipleObjects(&[raw(host), raw(&done)], false, INFINITE) };
        if waited == WAIT_EVENT(WAIT_OBJECT_0.0 + 1) {
            return Ok(None);
        }
        if waited != WAIT_OBJECT_0 {
            bail!("waiting for the host failed");
        }
    }
    let host = (owner.host_pid, owner.host_started);
    recover_owned(profile, |journal| {
        (journal.host_pid, journal.host_started) == host
    })
    .map(|found| found.map(|(_, recovery)| recovery))
}

/// At a host's start: resume what a journal of a host that no longer runs
/// names, and clear it. An unreadable journal is set aside (`.bad`) and
/// reported, never guessed at.
pub fn recover_at_start(profile: &Path) -> Result<Option<(Journal, Recovery)>> {
    let me = me();
    {
        let _lock = lock(profile)?;
        if let Err(error) = journal::load(profile) {
            let aside = journal::set_aside(profile)?;
            bail!("{error:#}; it was set aside as {}", aside.display());
        }
    }
    recover_owned(profile, |journal| {
        quick_suspend::stale(journal, me, suspend::alive)
    })
}

/// Recover the journal in `profile` when `owns` says it is this caller's to
/// recover: what resumed or exited leaves it, a process that failed to resume
/// stays for the next try.
fn recover_owned(
    profile: &Path,
    owns: impl Fn(&Journal) -> bool,
) -> Result<Option<(Journal, Recovery)>> {
    let _lock = lock(profile)?;
    let Some(journal) = journal::load(profile)? else {
        return Ok(None);
    };
    if !owns(&journal) {
        return Ok(None);
    }
    let recovery = quick_suspend::recover(&journal, &mut suspend::Thaw);
    if recovery.complete() {
        journal::remove(profile)?;
    } else {
        let mut left = journal.clone();
        left.processes = recovery
            .failed
            .iter()
            .map(|(record, _)| record.clone())
            .collect();
        journal::save(profile, &left)?;
    }
    Ok(Some((journal, recovery)))
}

#[cfg(test)]
mod tests {
    use super::super::suspend::tests::{sleeper, suspend_count};
    use super::*;
    use butterpollo_session::quick_suspend::Record;

    fn journal_for(host: (u32, u64), pid: u32, started: u64) -> Journal {
        let mut journal = Journal::new(host, "Test Game", 1);
        journal.processes.push(Record {
            pid,
            started,
            name: "PING.EXE".into(),
        });
        journal
    }

    #[test]
    fn a_journal_a_gone_host_left_is_resumed_and_cleared_at_start() {
        let profile = tempfile::tempdir().unwrap();
        let game = sleeper();
        let pid = game.0.id();
        let started = crate::process::creation_time(pid);
        // A host that crashed: its id and creation time no longer run.
        journal::save(
            profile.path(),
            &journal_for((u32::MAX - 3, 1), pid, started),
        )
        .unwrap();
        suspend::suspend(pid, started).unwrap();
        assert_eq!(suspend_count(pid), 1);
        let (journal, recovery) = recover_at_start(profile.path()).unwrap().unwrap();
        assert_eq!(journal.app_title, "Test Game");
        assert_eq!(recovery.resumed, ["PING.EXE"]);
        assert_eq!(suspend_count(pid), 0);
        assert_eq!(journal::load(profile.path()).unwrap(), None);
        // Nothing left: a second start finds nothing.
        assert_eq!(recover_at_start(profile.path()).unwrap(), None);
    }

    #[test]
    fn a_running_hosts_journal_is_left_alone_and_a_bad_one_set_aside() {
        let profile = tempfile::tempdir().unwrap();
        let other_host = sleeper();
        let host = (
            other_host.0.id(),
            crate::process::creation_time(other_host.0.id()),
        );
        journal::save(profile.path(), &journal_for(host, u32::MAX - 7, 1)).unwrap();
        assert_eq!(recover_at_start(profile.path()).unwrap(), None);
        assert!(journal::load(profile.path()).unwrap().is_some());
        // Its watchdog, once that host is gone, resumes and clears it.
        drop(other_host);
        let recovery = wait_and_recover(host.0, profile.path()).unwrap().unwrap();
        assert_eq!(recovery.gone, 1);
        assert_eq!(journal::load(profile.path()).unwrap(), None);
        // An unreadable journal is kept aside for a look, never guessed at.
        std::fs::write(journal::path(profile.path()), b"{").unwrap();
        assert!(recover_at_start(profile.path()).is_err());
        assert!(profile.path().join("quick-suspend.json.bad").exists());
        assert_eq!(journal::load(profile.path()).unwrap(), None);
    }

    #[test]
    fn a_watchdog_ignores_another_hosts_journal() {
        let profile = tempfile::tempdir().unwrap();
        journal::save(profile.path(), &journal_for((7, 1), u32::MAX - 7, 1)).unwrap();
        assert_eq!(wait_and_recover(8, profile.path()).unwrap(), None);
        assert!(journal::load(profile.path()).unwrap().is_some());
    }
}
