//! Automatic bitrate on a Foundation Sunshine PC, which runs the controller itself
//! (`nightjar_core::http::foundation`): the stream turns it on, reports its network every three
//! seconds and shows the bitrate the PC chose, as Moonlight V+ PC does. Rubylight PCs keep this
//! client's own controller (`bitrate.rs`).

use std::sync::{Arc, Mutex};

use nightjar_core::http::foundation::{AbrAction, AbrConfigured, AbrFeedback, ABR_FEEDBACK_INTERVAL};

/// The hand-off between the thread that asks the PC to turn its controller on and the stream,
/// which may end while that request is in flight: whichever side comes second turns it off, so
/// the PC never keeps its controller after the stream.
#[derive(Clone, Default)]
pub struct AbrStart(Arc<Mutex<StartState>>);

#[derive(Default, PartialEq, Debug)]
enum StartState {
    #[default]
    Pending,
    On,
    Ended,
}

impl AbrStart {
    /// The thread: the PC turned its controller on. False when the stream ended meanwhile; the
    /// thread then turns it off itself.
    pub fn turned_on(&self) -> bool {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if *state != StartState::Pending { return false; }
        *state = StartState::On;
        true
    }

    /// The stream ends or stops the controller: true when it is on and the caller turns it off.
    /// A start still in flight turns it off itself (`turned_on`).
    pub fn end(&self) -> bool {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::replace(&mut *state, StartState::Ended) == StartState::On
    }
}

/// The request that turns the PC's controller off, on its own thread: with the network gone its
/// timeouts would hold the window for about two seconds at stream end. A start that follows
/// waits for it ([`AbrStop::wait`]), so the PC never ends up off after the user turned it on again.
pub struct AbrStop(std::thread::JoinHandle<()>);

impl AbrStop {
    pub fn spawn(off: impl FnOnce() + Send + 'static) -> Self {
        Self(std::thread::spawn(off))
    }

    pub fn wait(self) {
        if self.0.join().is_err() {
            eprintln!("nightjar: the request that stops the PC's automatic bitrate panicked");
        }
    }
}

pub struct HostAbr {
    /// The bitrate in Kbps the PC last set.
    pub current_kbps: u32,
    frames: u64,
    drops: u64,
    decoded: u64,
    window_start_ms: u64,
    in_flight: bool,
}

impl HostAbr {
    /// The PC's controller is on from `now_ms`, starting at its initial bitrate (else the stream's).
    pub fn new(configured: &AbrConfigured, stream_kbps: u32, now_ms: u64) -> Self {
        let current_kbps = if configured.initial_bitrate > 0 { configured.initial_bitrate } else { stream_kbps };
        Self { current_kbps, frames: 0, drops: 0, decoded: 0, window_start_ms: now_ms, in_flight: false }
    }

    /// One overlay tick: frames completed and dropped, and frames decoded.
    pub fn add(&mut self, completed: u64, drops: u64, decoded: u64) {
        self.frames += completed + drops;
        self.drops += drops;
        self.decoded += decoded;
    }

    /// The report for the window that ends at `now_ms`, once three seconds have passed and the
    /// last report has been answered. Starts the next window.
    pub fn due(&mut self, now_ms: u64, rtt_ms: Option<f64>) -> Option<AbrFeedback> {
        let elapsed = now_ms.saturating_sub(self.window_start_ms);
        if self.in_flight || elapsed < ABR_FEEDBACK_INTERVAL.as_millis() as u64 {
            return None;
        }
        let feedback = AbrFeedback {
            packet_loss: AbrFeedback::loss_percent(self.drops, self.frames),
            rtt_ms: rtt_ms.unwrap_or(0.0),
            decode_fps: self.decoded as f64 * 1000.0 / elapsed as f64,
            dropped_frames: self.drops.min(u64::from(u32::MAX)) as u32,
            current_bitrate: self.current_kbps,
        };
        *self = Self { window_start_ms: now_ms, in_flight: true, ..Self::fresh(self.current_kbps) };
        Some(feedback)
    }

