//! A deadline for a blocking call that has none (brief G3). The SDK's CPU-buffer decode waits on its
//! Vulkan fence without a timeout (`pyrowave_decoder_decode_cpu_buffer_complete`), so on Linux the
//! wait runs on a thread of its own and the decoder thread gives up after the deadline, with the
//! same loud error as the Windows fence wait (`GPU_DECODE_TIMEOUT`).
//!
//! A call that missed its deadline may still be running, and may finish later: whatever it touches
//! must stay alive for good (the caller leaks it), and the watchdog refuses further work, because
//! its thread is stuck behind the stalled call.

use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread::JoinHandle;
use std::time::Duration;

type Job = Box<dyn FnOnce() -> Result<(), String> + Send>;

pub struct Watchdog {
    jobs: Option<mpsc::Sender<Job>>,
    results: mpsc::Receiver<Result<(), String>>,
    thread: Option<JoinHandle<()>>,
    stalled: bool,
}

impl Watchdog {
    pub fn new() -> Result<Self, String> {
        let (jobs, queue) = mpsc::channel::<Job>();
        let (done, results) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("pyrowave-wait".into())
            .spawn(move || {
                while let Ok(job) = queue.recv() {
                    if done.send(job()).is_err() { break; }
                }
            })
            .map_err(|e| format!("cannot start the PyroWave wait thread: {e}"))?;
        Ok(Self { jobs: Some(jobs), results, thread: Some(thread), stalled: false })
    }

    /// Runs `job` on the wait thread and returns its result, or an error if it is not done within
    /// `deadline` (after which the watchdog is stalled for good and `job` may still be running).
    pub fn run(&mut self, deadline: Duration, job: impl FnOnce() -> Result<(), String> + Send + 'static) -> Result<(), String> {
        if self.stalled {
            return Err("an earlier PyroWave decode never finished, so the Vulkan device may be hung or lost".into());
        }
        let sent = self.jobs.as_ref().is_some_and(|jobs| jobs.send(Box::new(job)).is_ok());
        if !sent {
            self.stalled = true;
            return Err("the PyroWave wait thread is gone".into());
        }
        match self.results.recv_timeout(deadline) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => {
                self.stalled = true;
                Err(format!("the PyroWave decode did not finish within {deadline:?}: the Vulkan device may be hung or lost"))
            }
            Err(RecvTimeoutError::Disconnected) => {
                self.stalled = true;
                Err("the PyroWave wait thread ended while decoding".into())
            }
        }
    }

    /// Whether a call missed its deadline: what it used must not be freed or reused.
    pub fn stalled(&self) -> bool {
        self.stalled
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        self.jobs = None;
        // A stalled thread is stuck in the call: leave it behind rather than block on it.
        if let Some(thread) = self.thread.take() {
            if !self.stalled { let _ = thread.join(); }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Instant;

    #[test]
    fn a_call_inside_the_deadline_returns_its_result() {
        let mut watchdog = Watchdog::new().unwrap();
        assert_eq!(watchdog.run(Duration::from_secs(5), || Ok(())), Ok(()));
        assert_eq!(watchdog.run(Duration::from_secs(5), || Err("device lost".into())), Err("device lost".to_owned()));
        assert!(!watchdog.stalled(), "a failed call is not a stalled one");
        assert_eq!(watchdog.run(Duration::from_secs(5), || Ok(())), Ok(()), "and the watchdog still works");
    }

    #[test]
    fn a_call_that_outlasts_the_deadline_is_a_loud_error_and_the_watchdog_refuses_more_work() {
        let mut watchdog = Watchdog::new().unwrap();
        let (release, held) = mpsc::channel::<()>();
        let finished = Arc::new(AtomicBool::new(false));
        let flag = finished.clone();
        let started = Instant::now();
        let error = watchdog.run(Duration::from_millis(100), move || {
            let _ = held.recv();
            flag.store(true, Ordering::SeqCst);
            Ok(())
        }).unwrap_err();
        let waited = started.elapsed();
        assert!(error.contains("did not finish within 100ms") && error.contains("hung or lost"), "{error}");
        assert!(waited >= Duration::from_millis(100) && waited < Duration::from_secs(5), "gave up after {waited:?}");
        assert!(watchdog.stalled());
        assert!(!finished.load(Ordering::SeqCst), "the call is still running");

        let ran = Arc::new(AtomicBool::new(false));
        let flag = ran.clone();
        let again = watchdog.run(Duration::from_secs(5), move || { flag.store(true, Ordering::SeqCst); Ok(()) }).unwrap_err();
        assert!(again.contains("never finished"), "{again}");
        assert!(!ran.load(Ordering::SeqCst), "no work is queued behind a stalled call");

        // Dropping a stalled watchdog does not wait for the call; the call ends once released.
        let dropped = Instant::now();
        drop(watchdog);
        assert!(dropped.elapsed() < Duration::from_secs(1));
        drop(release);
    }

    #[test]
    fn a_panicking_call_is_an_error_and_not_a_hang() {
        let mut watchdog = Watchdog::new().unwrap();
        let error = watchdog.run(Duration::from_secs(5), || panic!("boom")).unwrap_err();
        assert!(error.contains("wait thread ended"), "{error}");
        assert!(watchdog.stalled());
    }
}
