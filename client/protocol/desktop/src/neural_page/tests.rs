use super::*;
use nightjar_core::http::native_api::error::{ApiError, ErrorCode};
use nightjar_core::http::native_api::radeon::ControlState;
use nightjar_core::http::{NeuralAction, NeuralJob};
use std::sync::Mutex;

/// A PC whose game has frame generation off; a write turns it on through one download.
#[derive(Default)]
struct Fake {
    calls: Mutex<Vec<String>>,
    /// The write's refusal.
    refuse: Option<String>,
    /// Reads after the first one fail.
    unreachable: bool,
    /// Reads after the first one find the PC busy with the game until this many were made.
    busy_until: usize,
    cancelled: std::sync::atomic::AtomicBool,
}

fn state(on: bool, phase: Option<NeuralPhase>) -> Neural {
    Neural {
        app: "uuid-1".into(),
        accepted: false,
        gpu: Some("AMD Radeon 780M".into()),
        switches: vec![NeuralSwitch {
            id: FRAME_GENERATION.into(),
            state: ControlState::Configurable,
            on,
            reason: None,
        }],
        replaces_earlier_upscaling: !on,
        job: phase.map(|phase| NeuralJob {
            action: NeuralAction::Install,
            phase,
            done: 7_000_000,
            total: 14_000_000,
            error: None,
        }),
    }
}

impl NeuralSource for Fake {
    fn neural(&self, app: &str) -> Result<Neural, Error> {
        let mut calls = self.calls.lock().unwrap();
        calls.push(format!("read {app}"));
        if self.unreachable && calls.len() > 1 {
            return Err(Error::Cancelled);
        }
        let reads = calls.iter().filter(|c| c.starts_with("read")).count();
        if reads > 1 && reads <= self.busy_until {
            return Ok(state(false, Some(NeuralPhase::Remove)));
        }
        let wrote = calls.iter().any(|c| c.starts_with("write")) && self.refuse.is_none();
        let reads_since = calls
            .iter()
            .rev()
            .take_while(|c| !c.starts_with("write"))
            .count();
        // After the write: one read while it copies, then done.
        Ok(match (wrote, reads_since) {
            (false, _) => state(false, None),
            (true, 1) => state(true, Some(NeuralPhase::Place)),
            (true, _) => state(true, Some(NeuralPhase::Done)),
        })
    }

    fn write(&self, write: &NeuralWrite) -> Result<Neural, Error> {
        self.calls.lock().unwrap().push(format!(
            "write {} {} {} {}",
            write.app, write.switch, write.on, write.accept_notice
        ));
        match &self.refuse {
            Some(message) => Err(Error::Api {
                status: 409,
                error: ApiError {
                    code: ErrorCode::TemporarilyUnavailable,
                    message: message.clone(),
                    setting: Some(FRAME_GENERATION.into()),
                    retry_after_ms: None,
                },
            }),
            None => Ok(state(true, Some(NeuralPhase::Download))),
        }
    }

