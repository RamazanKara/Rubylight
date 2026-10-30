//! Nucleus Co-op sessions on a PC's details page (Experimental): the co-op games, the running
//! sessions with their players, free slots and this device's own seat, and a join into a free
//! slot (or back into this device's own). A join holds the slot on the PC for a while; the stream
//! is then the normal launch of the session's entry. One request at a time on a worker thread;
//! leaving the page cancels it.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use nightjar_core::http::{
    ClientIdentity, Coop, CoopEndReason, CoopJoined, CoopPhase, CoopProblem, CoopSession, Error,
    HostClient,
};

use crate::hosts::{client_for, Host, Waker};
use crate::strings::{en, tr, trf};

/// The co-op calls against one PC; `HostClient` in the app, a fake in tests.
pub trait CoopSource: Send + Sync {
    fn coop(&self) -> Result<Coop, Error>;
    fn join(&self, session: u32) -> Result<CoopJoined, Error>;
    /// Ends every request in flight, and every later one, with `Error::Cancelled`.
    fn cancel(&self);
}

impl CoopSource for HostClient {
    fn coop(&self) -> Result<Coop, Error> {
        HostClient::coop(self)
    }

    fn join(&self, session: u32) -> Result<CoopJoined, Error> {
        self.join_coop(session)
    }

    fn cancel(&self) {
        self.cancel_pending_requests();
    }
}

enum Answer {
    Listed(u64, Result<Coop, CoopProblem>),
    Joined(u32, Result<CoopJoined, CoopProblem>),
}

/// The co-op list of the page whose PC it was opened for.
#[derive(Default)]
pub struct CoopSessions {
    open: Option<Open>,
    /// The PC whose list could not open, and why.
    failed: Option<(u64, String)>,
}

pub struct Open {
    pub host: u64,
    source: Arc<dyn CoopSource>,
    waker: Waker,
    /// The last listing, kept while the next one loads.
    pub coop: Option<Coop>,
    /// A listing is on its way.
    pub loading: bool,
    /// The number of the listing being waited for; a later one replaces it.
    ask: u64,
    /// The session a join is on its way for.
    pub joining: Option<u32>,
    /// Why the last listing or join failed, in the page's words and the PC's.
    pub error: Option<String>,
    /// What the last join did.
    pub done: Option<String>,
    answers: Receiver<Answer>,
    sender: Sender<Answer>,
}

impl Drop for Open {
    fn drop(&mut self) {
        self.source.cancel();
    }
}

/// Command::Coop: opens the co-op list of `host` with a client of its own, since closing the
/// list cancels that client's requests for good.
pub fn start(
    coop: &mut CoopSessions,
    identity: Option<&Arc<ClientIdentity>>,
    host: &Host,
    waker: Waker,
) {
    let client = match identity {
        Some(identity) => client_for(identity, &host.record, host.connect_address()),
        None => Err(
            tr("Nightjar has no client identity yet. Restart Nightjar and try again.").to_owned(),
        ),
    };
    match client {
        Ok(client) => coop.open(host.id, Arc::new(client), waker),
        Err(error) => coop.fail(host.id, error),
    }
}

impl CoopSessions {
    /// Opens the list for `host` and asks the PC for it.
    pub fn open(&mut self, host: u64, source: Arc<dyn CoopSource>, waker: Waker) {
        let (sender, answers) = channel();
        self.failed = None;
        self.open = Some(Open {
            host,
            source,
            waker,
            coop: None,
            loading: false,
            ask: 0,
            joining: None,
            error: None,
            done: None,
            answers,
            sender,
        });
        self.list();
    }

    /// The list could not open: says why on the page.
    pub fn fail(&mut self, host: u64, error: String) {
        self.open = None;
        self.failed = Some((host, error));
    }

    pub fn close(&mut self) {
        self.open = None;
        self.failed = None;
    }

    /// Why the list of `host` could not open.
    pub fn failure_for(&self, host: u64) -> Option<&str> {
        self.failed
            .as_ref()
            .filter(|(h, _)| *h == host)
            .map(|(_, e)| e.as_str())
    }

    pub fn state_for(&self, host: u64) -> Option<&Open> {
        self.open.as_ref().filter(|open| open.host == host)
    }

    /// Asks the PC for its co-op games and sessions again.
    pub fn list(&mut self) {
        let Some(open) = &mut self.open else { return };
        open.ask += 1;
        open.loading = true;
        let (ask, source, sender, waker) = (
            open.ask,
            open.source.clone(),
            open.sender.clone(),
            open.waker.clone(),
        );
        let started = spawn("co-op list", move || {
            let _ = sender.send(Answer::Listed(
                ask,
                source.coop().map_err(|e| CoopProblem::of(&e)),
            ));
            waker();
        });
        if let Err(e) = started {
            open.loading = false;
            open.error = Some(e);
        }
    }

    /// Refresh on the page: lists again and clears what the last listing or join said.
    pub fn refresh(&mut self) {
        if let Some(open) = &mut self.open {
            open.error = None;
            open.done = None;
        }
        self.list();
    }

