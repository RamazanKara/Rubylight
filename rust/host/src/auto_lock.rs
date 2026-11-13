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

fn lock(h: &Host) {
    // Never during a stream: a device that arrived since the decision wins.
    if devices_connected(h) {
        tracing::info!("auto lock: a device connected again; Windows stays unlocked");
        return;
    }
    if !butterpollo_windows::process::user_signed_in() {
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
    fn auto_lock_off_by_default_never_spawns_a_lock() {
        let f = Fixture::new();
        let mut watch = Watch::default();
        watch.poll(&f.host);
        assert_eq!(watch.due(), None);
        assert!(!watch.locking.load(Ordering::Acquire));
    }
}
