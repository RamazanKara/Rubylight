//! Waking a PC, putting it to sleep, shutting it down or restarting it, and installing its update.

use super::*;

pub enum SleepStep {
    /// The PC offers to sleep: confirm, then spend `token`, which the PC handed out at `at`.
    Offer { token: String, at: Handed },
    /// The PC took the request (202). It does not confirm that the PC suspended.
    Accepted,
}

pub enum PcPowerStep {
    /// The PC offers it: confirm, then spend `token`. `wake_from_off`: a wired wake target is
    /// ready, so the PC can likely be woken after a shutdown.
    Offer { token: String, at: Handed, wake_from_off: bool },
    /// The PC took the request (202).
    Accepted,
    /// After the 202, Windows did not confirm it in time: the PC may still go down. The host's words.
    Unconfirmed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateStep {
    /// The host is installing `version`.
    Started(String),
    /// The install waits for another device's stream, in the host's words.
    Waiting(String),
    /// The PC answers with `version` again.
    Back(String),
}

/// How long an update may take before Nightjar stops waiting for the PC to come back. Time
/// spent waiting for another device's stream does not count.
const UPDATE_WAIT: Duration = Duration::from_secs(15 * 60);
/// The whole wait, another device's stream included: long enough for a film or a long session
/// on the TV to end and the update to go in, short enough that a forgotten wait does not poll the
/// PC for ever. Android's HostUpdateFlow.MAX_WAIT_MS matches.
pub(super) const UPDATE_MAX_WAIT: Duration = Duration::from_secs(3 * 60 * 60);
const UPDATE_POLL: Duration = Duration::from_secs(5);
/// After the PC took a restart or shutdown, how often and how many times Nightjar reads
/// `GET /power` for a refusal that came after the answer.
const POWER_REFUSAL_POLL: Duration = Duration::from_secs(2);
const POWER_REFUSAL_READS: u32 = 10;
pub(super) const UPDATE_GONE: &str = en("The PC has no update to install.");
pub(super) const UPDATE_FAILED: &str = en("The update failed on the PC.");

/// The calls the update wait makes; a fake PC in the tests.
pub(super) trait UpdateHost {
    fn host_update(&self) -> Result<HostUpdate, nightjar_core::http::Error>;
    /// The host version `/serverinfo` answers with, None while the PC does not answer.
    fn running_version(&self) -> Option<String>;
}

impl UpdateHost for HostClient {
    fn host_update(&self) -> Result<HostUpdate, nightjar_core::http::Error> {
        HostClient::host_update(self)
    }
    fn running_version(&self) -> Option<String> {
        self.server_info(false).ok().and_then(|info| info.capabilities.host_version().map(str::to_owned))
    }
}

/// The version an accepted install brings, from its 202 answer; none means there is nothing to
/// wait for.
pub(super) fn update_target(update: &HostUpdate) -> Result<String, String> {
    update.offered().map(str::to_owned).ok_or_else(|| UPDATE_GONE.to_owned())
}

/// Follows an accepted install until the PC is back on `target`: the updater's failure as it
/// says it, a PC back on another version, a wait for another device's stream (`said`, once
/// per reason), or `polls` reads without either, and `most` reads in all. `pause` sleeps between
/// reads.
pub(super) fn follow_update(
    host: &impl UpdateHost,
    target: &str,
    polls: u32,
    most: u32,
    pause: impl Fn(),
    mut said: impl FnMut(UpdateStep),
) -> Result<UpdateStep, String> {
    let (mut counted, mut gone, mut waiting) = (0, false, None::<String>);
    for _ in 0..most {
        if counted >= polls {
            break;
        }
        pause();
        if host.running_version().is_some_and(|v| nightjar_core::http::update_arrived(&v, target)) {
            return Ok(UpdateStep::Back(target.to_owned()));
        }
        let update = match host.host_update() {
            Ok(update) => update,
            Err(_) => {
                gone = true;
                counted += 1;
                continue;
            }
        };
        if let Some(error) = update.failure() {
            return Err(error.unwrap_or(UPDATE_FAILED).to_owned());
        }
        // Back after going away, on another version and with nothing under way: setup gave up.
        if gone && update.gave_up(target) {
            let back = trf("The PC came back on version {}; the update did not install.", &[&update.current_version]);
            return Err(update.error.map_or(back.clone(), |e| format!("{back} {e}")));
        }
        if update.waiting_for != waiting {
            waiting = update.waiting_for.clone();
            if let Some(reason) = &waiting {
                said(UpdateStep::Waiting(reason.clone()));
            }
        }
        if waiting.is_none() {
            counted += 1;
        }
    }
    Err(trf("The PC did not come back with version {} in time.", &[&target]))
}

/// The native calls Restart PC and Shut down PC make; a fake PC in the tests.
pub(super) trait PcPowerHost {
    fn pc_power(&self) -> Result<PcPowerOffer, nightjar_core::http::Error>;
    fn power_pc(&self, action: PcPower, token: &str) -> Result<(), nightjar_core::http::Error>;
}

impl PcPowerHost for HostClient {
    fn pc_power(&self) -> Result<PcPowerOffer, nightjar_core::http::Error> {
        HostClient::pc_power(self)
    }
    fn power_pc(&self, action: PcPower, token: &str) -> Result<(), nightjar_core::http::Error> {
        HostClient::power_pc(self, action, token)
    }
}

/// A refusal in the host's own words; anything else said like the other host errors.
pub(super) fn native_error(error: &nightjar_core::http::Error) -> String {
    nightjar_core::http::host_words(error).unwrap_or_else(|| describe(error))
}

/// Asks the PC for a token for `action`, timed as [`sleep_token`].
pub(super) fn pc_power_token(host: &impl PcPowerHost, action: PcPower) -> Result<PcPowerStep, String> {
    let at = Handed::now();
    let offer = host.pc_power().map_err(|e| native_error(&e))?;
    match offer.token_for(action) {
        Ok(token) => Ok(PcPowerStep::Offer { token: token.to_owned(), at, wake_from_off: offer.wake_from_off }),
        Err(reason) if reason.is_empty() => Err(PC_POWER_BUSY.into()),
        Err(reason) => Err(reason),
    }
}

/// Spends the confirmed token, asking for a new one first when it is about to expire.
pub(super) fn pc_power_confirmed(host: &impl PcPowerHost, action: PcPower, token: String, at: Handed) -> Result<PcPowerStep, String> {
    let token = match at.age() >= nightjar_core::http::POWER_TOKEN_REFRESH {
        true => match pc_power_token(host, action)? {
            PcPowerStep::Offer { token, .. } => token,
            PcPowerStep::Accepted | PcPowerStep::Unconfirmed(_) => token,
        },
        false => token,
    };
    host.power_pc(action, &token).map_err(|e| native_error(&e))?;
    Ok(PcPowerStep::Accepted)
}

/// After the PC took the request: why it did not go ahead after all (Windows refused, or the PC
/// got busy) or may not have (Windows did not confirm it in time), read from `GET /power` up to
/// `reads` times; None once the PC stops answering or never says.
pub(super) fn pc_power_refusal(host: &impl PcPowerHost, reads: u32, pause: impl Fn()) -> Option<LateRefusal> {
    for _ in 0..reads {
        pause();
        match host.pc_power() {
            Ok(offer) => {
                if let Some(refusal) = offer.late_refusal() {
                    return Some(refusal);
                }
            }
            // Going down.
            Err(_) => return None,
        }
    }
    None
}

/// When the PC handed out a sleep token, on two clocks: `Instant` stops while a Linux client is
/// suspended, the wall clock keeps running but can be set back. The larger age counts, so a
/// token never looks younger than it is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Handed {
    pub(super) at: Instant,
    pub(super) wall: SystemTime,
}

