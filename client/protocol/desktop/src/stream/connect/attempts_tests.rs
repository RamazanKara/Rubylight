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
    waiting.started = started;
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
    attempts.begin(connecting(screen_result, Some(screen_worker)));
    let started = Instant::now();
    assert!(!end_all_before(attempts.take_all(), Duration::from_millis(100), || panic!("torn down under a running launch")));
    assert!(started.elapsed() < Duration::from_secs(5), "the wait is bounded");
    let _ = release_ending.send(());
    let _ = release_screen.send(());
}
