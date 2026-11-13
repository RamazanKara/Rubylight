use super::*;
use nightjar_core::http::native_api::error::{ApiError, ErrorCode};
use nightjar_core::http::{CoopEnd, CoopGame, CoopPlayer};
use std::sync::Mutex;

#[derive(Default)]
pub(crate) struct Fake {
    pub(crate) calls: Mutex<Vec<String>>,
    /// The join's refusal; without one it seats the device in slot 1.
    pub(crate) refuse: Option<Error>,
    pub(crate) app_id: u32,
    pub(crate) cancelled: std::sync::atomic::AtomicBool,
}

pub(crate) fn api(status: u16, code: ErrorCode, message: &str) -> Error {
    Error::Api {
        status,
        error: ApiError {
            code,
            message: message.into(),
            setting: None,
            retry_after_ms: None,
        },
    }
}

fn session(you: bool) -> CoopSession {
    CoopSession {
        id: 3,
        app_id: 1234567,
        title: "Portal 2".into(),
        game: "g".into(),
        phase: CoopPhase::Running,
        end: None,
        players: vec![
            CoopPlayer {
                slot: 0,
                name: Some("Fold7".into()),
                you: false,
                away: false,
            },
            CoopPlayer {
                slot: 1,
                name: you.then(|| "Desk".into()),
                you,
                away: false,
            },
        ],
        free_slots: u32::from(!you),
        joinable: true,
        refusal: None,
    }
}

impl CoopSource for Fake {
    fn coop(&self) -> Result<Coop, Error> {
        let mut calls = self.calls.lock().unwrap();
        calls.push("coop".into());
        let seated = calls.iter().any(|c| c.starts_with("join")) && self.refuse.is_none();
        Ok(Coop {
            available: true,
            reason: None,
            games: vec![CoopGame {
                app_id: 1234567,
                title: "Portal 2 (Co-op)".into(),
                game: "g".into(),
                max_players: 2,
            }],
            sessions: vec![session(seated)],
        })
    }

    fn join(&self, session: u32) -> Result<CoopJoined, Error> {
        self.calls.lock().unwrap().push(format!("join {session}"));
        match &self.refuse {
            Some(Error::Api { status, error }) => Err(api(*status, error.code, &error.message)),
            Some(_) => Err(Error::Cancelled),
            None => Ok(CoopJoined {
                session,
                slot: 1,
                app_id: self.app_id,
            }),
        }
    }

