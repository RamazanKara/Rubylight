//! The cancel state machine: the connecting screen leaves at once, the worker ends on its own,
//! and a start that stops answering ends at [`CONNECT_TIMEOUT`].

use super::*;
use super::replace_tests::connecting;

type ConnectResult = Result<(Session, Facts, LaunchPlan), ConnectError>;

#[test]
fn cancel_returns_to_the_pc_list_before_the_worker_answers() {
    let (_sender, result) = channel::<ConnectResult>();
    let (release, held) = channel::<()>();
    // Deaf to the cancel, as a decoder being made on a resetting driver is.
    let worker = std::thread::spawn(move || { let _ = held.recv(); });
    let mut attempts = Attempts::default();
    attempts.begin(connecting(result, Some(worker)));
    assert!(attempts.is_connecting());
    assert!(attempts.cancel());
    assert!(!attempts.is_connecting(), "the connecting screen is gone while the worker runs");
    assert!(attempts.is_busy(), "its worker may still use the renderer's device");
    assert!(!attempts.cancel(), "there is no start left on the screen to cancel");
    let _ = release.send(());
    assert_eq!(attempts.take_all().len(), 1);
}

#[test]
fn a_cancelled_start_that_answers_is_reaped_without_a_message() {
    let (sender, result) = channel::<ConnectResult>();
    let (release, held) = channel::<()>();
    let (sent, answered) = channel::<()>();
    let worker = std::thread::spawn(move || {
        let _ = held.recv();
        let _ = sender.send(Err("refused".into()));
        let _ = sent.send(());
    });
    let mut attempts = Attempts::default();
    attempts.begin(connecting(result, Some(worker)));
    assert!(attempts.cancel());
    assert!(!attempts.reap(), "the worker has not answered yet");
    let _ = release.send(());
    answered.recv_timeout(Duration::from_secs(5)).expect("the worker answered");
    assert!(attempts.reap());
    assert!(!attempts.is_busy(), "an ending attempt leaves no screen and no device hold");
}

#[test]
fn a_start_that_stops_answering_ends_at_the_deadline() {
    let started = Instant::now();
    let (_sender, result) = channel::<ConnectResult>();
    let mut waiting = connecting(result, None);
    waiting.progress = started;
    let mut attempts = Attempts::default();
    attempts.begin(waiting);
    assert!(!attempts.overdue(started + CONNECT_TIMEOUT - Duration::from_millis(1)), "the deadline is the last moment");
    assert!(attempts.overdue(started + CONNECT_TIMEOUT), "the deadline ends it");
    let ended = attempts.take().expect("the overdue start");
    attempts.end(ended);
    assert!(!attempts.is_connecting(), "the timeout returns to the PC list");
    assert!(attempts.is_busy(), "its worker may still use the renderer's device");
    assert!(!attempts.overdue(started + CONNECT_TIMEOUT), "nothing is left on the screen to time out");
}

/// The timeout is a local decoder or device call that cannot answer: its words name this device's
/// graphics driver, not a PC that stopped answering (the network reads are bounded well below it).
#[test]
fn the_timeout_names_the_graphics_driver_and_its_remedy() {
    let text = connect_timeout_text();
    assert!(text.contains("graphics driver"), "{text}");
    assert!(text.contains("decoder"), "{text}");
    assert!(text.contains("update or reinstall the graphics driver"), "{text}");
    assert!(!text.contains("restart this PC"), "{text}");
}

/// The result is read before the deadline is applied (`App::launcher_frame`): a worker that
/// answered just under the deadline is not reported as stopped answering.
#[test]
fn a_result_that_arrives_at_the_deadline_is_read_not_timed_out() {
    let (sender, result) = channel::<ConnectResult>();
    let mut answered = connecting(result, None);
    answered.progress = Instant::now() - CONNECT_TIMEOUT;
    let mut attempts = Attempts::default();
    attempts.begin(answered);
    assert!(attempts.overdue(Instant::now()), "the deadline has passed");
    let _ = sender.send(Err("refused".into()));
    let mut connecting = attempts.take().expect("the start on the screen");
    assert!(
        matches!(connecting.poll(), Err(ConnectFailed::Message(message)) if message == "refused"),
        "the answer is read, not the deadline"
    );
}

