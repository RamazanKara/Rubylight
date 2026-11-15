//! A game's Neural Rendering on its page, while the PC lists `neural.frame_generation`: the frame
//! generation switch, the PC's one-time notice before the first install, what turning it on
//! replaces, and the PC's job while it downloads, copies or removes. The PC does the work on its
//! own after a write, so the page reads again while it runs. One request at a time on a worker
//! thread; leaving the page cancels it.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

use nightjar_core::http::{
    ClientIdentity, Error, HostClient, Neural, NeuralPhase, NeuralSwitch, NeuralWrite,
    FRAME_GENERATION,
};

use crate::hosts::{client_for, Host, Waker};
use crate::strings::{tr, trf};

/// How soon the page reads again while the PC works.
pub const POLL: Duration = Duration::from_millis(1500);

/// The calls against one PC; `HostClient` in the app, a fake in tests.
pub trait NeuralSource: Send + Sync {
    fn neural(&self, app: &str) -> Result<Neural, Error>;
    fn write(&self, write: &NeuralWrite) -> Result<Neural, Error>;
    /// Ends every request in flight, and every later one, with `Error::Cancelled`.
    fn cancel(&self);
}

impl NeuralSource for HostClient {
    fn neural(&self, app: &str) -> Result<Neural, Error> {
        HostClient::neural(self, app)
    }

    fn write(&self, write: &NeuralWrite) -> Result<Neural, Error> {
        self.write_neural(write)
    }

    fn cancel(&self) {
        self.cancel_pending_requests();
    }
}

/// The answer to request number `.0`, a write when `.1`.
struct Answer(u64, bool, Result<Neural, String>);

/// The Neural Rendering of the game page that opened it.
#[derive(Default)]
pub struct NeuralPanel {
    open: Option<Open>,
    /// The game (PC, library app id) whose panel could not open, and why.
    failed: Option<((u64, u32), String)>,
}

pub struct Open {
    pub host: u64,
    /// The game's library app id, which the page knows it by.
    pub game: u32,
    /// Its `install_id`: the app's id on the PC, which the calls name.
    pub app: String,
    source: Arc<dyn NeuralSource>,
    waker: Waker,
    /// The last answer, kept while the next one loads.
    pub state: Option<Neural>,
    pub loading: bool,
    pub writing: bool,
    ask: u64,
    /// Why the last read or write failed, in the PC's words.
    pub error: Option<String>,
    /// The error is a refused write's: it stays through the read that shows the game after it.
    refused: bool,
    answers: Receiver<Answer>,
    sender: Sender<Answer>,
}

impl Drop for Open {
    fn drop(&mut self) {
        self.source.cancel();
    }
}

/// Command::Neural: opens the panel of `game` on `host` with a client of its own, since closing it
/// cancels that client's requests for good.
pub fn start(
    panel: &mut NeuralPanel,
    identity: Option<&Arc<ClientIdentity>>,
    host: &Host,
    game: u32,
    app: String,
    waker: Waker,
) {
    let client = match identity {
        Some(identity) => client_for(identity, &host.record, host.connect_address()),
        None => Err(
            tr("Nightjar has no client identity yet. Restart Nightjar and try again.").to_owned(),
        ),
    };
    match client {
        Ok(client) => panel.open(host.id, game, app, Arc::new(client), waker),
        Err(error) => panel.failed = Some(((host.id, game), error)),
    }
}

impl NeuralPanel {
    pub fn open(
        &mut self,
        host: u64,
        game: u32,
        app: String,
        source: Arc<dyn NeuralSource>,
        waker: Waker,
    ) {
        let (sender, answers) = channel();
        self.failed = None;
        self.open = Some(Open {
            host,
            game,
            app,
            source,
            waker,
            state: None,
            loading: false,
            writing: false,
            ask: 0,
            error: None,
            refused: false,
            answers,
            sender,
        });
        self.read(Duration::ZERO);
    }

    /// Whether the page should ask for the panel of `game` on `host`: it is neither open nor
    /// failed for it.
    pub fn wanted(&self, host: u64, game: u32) -> bool {
        self.state_for(host, game).is_none() && self.failure_for(host, game).is_none()
    }

    pub fn state_for(&self, host: u64, game: u32) -> Option<&Open> {
        self.open
            .as_ref()
            .filter(|open| open.host == host && open.game == game)
    }

    pub fn failure_for(&self, host: u64, game: u32) -> Option<&str> {
        self.failed
            .as_ref()
            .filter(|(key, _)| *key == (host, game))
            .map(|(_, e)| e.as_str())
    }