    fn cancel(&self) {
        self.cancelled
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

pub(crate) fn waker() -> Waker {
    Arc::new(|| {})
}

/// Polls until nothing is running; the launches the joins gave.
fn settle(coop: &mut CoopSessions, host: u64) -> Vec<(u64, i32)> {
    let mut launches = Vec::new();
    for _ in 0..500 {
        launches.extend(coop.poll(Some(host)));
        if coop
            .state_for(host)
            .is_some_and(|open| !open.loading && open.joining.is_none())
        {
            return launches;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("the co-op list did not settle");
}

pub(crate) fn calls(fake: &Fake) -> Vec<String> {
    fake.calls.lock().unwrap().clone()
}

#[test]
fn a_join_seats_this_device_and_launches_the_sessions_entry() {
    let fake = Arc::new(Fake {
        app_id: 1234567,
        ..Fake::default()
    });
    let mut coop = CoopSessions::default();
    coop.open(7, fake.clone(), waker());
    assert!(settle(&mut coop, 7).is_empty(), "listing launches nothing");
    let listed = coop.state_for(7).unwrap().coop.clone().unwrap();
    assert_eq!(listed.sessions[0].free_slots, 1);
    assert!(!listed.sessions[0].seated());
    assert_eq!(join_label(&listed.sessions[0]), "Join");
    coop.join(7, 3);
    // A second click while the first is on its way asks nothing.
    coop.join(7, 3);
    assert_eq!(settle(&mut coop, 7), [(7, 1234567)]);
    let open = coop.state_for(7).unwrap();
    assert_eq!(
        open.done.as_deref(),
        Some("Joined Portal 2 as player 2. Starting the stream…")
    );
    assert_eq!(open.error, None);
    // The listing after the join shows this device's own seat.
    let session = &open.coop.as_ref().unwrap().sessions[0];
    assert!(session.seated() && session.players[1].you);
    assert_eq!(join_label(session), "Return to my slot");
    assert_eq!(calls(&fake), ["coop", "join 3", "coop"]);
    // Another PC's page sees nothing of it, and does nothing to it.
    assert!(coop.state_for(8).is_none());
    coop.join(8, 3);
    assert_eq!(calls(&fake).len(), 3);
}

#[test]
fn every_refusal_is_shown_with_the_pcs_words_and_launches_nothing() {
    let permission = "Turn on “Can join co-op games” for this device in the Rubylight console.";
    for (error, page) in [
        (
            api(403, ErrorCode::PermissionRequired, permission),
            "This device may not join co-op sessions on this PC.",
        ),
        (
            api(409, ErrorCode::Unknown, "The session is full"),
            "The PC refused the join.",
        ),
        (
            api(409, ErrorCode::NoSession, "No such session"),
            "The co-op session is no longer running.",
        ),
        (
            api(
                503,
                ErrorCode::TemporarilyUnavailable,
                "Nucleus Co-op is not installed",
            ),
            "The PC cannot run co-op sessions right now.",
        ),
        (
            api(409, ErrorCode::SessionBusy, "Nucleus Co-op runs Halo"),
            "Nucleus Co-op on the PC is busy with another session.",
        ),
    ] {
        let message = match &error {
            Error::Api { error, .. } => error.message.clone(),
            _ => unreachable!(),
        };
        let fake = Arc::new(Fake {
            refuse: Some(error),
            ..Fake::default()
        });
        let mut coop = CoopSessions::default();
        coop.open(7, fake.clone(), waker());
        settle(&mut coop, 7);
        coop.join(7, 3);
        assert!(settle(&mut coop, 7).is_empty(), "{page}");
        let open = coop.state_for(7).unwrap();
        assert_eq!(
            open.error.as_deref(),
            Some(format!("{page}\n{message}").as_str())
        );
        assert_eq!(open.done, None);
        assert_eq!(
            calls(&fake),
            ["coop", "join 3", "coop"],
            "the listing is asked again"
        );
        // Refresh starts over: the refusal goes, and the listing that comes back says nothing.
        coop.refresh();
        settle(&mut coop, 7);
        assert_eq!(coop.state_for(7).unwrap().error, None, "{page}");
    }
    // Anything else reads as the error does.
    assert_eq!(
        problem_text(&CoopProblem::Other("timed out".into())),
        "timed out"
    );
}

#[test]
fn an_app_id_out_of_the_launch_range_is_refused_loudly() {
    let fake = Arc::new(Fake {
        app_id: u32::MAX,
        ..Fake::default()
    });
    let mut coop = CoopSessions::default();
    coop.open(7, fake, waker());
    settle(&mut coop, 7);
    coop.join(7, 3);
    assert!(settle(&mut coop, 7).is_empty());
    let open = coop.state_for(7).unwrap();
    assert_eq!(
        open.error.as_deref(),
        Some("The PC named an app this version cannot launch: 4294967295")
    );
    assert_eq!(open.done, None);
}

#[test]
fn leaving_the_page_cancels_and_a_late_answer_is_dropped() {
    let fake = Arc::new(Fake::default());
    let mut coop = CoopSessions::default();
    coop.open(7, fake.clone(), waker());
    coop.join(7, 3);
    assert_eq!(coop.poll(Some(8)), None);
    assert!(coop.state_for(7).is_none());
    assert!(fake.cancelled.load(std::sync::atomic::Ordering::SeqCst));
    // A failed open is shown on its own page only, and goes with it.
    coop.fail(7, "no identity".into());
    assert_eq!(coop.failure_for(7), Some("no identity"));
    assert_eq!(coop.failure_for(8), None);
    coop.poll(None);
    assert_eq!(coop.failure_for(7), None);
}

#[test]
fn slots_phases_and_ends_read_as_the_page_shows_them() {
    assert_eq!(slot_text(0, Some("Fold7"), false, false), "Player 1: Fold7");
    assert_eq!(slot_text(1, None, false, false), "Player 2: Free");
    assert_eq!(
        slot_text(1, Some("Desk"), true, true),
        "Player 2: This device (away, the slot is held)"
    );
    let mut ended = session(false);
    assert_eq!(phase_text(&ended), "Running");
    ended.phase = CoopPhase::Ended;
    ended.end = Some(CoopEnd {
        reason: CoopEndReason::Crashed,
        detail: Some("Player 2's instance of Portal 2 exited".into()),
    });
    assert_eq!(
        phase_text(&ended),
        "Ended: a game instance crashed (Player 2's instance of Portal 2 exited)"
    );
    ended.end = Some(CoopEnd {
        reason: CoopEndReason::Empty,
        detail: None,
    });
    assert_eq!(phase_text(&ended), "Ended: every player left");
}
