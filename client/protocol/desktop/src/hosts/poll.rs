//! Host worker events (`Event`), per-PC background worker lanes, mDNS discovery, STUN lookup and polling.

use std::net::{IpAddr, Ipv4Addr};
use std::sync::mpsc::channel;
use std::sync::Arc;
use std::time::Instant;

use nightjar_core::http::native_api::session::{ActionId, Session};
use nightjar_core::http::{App, InfoPath, PcPower, ServerInfo, StreamMove};

use crate::config::HostRecord;
use super::{
    client_for, poll_error, Art, Hosts, Lane, NativeRead, PcJob, PcPowerStep, PollFailure, Reach,
    SleepStep, UpdateStep, ANOTHER_PC_ANSWERS, MDNS_SERVICE, RECHECK_WINDOW,
};

pub enum Event {
    Info {
        id: u64,
        seq: u64,
        address: String,
        /// How the answer was reached: what its pairing status is worth.
        path: InfoPath,
        result: Result<Box<ServerInfo>, String>,
    },
    Discovered {
        name: String,
        addresses: Vec<IpAddr>,
        port: u16,
    },
    /// Whether the PC `unique_id` has Foundation Sunshine's bandwidth test; None when the
    /// question did not get through.
    FoundationProbe {
        id: u64,
        unique_id: String,
        found: Option<bool>,
    },
    /// The PC's `/internet-play/v1` reply as it came, read on its network.
    InternetPlay {
        id: u64,
        unique_id: String,
        result: Result<String, String>,
    },
    /// This device's WAN address from STUN, the remote address of a PC found on the LAN.
    Remote {
        id: u64,
        result: std::io::Result<Option<Ipv4Addr>>,
    },
    Paired {
        id: u64,
        request: u64,
        result: Result<String, String>,
    },
    /// An invite opened: the PC reached and paired over the internet, or why not. `id` is the
    /// PC in the list once the answer was taken in.
    Invited {
        result: Result<Box<nightjar_core::invite::Redeemed>, String>,
        id: Option<u64>,
    },
    Apps {
        id: u64,
        result: Result<Vec<App>, String>,
    },
    Art {
        id: u64,
        app: i32,
        version: String,
        image: Option<egui::ColorImage>,
        ambient: Option<egui::ColorImage>,
        /// The pixels per point `image` was made smaller for, None when it is at its own size.
        sized: Option<f32>,
    },
    /// A cover that was given up for room has no file to come back from, and its PC is away: it
    /// is asked for again when the PC is back.
    ArtAway {
        id: u64,
        app: i32,
        version: String,
    },
    /// A cover's download failed other than with "not found": a title for now, and asked for
    /// again after [`ART_RETRY_INTERVAL`].
    ArtFailed {
        id: u64,
        app: i32,
        version: String,
    },
    /// A cover that was made smaller, loaded again for a screen that shows more of it.
    ArtSharper {
        id: u64,
        app: i32,
        version: String,
        image: egui::ColorImage,
        sized: Option<f32>,
    },
    Native {
        id: u64,
        epoch: u64,
        seq: u64,
        result: Result<Box<NativeRead>, String>,
    },
    /// Who plays the running game, read for selecting `app`; None when the PC did not say in time.
    SelectRead {
        id: u64,
        app: i32,
        resume: bool,
        /// The user pressed Stream here too on "did not move".
        too: bool,
        read: Option<StreamMove>,
    },
    StreamMoved {
        id: u64,
        app: i32,
        /// The device the stream moved to; None when it did not, or the PC could not say.
        device: Option<String>,
    },
    MoveWatched {
        /// The prompt that asked: see [`crate::move_watch`].
        token: u64,
        id: u64,
        app: i32,
        read: Result<StreamMove, String>,
    },
    ContinueHere {
        id: u64,
        app: i32,
        result: Result<(), String>,
    },
    Session {
        id: u64,
        action: ActionId,
        result: Result<Box<Session>, String>,
    },
    Quit {
        id: u64,
        result: Result<(), String>,
    },
    Wake {
        id: u64,
        run: u64,
        result: Result<(), String>,
    },
    /// A Sleep PC step: the offer to confirm, or (`confirmed`) the answer to the request. `ask`
    /// is the number the UI gave the request, so an answer to an earlier ask is told apart.
    Sleep {
        id: u64,
        ask: u64,
        /// The PC's name when it was asked, for the message about the answer.
        name: String,
        confirmed: bool,
        result: Result<SleepStep, String>,
    },
    /// A Restart PC or Shut down PC step, as [`Event::Sleep`].
    PcPower {
        id: u64,
        ask: u64,
        name: String,
        action: PcPower,
        confirmed: bool,
        result: Result<PcPowerStep, String>,
    },
    /// The PC answered an approval or a denial of a device waiting to pair.
    PairingAnswer {
        id: u64,
        request: String,
        name: String,
        denied: bool,
        result: Result<(), String>,
    },
    /// The PC's host update: `Started` once the PC took the request, then `Back` once it
    /// answers with the new version, or why either failed.
    HostUpdate {
        id: u64,
        name: String,
        result: Result<UpdateStep, String>,
    },
    /// The PC answered (or not) to being asked to drop this device. The pin is gone either way.
    Unpaired {
        id: u64,
        result: Result<(), String>,
    },
    Action {
        id: u64,
        result: Result<(), String>,
    },
    /// A game's artwork and facts record, looked up or read from disk ([`crate::game_info`]).
    GameInfo(Box<nightjar_policy::game_info::cache::Record>),
    /// Host Hub Sync pass completed (`hub/snapshot`, outbox replay, and ETag covers).
    HubSync {
        id: u64,
        epoch: u64,
        seq: u64,
        engine: Box<nightjar_policy::hub_sync::SyncEngine>,
        cover_etags: std::collections::BTreeMap<u32, String>,
        updated_covers: Vec<u32>,
    },
}