impl Handed {
    pub fn now() -> Self {
        Self { at: Instant::now(), wall: SystemTime::now() }
    }

    pub(super) fn age(&self) -> Duration {
        self.at.elapsed().max(self.wall.elapsed().unwrap_or_default())
    }
}

/// The power-action calls Sleep PC makes; a fake PC in the tests.
pub(super) trait PowerHost {
    fn power_capabilities(&self) -> Result<PowerReply, PowerError>;
    fn sleep(&self, token: &str) -> Result<PowerReply, PowerError>;
}

impl PowerHost for HostClient {
    fn power_capabilities(&self) -> Result<PowerReply, PowerError> {
        HostClient::power_capabilities(self)
    }
    fn sleep(&self, token: &str) -> Result<PowerReply, PowerError> {
        HostClient::sleep(self, token)
    }
}

/// A refusal in the host's own words; anything else said like the other host errors.
fn power_error(error: &PowerError) -> String {
    match error {
        PowerError::Refused { reply, .. } if !reply.message.is_empty() => reply.message.clone(),
        PowerError::Refused { reply, .. } => trf("The PC refused with status {}.", &[&reply.status]),
        PowerError::Host(e) => describe(e),
    }
}

/// Asks the PC for a sleep token; the time is taken before asking, so the token's age is never
/// under-counted.
pub(super) fn sleep_token(host: &impl PowerHost) -> Result<(String, Handed), String> {
    let at = Handed::now();
    let reply = host.power_capabilities().map_err(|e| power_error(&e))?;
    match reply.token {
        Some(token) => Ok((token, at)),
        None if !reply.sleep_supported => Err(SLEEP_UNSUPPORTED.into()),
        None if !reply.sleep_allowed => Err(SLEEP_NOT_ALLOWED.into()),
        // Supported and allowed but no token, whatever `sleep_available` says: busy, like Android.
        None => Err(SLEEP_BUSY.into()),
    }
}