    /// Reads the game's state again after `delay`; a later read replaces it.
    fn read(&mut self, delay: Duration) {
        let Some(open) = &mut self.open else { return };
        open.ask += 1;
        open.loading = true;
        let (ask, source, sender, waker, app) = (
            open.ask,
            open.source.clone(),
            open.sender.clone(),
            open.waker.clone(),
            open.app.clone(),
        );
        let started = spawn("neural read", move || {
            if !delay.is_zero() {
                std::thread::sleep(delay);
            }
            let _ = sender.send(Answer(ask, false, source.neural(&app).map_err(|e| said(&e))));
            waker();
        });
        if let Err(e) = started {
            open.loading = false;
            open.error = Some(e);
        }
    }

    /// Turns the game's frame generation on or off; `accept_notice` once the player accepted the
    /// notice just now. A second write while one is on its way does nothing.
    pub fn write(&mut self, host: u64, game: u32, on: bool, accept_notice: bool) {
        let Some(open) = self
            .open
            .as_mut()
            .filter(|open| open.host == host && open.game == game)
        else {
            return;
        };
        if open.writing {
            return;
        }
        open.writing = true;
        open.error = None;
        open.ask += 1;
        let write = NeuralWrite {
            app: open.app.clone(),
            switch: FRAME_GENERATION.into(),
            on,
            accept_notice,
        };
        let (ask, source, sender, waker) = (
            open.ask,
            open.source.clone(),
            open.sender.clone(),
            open.waker.clone(),
        );
        let started = spawn("neural write", move || {
            let _ = sender.send(Answer(ask, true, source.write(&write).map_err(|e| said(&e))));
            waker();
        });
        if let Err(e) = started {
            open.writing = false;
            open.error = Some(e);
        }
    }

    /// Takes in what the workers found. `page` is the game whose page is open: leaving it closes
    /// the panel, which cancels what is running.
    pub fn poll(&mut self, page: Option<(u64, u32)>) {
        if self.failed.as_ref().is_some_and(|(key, _)| page != Some(*key)) {
            self.failed = None;
        }
        if self
            .open
            .as_ref()
            .is_some_and(|open| page != Some((open.host, open.game)))
        {
            self.open = None;
            return;
        }
        let Some(open) = self.open.as_mut() else {
            return;
        };
        // The next read's delay, from the answer that asks for it.
        let mut again = None;
        while let Ok(Answer(ask, wrote, result)) = open.answers.try_recv() {
            if ask != open.ask {
                continue;
            }
            open.loading = false;
            open.writing = false;
            match result {
                Ok(state) => {
                    // A good answer ends an earlier failure, except a refusal's words, which
                    // stay through the read that follows it.
                    if !std::mem::take(&mut open.refused) {
                        open.error = None;
                    }
                    again = state.working().then_some(POLL);
                    open.state = Some(state);
                }
                Err(error) => {
                    open.error = Some(error);
                    open.refused = wrote;
                    // A refused write may still have changed something: show it as it is now.
                    // A failed read waits for the player's next visit or press instead.
                    again = wrote.then_some(Duration::ZERO);
                }
            }
        }
        if let Some(delay) = again {
            self.read(delay);
        }
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

/// The PC's own words for a refusal, else what failed.
fn said(error: &Error) -> String {
    match error {
        Error::Api { error, .. } if !error.message.is_empty() => error.message.clone(),
        other => other.to_string(),
    }
}

/// The switch's line: why it can't change now, or whether it is on.
pub fn switch_text(switch: &NeuralSwitch) -> String {
    match switch.reason.as_deref().filter(|_| !configurable(switch)) {
        Some(reason) => reason.to_owned(),
        None if switch.on => tr("On").to_owned(),
        None => tr("Off").to_owned(),
    }
}

pub fn configurable(switch: &NeuralSwitch) -> bool {
    switch.state == nightjar_core::http::native_api::radeon::ControlState::Configurable
}

/// What the PC is doing for the game, or why it last failed; None when there is nothing to say.
pub fn job_text(state: &Neural) -> Option<String> {
    let job = state.job.as_ref()?;
    Some(match job.phase {
        NeuralPhase::Download if job.total > 0 => trf(
            "Downloading: {} of {} MB",
            &[
                &format!("{:.1}", job.done as f64 / 1e6),
                &format!("{:.1}", job.total as f64 / 1e6),
            ],
        ),
        NeuralPhase::Download => tr("Downloading…").to_owned(),
        NeuralPhase::Place => tr("Copying into the game’s folder…").to_owned(),
        NeuralPhase::Remove => tr("Removing from the game’s folder…").to_owned(),
        NeuralPhase::Failed => job
            .error
            .clone()
            .filter(|e| !e.is_empty())
            .unwrap_or_else(|| tr("The last change failed.").to_owned()),
        NeuralPhase::Done | NeuralPhase::Unknown => return None,
    })
}

#[cfg(test)]
pub(crate) mod tests;