    /// The PC's answer to the last report: the bitrate it moved to, if it moved.
    pub fn answered(&mut self, action: Option<&AbrAction>) -> Option<u32> {
        self.in_flight = false;
        let kbps = action?.new_bitrate.filter(|kbps| *kbps > 0 && *kbps != self.current_kbps)?;
        self.current_kbps = kbps;
        Some(kbps)
    }

    fn fresh(current_kbps: u32) -> Self {
        Self { current_kbps, frames: 0, drops: 0, decoded: 0, window_start_ms: 0, in_flight: false }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_go_every_three_seconds_one_at_a_time() {
        let configured = AbrConfigured { success: true, enabled: true, initial_bitrate: 35000, ..Default::default() };
        let mut abr = HostAbr::new(&configured, 50000, 1000);
        assert_eq!(abr.current_kbps, 35000, "the PC's initial bitrate");
        assert_eq!(HostAbr::new(&AbrConfigured::default(), 50000, 0).current_kbps, 50000);
        for _ in 0..3 {
            abr.add(118, 2, 120);
        }
        assert_eq!(abr.due(3999, Some(8.0)), None, "not before three seconds");
        let report = abr.due(4000, Some(8.0)).unwrap();
        assert_eq!(report.packet_loss, 6.0 * 100.0 / 360.0);
        assert_eq!((report.dropped_frames, report.current_bitrate, report.rtt_ms), (6, 35000, 8.0));
        assert_eq!(report.decode_fps, 120.0);
        abr.add(120, 0, 120);
        assert_eq!(abr.due(8000, None), None, "the last report is not answered yet");
        let moved = AbrAction { new_bitrate: Some(28000), bitrate_applied: true, ..Default::default() };
        assert_eq!(abr.answered(Some(&moved)), Some(28000));
        let next = abr.due(8000, None).unwrap();
        assert_eq!((next.current_bitrate, next.dropped_frames, next.rtt_ms), (28000, 0, 0.0));
        assert_eq!(abr.answered(Some(&AbrAction::default())), None, "kept");
        assert_eq!(abr.answered(None), None, "a failed report changes nothing");
        assert_eq!(abr.current_kbps, 28000);
    }

    #[test]
    fn whichever_side_comes_second_turns_the_pcs_controller_off() {
        // The stream ends while the PC turns it on: the stream has nothing to stop, the start does.
        let start = AbrStart::default();
        assert!(!start.clone().end(), "nothing is on yet");
        assert!(!start.turned_on(), "the late start turns it off itself");
        // The PC turned it on first: the stream turns it off, even before it saw the answer.
        let start = AbrStart::default();
        assert!(start.clone().turned_on());
        assert!(start.end());
        assert!(!start.end(), "once");
        // The PC had none, or refused: the start never turned it on, nothing to stop.
        assert!(!AbrStart::default().end());
    }

    #[test]
    fn the_stop_does_not_hold_the_caller_and_a_later_start_waits_for_it() {
        use std::sync::mpsc;
        let order = Arc::new(Mutex::new(Vec::new()));
        let (release, held) = mpsc::channel::<()>();
        let stop_order = order.clone();
        // The PC does not answer: the request hangs until released.
        let stop = AbrStop::spawn(move || {
            held.recv().unwrap();
            stop_order.lock().unwrap().push("off");
        });
        // The caller (the window thread) is back at once, before the request ended.
        order.lock().unwrap().push("caller");
        let start_order = order.clone();
        let start = std::thread::spawn(move || {
            stop.wait();
            start_order.lock().unwrap().push("on");
        });
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(*order.lock().unwrap(), ["caller"], "the start waits for the stop");
        release.send(()).unwrap();
        start.join().unwrap();
        assert_eq!(*order.lock().unwrap(), ["caller", "off", "on"]);
    }
}