/// Spends the confirmed token, asking for a new one first when it is about to expire.
pub(super) fn sleep_confirmed(host: &impl PowerHost, token: String, at: Handed) -> Result<SleepStep, String> {
    let token = if at.age() >= SLEEP_TOKEN_REFRESH { sleep_token(host)?.0 } else { token };
    host.sleep(&token).map_err(|e| power_error(&e))?;
    Ok(SleepStep::Accepted)
}

impl Hosts {
    /// Sends the wake packets once; `run` comes back with the result (see `wol::Wakes`).
    pub fn wake(&self, id: u64, run: u64) {
        let Some(host) = self.get(id) else { return };
        let record = host.record.clone();
        self.spawn("wake", move || Some(Event::Wake { id, run, result: crate::wol::wake(&record) }));
    }

    /// Sleep PC, first step: asks the PC for the token the confirmation spends.
    pub fn sleep_offer(&self, id: u64, ask: u64) {
        self.sleep_step(id, ask, false, |client| sleep_token(client).map(|(token, at)| SleepStep::Offer { token, at }));
    }

    /// Sleep PC, confirmed: spends `token` (handed out at `at`), or a fresh one when it is old.
    pub fn sleep(&self, id: u64, ask: u64, token: String, at: Handed) {
        self.sleep_step(id, ask, true, move |client| sleep_confirmed(client, token, at));
    }

    fn sleep_step(
        &self,
        id: u64,
        ask: u64,
        confirmed: bool,
        work: impl FnOnce(&HostClient) -> Result<SleepStep, String> + Send + 'static,
    ) {
        let Some(host) = self.get(id) else { return };
        let record = host.connect_record();
        let name = host.display_name().to_owned();
        let identity = self.identity.clone();
        self.spawn("sleep", move || {
            let result = identity
                .ok_or_else(|| en("Nightjar has no client identity yet. Restart Nightjar and try again.").to_owned())
                .and_then(|identity| client_for(&identity, &record, &record.address))
                .and_then(|client| work(&client));
            Some(Event::Sleep { id, ask, name, confirmed, result })
        });
    }