    fn cancel(&self) {
        self.cancelled
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

fn waker() -> Waker {
    Arc::new(|| {})
}

fn settle(panel: &mut NeuralPanel) {
    for _ in 0..800 {
        panel.poll(Some((1, 7)));
        if panel
            .state_for(1, 7)
            .is_some_and(|open| !open.loading && !open.writing)
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("the panel did not settle");
}

#[test]
fn turning_it_on_carries_the_notice_and_reads_until_the_pc_is_done() {
    let fake = Arc::new(Fake::default());
    let mut panel = NeuralPanel::default();
    assert!(panel.wanted(1, 7));
    panel.open(1, 7, "uuid-1".into(), fake.clone(), waker());
    assert!(!panel.wanted(1, 7));
    settle(&mut panel);
    let open = panel.state_for(1, 7).unwrap();
    let neural = open.state.as_ref().unwrap();
    assert!(!neural.frame_generation().unwrap().on && neural.replaces_earlier_upscaling);

    panel.write(1, 7, true, true);
    // A second press while the first is on its way sends nothing.
    panel.write(1, 7, true, true);
    settle(&mut panel);
    let neural = panel.state_for(1, 7).unwrap().state.clone().unwrap();
    assert!(neural.frame_generation().unwrap().on);
    assert!(!neural.working(), "{neural:?}");
    let calls = fake.calls.lock().unwrap().clone();
    assert_eq!(
        calls,
        [
            "read uuid-1",
            "write uuid-1 frame_generation true true",
            "read uuid-1",
            "read uuid-1"
        ]
    );
}

#[test]
fn a_refused_write_says_the_pcs_words_and_shows_the_game_as_it_is() {
    let fake = Arc::new(Fake {
        refuse: Some("Turn it on when no stream is running.".into()),
        ..Fake::default()
    });
    let mut panel = NeuralPanel::default();
    panel.open(1, 7, "uuid-1".into(), fake.clone(), waker());
    settle(&mut panel);
    panel.write(1, 7, true, false);
    settle(&mut panel);
    let open = panel.state_for(1, 7).unwrap();
    assert_eq!(
        open.error.as_deref(),
        Some("Turn it on when no stream is running.")
    );
    assert!(!open.state.as_ref().unwrap().frame_generation().unwrap().on);
}

#[test]
fn leaving_the_game_page_cancels_and_closes() {
    let fake = Arc::new(Fake::default());
    let mut panel = NeuralPanel::default();
    panel.open(1, 7, "uuid-1".into(), fake.clone(), waker());
    panel.poll(Some((1, 8)));
    assert!(panel.state_for(1, 7).is_none());
    assert!(fake.cancelled.load(std::sync::atomic::Ordering::SeqCst));
}

#[test]
fn the_lines_say_the_job_and_the_switch() {
    let mut neural = state(false, Some(NeuralPhase::Download));
    assert_eq!(
        job_text(&neural).as_deref(),
        Some("Downloading: 7.0 of 14.0 MB")
    );
    neural.job.as_mut().unwrap().phase = NeuralPhase::Failed;
    neural.job.as_mut().unwrap().error = Some("The game uses BattlEye.".into());
    assert_eq!(job_text(&neural).as_deref(), Some("The game uses BattlEye."));
    neural.job = None;
    assert_eq!(job_text(&neural), None);
    let mut switch = neural.switches[0].clone();
    assert_eq!(switch_text(&switch), "Off");
    switch.state = ControlState::NeedsExternalSetup;
    switch.reason = Some("Choose the game's program in the Rubylight console on the PC.".into());
    assert_eq!(
        switch_text(&switch),
        "Choose the game's program in the Rubylight console on the PC."
    );
    // A device without Host control can't press it, so it never shows the notice.
    switch.state = ControlState::ReadOnly;
    switch.reason = Some("Turn on Host control for this device.".into());
    assert!(!configurable(&switch));
    assert_eq!(switch_text(&switch), "Turn on Host control for this device.");
}

/// A write refused while the PC works on the game keeps its words through the read after it,
/// reads again at the polling pace, not in a tight loop, and a later good read clears them.
#[test]
fn a_refusal_during_a_job_polls_at_its_pace_and_clears() {
    let fake = Arc::new(Fake {
        refuse: Some("Neural Rendering is busy with this game.".into()),
        busy_until: 3,
        ..Fake::default()
    });
    let mut panel = NeuralPanel::default();
    panel.open(1, 7, "uuid-1".into(), fake.clone(), waker());
    settle(&mut panel);
    panel.write(1, 7, true, false);
    // Until the read after the refusal is in (settle would wait out the next poll).
    for _ in 0..800 {
        panel.poll(Some((1, 7)));
        if panel.state_for(1, 7).unwrap().state.as_ref().unwrap().working() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let open = panel.state_for(1, 7).unwrap();
    assert_eq!(
        open.error.as_deref(),
        Some("Neural Rendering is busy with this game.")
    );
    assert!(open.state.as_ref().unwrap().working());
    for _ in 0..40 {
        panel.poll(Some((1, 7)));
        std::thread::sleep(Duration::from_millis(5));
    }
    // read, write, the read after it; the next waits for POLL.
    assert_eq!(fake.calls.lock().unwrap().len(), 3);
    let deadline = std::time::Instant::now() + POLL * 4;
    while panel.state_for(1, 7).unwrap().error.is_some() {
        assert!(std::time::Instant::now() < deadline, "the error stayed");
        panel.poll(Some((1, 7)));
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(fake.calls.lock().unwrap().len(), 4);
}

/// A read that fails after a good one is shown and not repeated on its own.
#[test]
fn a_failed_read_is_not_retried_by_itself() {
    let fake = Arc::new(Fake {
        unreachable: true,
        ..Fake::default()
    });
    let mut panel = NeuralPanel::default();
    panel.open(1, 7, "uuid-1".into(), fake.clone(), waker());
    settle(&mut panel);
    panel.read(Duration::ZERO);
    settle(&mut panel);
    for _ in 0..20 {
        panel.poll(Some((1, 7)));
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(panel.state_for(1, 7).unwrap().error.is_some());
    assert_eq!(fake.calls.lock().unwrap().len(), 2);
}