impl Hosts {
    pub(super) fn spawn(&self, name: &str, work: impl FnOnce() -> Option<Event> + Send + 'static) {
        // The demo library never reaches for a network, even by accident.
        if cfg!(feature = "demo-library") {
            return;
        }
        let sender = self.sender.clone();
        let waker = self.waker.clone();
        if let Err(e) = std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                if let Some(event) = work() {
                    let _ = sender.send(event);
                    waker();
                }
            })
        {
            eprintln!("nightjar: cannot start {name} worker: {e}");
        }
    }

    /// Runs `work` on PC `id`'s worker for `lane`, started on its first request: the lane's
    /// requests wait for each other rather than each starting a thread. False when no worker
    /// could take it.
    pub(super) fn run_on(&mut self, id: u64, lane: Lane, work: impl FnOnce() -> Option<Event> + Send + 'static) -> bool {
        // The demo library never reaches for a network, even by accident.
        if cfg!(feature = "demo-library") {
            return true;
        }
        let mut job: PcJob = Box::new(work);
        if let Some(worker) = self.pc_workers.get(&(id, lane)) {
            match worker.send(job) {
                Ok(()) => return true,
                // Its thread ended (a request panicked): a new one takes the job.
                Err(std::sync::mpsc::SendError(back)) => job = back,
            }
        }
        let (jobs, queue) = channel::<PcJob>();
        let (sender, waker) = (self.sender.clone(), self.waker.clone());
        let name = match lane {
            Lane::Asked => "pc-worker",
            Lane::Background => "pc-poller",
        };
        let started = std::thread::Builder::new().name(name.into()).spawn(move || {
            for job in queue {
                if let Some(event) = job() {
                    if sender.send(event).is_err() {
                        return;
                    }
                    waker();
                }
            }
        });
        let accepted = started.is_ok() && jobs.send(job).is_ok();
        if accepted {
            self.pc_workers.insert((id, lane), jobs);
        }
        accepted
    }

    /// Ends PC `id`'s workers, once the requests they hold are done.
    pub(super) fn drop_workers(&mut self, id: u64) {
        self.pc_workers.retain(|(pc, _), _| *pc != id);
    }

    /// Starts looking for PCs on the network again, when it could not start (a network not up yet,
    /// or its port held). True when it runs now.
    pub fn search_again(&mut self) -> bool {
        if self._mdns.is_none() {
            self._mdns = self.start_discovery();
            self.discovering = self._mdns.is_some();
        }
        self.discovering
    }

    pub(super) fn start_discovery(&self) -> Option<mdns_sd::ServiceDaemon> {
        let daemon = mdns_sd::ServiceDaemon::new().ok()?;
        let receiver = daemon.browse(MDNS_SERVICE).ok()?;
        let sender = self.sender.clone();
        let waker = self.waker.clone();
        std::thread::Builder::new()
            .name("mdns".into())
            .spawn(move || {
                while let Ok(event) = receiver.recv() {
                    if let mdns_sd::ServiceEvent::ServiceResolved(service) = event {
                        let mut addresses: Vec<IpAddr> = service
                            .addresses
                            .iter()
                            .map(|a| a.to_ip_addr())
                            .filter(|a| !a.is_loopback())
                            .collect();
                        // IPv4 first: link-local IPv6 needs a scope the HTTP client may lack.
                        addresses.sort_by_key(|a| !a.is_ipv4());
                        let name = service
                            .fullname
                            .split("._nvstream")
                            .next()
                            .unwrap_or_default()
                            .to_owned();
                        if sender
                            .send(Event::Discovered {
                                name,
                                addresses,
                                port: service.port,
                            })
                            .is_err()
                        {
                            break;
                        }
                        waker();
                    }
                }
            })
            .ok()?;
        Some(daemon)
    }

    /// Asks STUN for this device's WAN address, which reaches a PC on its LAN from outside it
    /// once the router forwards the PC's ports (`LiFindExternalAddressIP4`, as Moonlight V+ PC
    /// does for a PC found by mDNS or added by a private IPv4 address).
    pub(super) fn find_remote_address(&self, id: u64) {
        if !self.stun {
            return;
        }
        self.spawn("stun", move || {
            let result = nightjar_core::stun::find_external_ipv4(
                nightjar_core::stun::MOONLIGHT_STUN_SERVER,
                nightjar_core::stun::MOONLIGHT_STUN_PORT,
                nightjar_core::stun::STUN_TIMEOUT,
            );
            Some(Event::Remote { id, result })
        });
    }

    /// Checks every host that is due. While the window is away (`shown` false) only the PC being
    /// woken (`waking`) and those in their recheck window are: a wake waits for its PC's answer,
    /// and a quit or a stream's end for the PC's new state. The others wait for the window.
    pub fn poll(&mut self, shown: bool, waking: Option<u64>) {
        for id in self.due(Instant::now(), shown, waking) {
            self.start_poll(id, Lane::Background);
        }
    }

    /// The hosts [`Self::poll`] asks at `now`.
    pub(super) fn due(&self, now: Instant, shown: bool, waking: Option<u64>) -> Vec<u64> {
        self.list
            .iter()
            .filter(|h| shown || Some(h.id) == waking || h.recheck_until.is_some_and(|until| now < until))
            .filter(|h| !h.polling && h.background_poll.is_none())
            .filter(|h| h.last_poll.is_none_or(|t| now.saturating_duration_since(t) >= h.poll_interval(now)))
            .map(|h| h.id)
            .collect()
    }

    /// A quit, a closed game or a stream has ended: ask the PC what runs now, and keep asking
    /// every second for a while, since the PC changes its state after it has answered. Reads the
    /// native session and library too, because Home shows the running game from those.
    pub fn refresh_after_exit(&mut self, id: u64) {
        let Some(host) = self.get_mut(id) else { return };
        host.recheck_until = Some(Instant::now() + RECHECK_WINDOW);
        self.poll_one(id);
        self.refresh_native(id, true);
    }

    /// [`Self::poll_one`] for a check the user asked for, which the card shows as Checking….
    pub fn check_now(&mut self, id: u64) {
        self.poll_one(id);
        if let Some(host) = self.get_mut(id) {
            host.checking = host.polling;
        }
    }

    /// Asks the PC now, beside a background poll still out rather than behind it. While another
    /// asked poll is out, this one starts once that answers: they never queue up, and only the
    /// answer to the newest ends the PC's Checking….
    pub fn poll_one(&mut self, id: u64) {
        if self.identity.is_none() {
            return;
        }
        let Some(host) = self.get_mut(id) else {
            return;
        };
        if host.asked_poll.is_some() {
            host.polling = true;
            host.last_poll = Some(Instant::now());
            host.poll_seq += 1;
            host.asked_again = true;
            return;
        }
        self.start_poll(id, Lane::Asked);
    }

    /// Poll `seq` of PC `id` answered, and its lane is free again. True when a poll was asked for
    /// meanwhile and starts now.
    pub(super) fn poll_landed(&mut self, id: u64, seq: u64) -> bool {
        let Some(host) = self.get_mut(id) else {
            return false;
        };
        if host.background_poll == Some(seq) {
            host.background_poll = None;
        }
        if host.asked_poll != Some(seq) {
            return false;
        }
        host.asked_poll = None;
        std::mem::take(&mut host.asked_again)
    }

    pub(super) fn start_poll(&mut self, id: u64, lane: Lane) {
        let Some(identity) = self.identity.clone() else {
            return;
        };
        let Some(host) = self.get_mut(id) else {
            return;
        };
        host.polling = true;
        host.last_poll = Some(Instant::now());
        host.poll_seq += 1;
        let seq = host.poll_seq;
        match lane {
            Lane::Asked => host.asked_poll = Some(seq),
            Lane::Background => host.background_poll = Some(seq),
        }
        let record = host.record.clone();
        let likely_online = host.reach == Reach::Online;
        let away = host.reach == Reach::Offline;
        let poll = move || {
            let mut failure = PollFailure::default();
            let mut tried = Vec::new();
            for address in poll_addresses(&record) {
                let result = client_for(&identity, &record, &address)
                    .and_then(|c| c.server_info_checked(likely_online).map_err(|e| poll_error(&e)));
                match result {
                    Ok((info, path)) => {
                        return Some(Event::Info {
                            id,
                            seq,
                            address,
                            path,
                            result: Ok(Box::new(info)),
                        })
                    }
                    Err(e) => {
                        tried.push(address);
                        failure.failed(e);
                    }
                }
            }
            // Every direct path failed: a punched one, where the PC gave this device its key, once
            // the PC reads as away (`punches_now`).
            let punch = punches_now(away, || nightjar_core::internet::connected(&record.unique_id).is_some());
            if let Some(reply) = internet_reply(&record).filter(|_| record.pinned_address.is_none() && punch) {
                let asked = |address: String| tried.contains(&address);
                let tried = nightjar_core::internet::Tried {
                    lan: asked(record.address.clone()),
                    ipv6: reply.ipv6.iter().any(|ip| asked(ip.to_string())),
                    mapped: reply.external_ipv4.is_some_and(|ip| asked(ip.to_string())),
                };
                let punched = nightjar_core::internet::punch(&record.unique_id, &reply, tried).and_then(|at| {
                    let address = at.ip().to_string();
                    client_for(&identity, &record, &address)
                        .and_then(|c| c.server_info_checked(true).map_err(|e| poll_error(&e)))
                        .map(|info| (address, info))
                        .map_err(|e| {
                            failure.another_pc |= e == ANOTHER_PC_ANSWERS;
                            nightjar_core::internet::unreachable(&reply, tried, Some(e))
                        })
                });
                match punched {
                    Ok((address, (info, path))) => {
                        return Some(Event::Info { id, seq, address, path, result: Ok(Box::new(info)) })
                    }
                    Err(words) => failure.failed(words),
                }
            }
            Some(Event::Info {
                id,
                seq,
                address: record.address.clone(),
                path: InfoPath::Plain,
                result: Err(failure.reason()),
            })
        };
        let accepted = self.run_on(id, lane, poll);
        if !accepted {
            // No worker could take it (no thread to be had): the lane is free for the next poll, or
            // the PC would read Checking… and take none until Nightjar is started again.
            if let Some(host) = self.get_mut(id) {
                match lane {
                    Lane::Asked => host.asked_poll = None,
                    Lane::Background => host.background_poll = None,
                }
                host.polling = false;
                host.checking = false;
            }
        }
    }

    /// A new renderer (Linux's switch to the GPU renderer): the covers and ambients went with the
    /// old one and load again from the art cache as they are asked for ([`Self::request_art`]).
    pub fn renderer_changed(&mut self) {
        for host in &mut self.list {
            Arc::make_mut(&mut host.art).retain(|_, art| !matches!(art, Art::Ready(_)));
            Arc::make_mut(&mut host.ambient).clear();
            Arc::make_mut(&mut host.art_sized).clear();
        }
        self.art_used.clear();
    }
}

