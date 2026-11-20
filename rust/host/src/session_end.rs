//! Shutting down for the end of the Windows session.
//!
//! When Windows shuts down it ends the console session's processes, the host among them,
//! before the service hears of the shutdown. Without this the host died mid-stream: clients
//! waited out their connection timeout and saw forwarded USB devices fail instead of detach.
//! The session-end window (butterpollo_windows::session_end) asks the host's normal shutdown
//! to run and holds Windows' reply until that shutdown has told the clients, for at most
//! [`WAIT`]. The service's stop takes the same path, and whichever comes first runs it, once.
use std::{
    sync::{
        Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::Notify;

/// How long the session-end window waits for the clients to be told before it lets Windows
/// go on. Windows lists a program as preventing the shutdown after about five seconds; telling
/// the clients takes at most one (`clients_told_of_shutdown`).
pub(crate) const WAIT: Duration = Duration::from_secs(3);

#[derive(Default)]
pub(crate) struct SessionEnd {
    requested: AtomicBool,
    wake: Notify,
    told: Mutex<bool>,
    told_changed: Condvar,
}
impl SessionEnd {
    /// Windows ends the session: asks for the shutdown, the first time only, and waits at most
    /// `wait` until the shutdown, whatever started it, has told the clients. True when it had.
    pub(crate) fn ended(&self, wait: Duration) -> bool {
        self.request();
        self.wait_told(wait)
    }
    /// True for the first request; later ones change nothing.
    fn request(&self) -> bool {
        if self.requested.swap(true, Ordering::AcqRel) {
            return false;
        }
        // A stored permit: a shutdown asked for before the host waits for it is not lost.
        self.wake.notify_one();
        true
    }
    /// Returns once the end of the session asked for the shutdown.
    pub(crate) async fn requested(&self) {
        while !self.requested.load(Ordering::Acquire) {
            self.wake.notified().await;
        }
    }
    /// The shutdown has told the clients and detached forwarded USB devices.
    pub(crate) fn clients_told(&self) {
        *self.told.lock().unwrap() = true;
        self.told_changed.notify_all();
    }
    fn wait_told(&self, wait: Duration) -> bool {
        let told = self.told.lock().unwrap();
        let (told, _) = self
            .told_changed
            .wait_timeout_while(told, wait, |told| !*told)
            .unwrap();
        *told
    }
}

/// The exit code a host leaves with instead of returning, if any: a supervised host that shut
/// down for the end of its session leaves as Windows would have ended it, so the service starts
/// the next session's host as before. Returning, its exit code 0 would stop the service.
pub(crate) fn exit_code(supervised: bool, session_ended: bool) -> Option<u32> {
    (supervised && session_ended).then_some(butterpollo_windows::service::SESSION_ENDED_EXIT_CODE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::Arc, time::Instant};

    #[tokio::test]
    async fn the_end_of_the_session_asks_for_the_shutdown_once() {
        let end = Arc::new(SessionEnd::default());
        assert!(end.request());
        assert!(!end.request());
        // The permit stored by the first request wakes the host's wait at once.
        tokio::time::timeout(Duration::from_secs(1), end.requested())
            .await
            .expect("the shutdown was asked for");
    }

    #[tokio::test]
    async fn the_host_waits_until_the_session_ends() {
        let end = Arc::new(SessionEnd::default());
        let waiting = {
            let end = end.clone();
            tokio::spawn(async move { end.requested().await })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!waiting.is_finished());
        let ending = end.clone();
        std::thread::spawn(move || ending.ended(Duration::from_millis(10)));
        tokio::time::timeout(Duration::from_secs(1), waiting)
            .await
            .expect("woken")
            .unwrap();
    }

    #[test]
    fn the_window_waits_for_the_clients_to_be_told() {
        let end = Arc::new(SessionEnd::default());
        let shutdown = {
            let end = end.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(50));
                end.clients_told();
            })
        };
        let started = Instant::now();
        assert!(end.ended(Duration::from_secs(5)));
        assert!(started.elapsed() < Duration::from_secs(4));
        shutdown.join().unwrap();
    }

    #[test]
    fn a_shutdown_already_past_its_clients_lets_the_window_go_at_once() {
        // The service's stop came first: its shutdown told the clients before the session ended.
        let end = SessionEnd::default();
        end.clients_told();
        let started = Instant::now();
        assert!(end.ended(Duration::from_secs(5)));
        assert!(started.elapsed() < Duration::from_secs(1));
        // A second end, or a second mark, changes nothing.
        end.clients_told();
        assert!(end.ended(Duration::from_secs(5)));
    }

    #[test]
    fn the_window_lets_windows_go_on_when_the_shutdown_takes_too_long() {
        let end = SessionEnd::default();
        let started = Instant::now();
        assert!(!end.ended(Duration::from_millis(50)));
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(WAIT < Duration::from_secs(5));
    }

    #[test]
    fn only_a_supervised_host_ended_with_its_session_leaves_as_windows_ends_it() {
        // The code the service retries after "Windows terminated the host" (0x40010004).
        assert_eq!(exit_code(true, true), Some(1_073_807_364));
        assert_eq!(exit_code(true, false), None);
        assert_eq!(exit_code(false, true), None);
        assert_eq!(exit_code(false, false), None);
    }
}