    /// Restart PC or Shut down PC, first step: asks the PC for the token the confirmation spends.
    pub fn pc_power_offer(&self, id: u64, ask: u64, action: PcPower) {
        self.pc_power_step(id, ask, action, false, move |client| pc_power_token(client, action));
    }

    /// Restart PC or Shut down PC, confirmed: spends `token`, or a fresh one when it is old.
    pub fn pc_power(&self, id: u64, ask: u64, action: PcPower, token: String, at: Handed) {
        self.pc_power_step(id, ask, action, true, move |client| pc_power_confirmed(client, action, token, at));
    }

    fn pc_power_step(
        &self,
        id: u64,
        ask: u64,
        action: PcPower,
        confirmed: bool,
        work: impl FnOnce(&HostClient) -> Result<PcPowerStep, String> + Send + 'static,
    ) {
        let Some(host) = self.get(id) else { return };
        let record = host.connect_record();
        let name = host.display_name().to_owned();
        let identity = self.identity.clone();
        let sender = self.sender.clone();
        let waker = self.waker.clone();
        self.spawn("pc-power", move || {
            let client = identity
                .ok_or_else(|| en("Nightjar has no client identity yet. Restart Nightjar and try again.").to_owned())
                .and_then(|identity| client_for(&identity, &record, &record.address));
            let result = client.as_ref().map_err(Clone::clone).and_then(work);
            let client = match (client, matches!(result, Ok(PcPowerStep::Accepted))) {
                (Ok(client), true) => client,
                _ => return Some(Event::PcPower { id, ask, name, action, confirmed, result }),
            };
            // Taken; Windows may still refuse once the answer has left.
            let _ = sender.send(Event::PcPower { id, ask, name: name.clone(), action, confirmed, result });
            waker();
            let result = match pc_power_refusal(&client, POWER_REFUSAL_READS, || std::thread::sleep(POWER_REFUSAL_POLL))? {
                LateRefusal::Refused(refusal) => Err(refusal),
                LateRefusal::Unconfirmed(said) => Ok(PcPowerStep::Unconfirmed(said)),
            };
            Some(Event::PcPower { id, ask, name, action, confirmed, result })
        });
    }

    /// Installs the PC's host update, then waits for the PC to answer with the new version.
    pub fn install_update(&self, id: u64) {
        let Some(host) = self.get(id) else { return };
        let record = host.connect_record();
        let name = host.display_name().to_owned();
        let identity = self.identity.clone();
        let sender = self.sender.clone();
        let waker = self.waker.clone();
        self.spawn("host-update", move || {
            let client = match identity
                .ok_or_else(|| en("Nightjar has no client identity yet. Restart Nightjar and try again.").to_owned())
                .and_then(|identity| client_for(&identity, &record, &record.address))
            {
                Ok(client) => client,
                Err(e) => return Some(Event::HostUpdate { id, name, result: Err(e) }),
            };
            let target = match client.install_host_update().map_err(|e| native_error(&e)).and_then(|u| update_target(&u)) {
                Ok(target) => target,
                Err(e) => return Some(Event::HostUpdate { id, name, result: Err(e) }),
            };
            let say = |step| {
                let _ = sender.send(Event::HostUpdate { id, name: name.clone(), result: Ok(step) });
                waker();
            };
            say(UpdateStep::Started(target.clone()));
            let polls = (UPDATE_WAIT.as_secs() / UPDATE_POLL.as_secs()) as u32;
            let most = (UPDATE_MAX_WAIT.as_secs() / UPDATE_POLL.as_secs()) as u32;
            let result = follow_update(&client, &target, polls, most, || std::thread::sleep(UPDATE_POLL), say);
            Some(Event::HostUpdate { id, name, result })
        });
    }
}