/// A stage notice is progress: the deadline counts from it, not from the start, so a slow launch
/// that keeps moving is not reported as stuck.
#[test]
fn a_stage_notice_restarts_the_deadline() {
    let (_sender, result) = channel::<ConnectResult>();
    let mut waiting = connecting(result, None);
    waiting.progress = Instant::now() - CONNECT_TIMEOUT;
    waiting.shared.notify(Notice::Stage(Stage::VideoStreamInit));
    let now = Instant::now();
    assert!(waiting.overdue(now), "nothing has moved since the start");
    assert!(matches!(waiting.poll(), Ok(None)), "the notice is read");
    assert!(!waiting.overdue(now), "the stage it reached restarts the deadline");
    assert_eq!(waiting.stage, "Starting video");
}

/// `begin`'s one-start-at-a-time rule holds in release builds too: the older attempt is kept as
/// ending (and cancelled), never dropped with its device hold.
#[test]
fn a_second_begin_keeps_the_older_attempt_as_ending() {
    let (_first_sender, first_result) = channel::<ConnectResult>();
    let (_second_sender, second_result) = channel::<ConnectResult>();
    let (release, held) = channel::<()>();
    let worker = std::thread::spawn(move || { let _ = held.recv(); });
    let mut attempts = Attempts::default();
    let first = connecting(first_result, Some(worker));
    let first_interrupt = first.interrupt.clone();
    attempts.begin(first);
    attempts.begin(connecting(second_result, None));
    assert!(first_interrupt.is_interrupted(), "the older attempt is told to stop, not dropped");
    assert!(attempts.is_connecting() && attempts.is_busy(), "the older one stays as ending");
    let _ = release.send(());
    assert_eq!(attempts.take_all().len(), 2);
}

/// A start that answered in time is taken for streaming, not ended, and is never overdue.
#[test]
fn a_start_that_answers_is_taken_not_ended() {
    let (sender, result) = channel::<ConnectResult>();
    let mut attempts = Attempts::default();
    attempts.begin(connecting(result, None));
    let _ = sender.send(Err("refused".into()));
    match attempts.take() {
        Some(mut connecting) => {
            assert!(!connecting.overdue(Instant::now()));
            assert!(matches!(connecting.poll(), Err(ConnectFailed::Message(message)) if message == "refused"));
        }
        None => panic!("the start was on the screen"),
    }
    assert!(!attempts.is_busy());
}

#[test]
fn leaving_waits_for_the_screen_and_the_ending_attempts_under_one_wait() {
    let (release_ending, ending_holds) = channel::<()>();
    let ending_worker = std::thread::spawn(move || { let _ = ending_holds.recv(); });
    let (release_screen, screen_holds) = channel::<()>();
    let screen_worker = std::thread::spawn(move || { let _ = screen_holds.recv(); });
    let (_ending_sender, ending_result) = channel::<ConnectResult>();
    let (_screen_sender, screen_result) = channel::<ConnectResult>();
    let mut attempts = Attempts::default();
    attempts.begin(connecting(ending_result, Some(ending_worker)));
    assert!(attempts.cancel());
    let screen = connecting(screen_result, Some(screen_worker));
    let screen_interrupt = screen.interrupt.clone();
    attempts.begin(screen);
    let started = Instant::now();
    assert!(!end_all_before(attempts.take_all(), Duration::from_millis(100), || panic!("torn down under a running launch")));
    assert!(screen_interrupt.is_interrupted(), "the attempt on the screen was cancelled although the one before it was stuck");
    assert!(started.elapsed() < Duration::from_secs(5), "the wait is bounded");
    let _ = release_ending.send(());
    let _ = release_screen.send(());
}
