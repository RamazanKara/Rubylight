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
pub(crate) mod tests;