/// Whether a poll whose direct paths failed tries the punched path now.
pub(super) fn punches_now(away: bool, tunnel_up: impl FnOnce() -> bool) -> bool {
    if away {
        nightjar_policy::hosts::punches_now(false, true, false)
    } else {
        nightjar_policy::hosts::punches_now(false, false, tunnel_up())
    }
}

/// The addresses a poll tries in turn.
pub(crate) fn poll_addresses(record: &HostRecord) -> Vec<String> {
    nightjar_core::hosts::poll::desktop_poll_addresses(
        &record.to_shared_fields(),
        internet_reply(record).as_ref(),
    )
}

/// The PC's stored internet play reply, read again.
pub(super) fn internet_reply(record: &HostRecord) -> Option<nightjar_core::http::InternetPlay> {
    nightjar_core::hosts::poll::parse_internet_reply(record.internet_play.as_deref())
}

/// Whether `address` reaches the PC from outside its network.
pub(super) fn away_address(record: &HostRecord, address: &str) -> bool {
    nightjar_core::hosts::poll::away_address(
        &record.to_shared_fields(),
        internet_reply(record).as_ref(),
        address,
    )
}

/// `text` as an IPv4 address that a PC outside this network can be reached at.
pub(super) fn public_ipv4(text: &str) -> Option<String> {
    nightjar_policy::hosts::public_ipv4(text).map(|ip| ip.to_string())
}
