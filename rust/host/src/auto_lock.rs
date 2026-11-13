//! Locks Windows when the last device leaves (`lock_on_last_disconnect`, off by
//! default; `butterpollo_core::auto_lock`). The lock runs as the signed-in user
//! in their session, the same way PC commands run, because a SYSTEM service
//! cannot lock another session's desktop.
use crate::state::{Host, Shared};
use butterpollo_core::auto_lock::{AutoLock, KEY};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

/// The command that locks the signed-in user's session.
const LOCK: &str = "rundll32.exe user32.dll,LockWorkStation";

/// Whether any device streams, is launching, is ending its stream, plays in a
/// browser or holds a controller session: the PC is in use from afar.
pub(crate) fn devices_connected(h: &Host) -> bool {
    let sessions = h.sessions.lock().unwrap();
    let streams = !sessions.active.is_empty()
        || !sessions.pending.is_empty()
        || !sessions.teardown.is_empty();
    drop(sessions);
    streams || h.webrtc_links.len() != 0 || h.controller_sessions.in_use()
}

/// The maintenance loop's watch, looked at on every pass.
#[derive(Default)]
pub(crate) struct Watch {
    state: AutoLock,
    locking: Arc<AtomicBool>,
}

impl Watch {
    pub(crate) fn poll(&mut self, h: &Shared) {
        let enabled = h.config.read().unwrap().boolean(KEY, false);
        let devices = enabled && devices_connected(h);
        if !self.state.tick(enabled, devices, Instant::now())
            || self.locking.swap(true, Ordering::AcqRel)
        {
            return;
        }
        let h = h.clone();
        let locking = self.locking.clone();
        tokio::task::spawn_blocking(move || {
            lock(&h);
            locking.store(false, Ordering::Release);
        });
    }
    /// When the loop must look again, so an idle loop wakes for the lock.
    pub(crate) fn due(&self) -> Option<Instant> {
        self.state.due()
    }
}

/// Why the lock is skipped, None to lock. A device that arrived since the decision wins (never
/// during a stream). Only the service host (SYSTEM) needs a signed-in user to lock for: its user
/// token query is what fails without one; a host in the user's own session always has its user,
/// and that query fails there anyway (1314, no SYSTEM privilege).
fn skip_reason(
    devices: bool,
    system: bool,
    signed_in: impl FnOnce() -> bool,
) -> Option<&'static str> {
    if devices {
        Some("a device connected again; Windows stays unlocked")
    } else if system && !signed_in() {
        Some("nobody is signed in to Windows; there is nothing to lock")
    } else {
        None
    }
}

fn lock(h: &Host) {
    use butterpollo_windows::process;
    if let Some(why) = skip_reason(
        devices_connected(h),
        process::is_system(),
        process::user_signed_in,
    ) {
        tracing::info!("auto lock: skipped: {why}");
        return;
    }
    match butterpollo_windows::process::Process::shell_detached(LOCK, None, false, &BTreeMap::new())
    {
        Ok(()) => tracing::info!("auto lock: the last device left; Windows is locked"),
        Err(error) => tracing::warn!(%error, "auto lock: Windows could not be locked"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_support::Fixture;

    #[test]
    fn auto_lock_counts_streams_and_launches_as_connected_devices() {
        let f = Fixture::new();
        assert!(!devices_connected(&f.host));
        let client = f.client(u32::MAX);
        let _session = f.session(client, butterpollo_core::session::Role::Stream);
        assert!(devices_connected(&f.host));
        f.host.sessions.lock().unwrap().active.clear();
        assert!(!devices_connected(&f.host));
    }

    #[test]
    fn auto_lock_skips_only_for_a_device_or_a_service_host_without_a_user() {
        assert!(skip_reason(true, false, || true).is_some());
        assert!(
            skip_reason(false, true, || false)
                .unwrap()
                .contains("nobody is signed in")
        );
        assert_eq!(skip_reason(false, true, || true), None);
        // A host in the user's session locks: its user token query fails there (1314) and is not asked.
        assert_eq!(
            skip_reason(false, false, || panic!("not asked outside the service")),
            None
        );
    }

    #[test]
    fn auto_lock_off_by_default_never_spawns_a_lock() {
        let f = Fixture::new();
        let mut watch = Watch::default();
        watch.poll(&f.host);
        assert_eq!(watch.due(), None);
        assert!(!watch.locking.load(Ordering::Acquire));
    }
}