    /// Joins `session` of `host`: a free slot, or this device's own one. A second join while one
    /// is on its way does nothing; the button is off meanwhile.
    pub fn join(&mut self, host: u64, session: u32) {
        let Some(open) = self.open.as_mut().filter(|open| open.host == host) else {
            return;
        };
        if open.joining.is_some() {
            return;
        }
        open.joining = Some(session);
        open.error = None;
        open.done = None;
        let (source, sender, waker) =
            (open.source.clone(), open.sender.clone(), open.waker.clone());
        let started = spawn("co-op join", move || {
            let _ = sender.send(Answer::Joined(
                session,
                source.join(session).map_err(|e| CoopProblem::of(&e)),
            ));
            waker();
        });
        if let Err(e) = started {
            open.joining = None;
            open.error = Some(e);
        }
    }

    /// Takes in what the workers found. `page` is the PC whose details page is open or on the
    /// way back: leaving it closes the list, which cancels what is running. A join that went
    /// through gives the PC and the app to launch, which streams the joined slot.
    pub fn poll(&mut self, page: Option<u64>) -> Option<(u64, i32)> {
        if self
            .failed
            .as_ref()
            .is_some_and(|(host, _)| page != Some(*host))
        {
            self.failed = None;
        }
        if self
            .open
            .as_ref()
            .is_some_and(|open| page != Some(open.host))
        {
            self.open = None;
            return None;
        }
        let open = self.open.as_mut()?;
        let (mut launch, mut relist) = (None, false);
        while let Ok(answer) = open.answers.try_recv() {
            match answer {
                Answer::Listed(ask, _) if ask != open.ask => {}
                Answer::Listed(_, result) => {
                    open.loading = false;
                    match result {
                        Ok(coop) => open.coop = Some(coop),
                        Err(problem) => open.error = Some(problem_text(&problem)),
                    }
                }
                Answer::Joined(session, result) => {
                    open.joining = None;
                    // The listing shows the seat; a refusal may have changed the session too.
                    relist = true;
                    let title = open
                        .coop
                        .as_ref()
                        .and_then(|coop| coop.sessions.iter().find(|s| s.id == session))
                        .map_or_else(String::new, |s| s.title.clone());
                    match result {
                        Ok(joined) => match i32::try_from(joined.app_id) {
                            Ok(app) => {
                                open.done = Some(trf(
                                    "Joined {} as player {}. Starting the stream…",
                                    &[&title, &(joined.slot + 1)],
                                ));
                                launch = Some((open.host, app));
                            }
                            Err(_) => {
                                open.error = Some(trf(
                                    "The PC named an app this version cannot launch: {}",
                                    &[&joined.app_id],
                                ));
                            }
                        },
                        Err(problem) => open.error = Some(problem_text(&problem)),
                    }
                }
            }
        }
        if relist {
            self.list();
        }
        launch
    }
}

/// Runs `work` on its own thread; why it could not start, as the page says it.
fn spawn(name: &str, work: impl FnOnce() + Send + 'static) -> Result<(), String> {
    std::thread::Builder::new()
        .name(name.into())
        .spawn(work)
        .map(drop)
        .map_err(|e| trf("Could not start the request: {}", &[&e]))
}

/// What failed, in the page's words, then the PC's message as it came.
pub fn problem_text(problem: &CoopProblem) -> String {
    let why = match problem {
        CoopProblem::Permission(_) => {
            Some(en("This device may not join co-op sessions on this PC."))
        }
        CoopProblem::Refused(_) => Some(en("The PC refused the join.")),
        CoopProblem::Gone(_) => Some(en("The co-op session is no longer running.")),
        CoopProblem::Unavailable(_) => Some(en("The PC cannot run co-op sessions right now.")),
        CoopProblem::Busy(_) => Some(en("Nucleus Co-op on the PC is busy with another session.")),
        CoopProblem::Other(_) => None,
    };
    match why {
        Some(why) => format!("{}\n{}", tr(why), problem.message()),
        None => problem.message().to_owned(),
    }
}

/// A session's phase as the page names it, with how it ended.
pub fn phase_text(session: &CoopSession) -> String {
    let phase = match session.phase {
        CoopPhase::Starting => tr("Starting"),
        CoopPhase::Running => tr("Running"),
        CoopPhase::Ended => tr("Ended"),
        CoopPhase::Unknown => tr("Unknown state"),
    };
    let Some(end) = &session.end else {
        return phase.to_owned();
    };
    let reason = match end.reason {
        CoopEndReason::Ended => tr("ended on the PC"),
        CoopEndReason::Empty => tr("every player left"),
        CoopEndReason::Crashed => tr("a game instance crashed"),
        CoopEndReason::Failed => tr("the start failed"),
        CoopEndReason::Unknown => tr("for a reason this version does not know"),
    };
    match &end.detail {
        Some(detail) => format!("{phase}: {reason} ({detail})"),
        None => format!("{phase}: {reason}"),
    }
}

/// One slot as the page lists it: the player, or free, and this device's own seat.
pub fn slot_text(slot: u32, name: Option<&str>, you: bool, away: bool) -> String {
    let who = match (name, you) {
        (_, true) => tr("This device").to_owned(),
        (Some(name), false) => name.to_owned(),
        (None, false) => tr("Free").to_owned(),
    };
    let line = trf("Player {}: {}", &[&(slot + 1), &who]);
    if away {
        format!("{line} ({})", tr("away, the slot is held"))
    } else {
        line
    }
}

/// The join button's label for a session: back into this device's seat, or into a free slot.
pub fn join_label(session: &CoopSession) -> &'static str {
    if session.seated() {
        en("Return to my slot")
    } else {
        en("Join")
    }
}

#[cfg(test)]
pub(crate) mod tests {
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
}
