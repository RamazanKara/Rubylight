//! `nightjar wake-relay`: this device as an away wake relay for the paired PCs that offer one
//! (`docs/internet-play.md`, "Away wake relay"). It runs in the foreground without a window and
//! prints a short line for each change: the PCs it serves and their devices, its public address
//! and NAT, STUN and DHT trouble, and each wake. Ctrl-C or SIGTERM stops it and keeps the
//! enrollments; `--leave` leaves every PC and deletes the saved file; `--enroll` enrolls again
//! with the PCs that refused it before.
//!
//! [`nightjar_core::wake_relay::driver`] decides what to ask and what to keep; this does the
//! asking (only at the PC's addresses on this network, where it takes an enrollment), lists the
//! interfaces the magic packets leave from, and prints. It never prints a secret, and nothing
//! runs unless the user starts it.

use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[cfg(test)]
use std::net::Ipv4Addr;

#[cfg(test)]
use nightjar_core::http::Error;
#[cfg(test)]
use nightjar_core::http::native_api::error::ErrorCode;
use nightjar_core::http::ClientIdentity;
use nightjar_core::wake_relay::driver::{
    self, Driver, Nat, Note, RelayStatus, Status, UdpLan, WakeRelayPeer,
};
use nightjar_core::wake_relay::saved;

use crate::config::{HostRecord, Store};
pub(crate) use crate::hosts::wake_relay_worker::{
    LeaveStep, ask, display_name, interfaces, leave_saved, paired, try_acquire_process_lock,
};
#[cfg(test)]
pub(crate) use crate::hosts::wake_relay_worker::{interface_of, left_already, on_this_network};

/// `nightjar wake-relay`'s options.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct WakeRelay {
    /// `--name`: how the PCs list this relay; the device's own name without it.
    pub name: Option<String>,
    /// `--enroll`: enroll again with the PCs that refused this relay before.
    pub enroll: bool,
    /// `--leave`: leave every PC and delete the saved assignments.
    pub leave: bool,
}

/// `wake-relay`'s options from the command line's (`--name` and the flags given).
pub(super) fn options(name: Option<String>, flags: &[String]) -> Result<WakeRelay, String> {
    let relay = WakeRelay {
        name,
        enroll: flags.iter().any(|flag| flag == "--enroll"),
        leave: flags.iter().any(|flag| flag == "--leave"),
    };
    if relay.leave && (relay.enroll || relay.name.is_some()) {
        return Err("wake-relay --leave takes no other option".into());
    }
    Ok(relay)
}

/// Set by SIGINT or SIGTERM.
static STOP: AtomicBool = AtomicBool::new(false);

// OS intent: SIGINT and SIGTERM (`systemctl stop`) end the relay between two steps on Linux and
// macOS; elsewhere Ctrl-C ends the process, and the saved file already holds every change.
#[cfg(unix)]
fn catch_stop() {
    extern "C" fn on_signal(_: libc::c_int) {
        STOP.store(true, Ordering::SeqCst);
    }
    let handler = on_signal as extern "C" fn(libc::c_int) as *const () as libc::sighandler_t;
    // SAFETY: the handler only stores to an atomic, which is async-signal-safe.
    unsafe {
        libc::signal(libc::SIGINT, handler);
        libc::signal(libc::SIGTERM, handler);
    }
}

// OS intent: a platform without Unix signals (Windows) keeps its default Ctrl-C, which ends the
// process at once.
#[cfg(not(unix))]
fn catch_stop() {}

fn stopped() -> bool {
    STOP.load(Ordering::SeqCst)
}

/// A line on stdout; a reader that is gone is no reason to stop relaying.
fn say(line: &str) {
    let _ = writeln!(std::io::stdout(), "{line}");
}

/// A PC's note as a line.
fn note_line(name: &str, note: &Note) -> String {
    match note {
        Note::Enrolled { devices } => format!("{name}: enrolled; {}", devices_text(*devices)),
        Note::Changed { devices, epoch } => {
            format!("{name}: updated (keys {epoch}); {}", devices_text(*devices))
        }
        Note::Back => format!("{name}: answers again"),
        Note::Unreachable(why) => format!("{name}: not reachable, enrollment kept ({why})"),
        Note::NotOffered(why) => {
            format!("{name}: no wake relay offered now, not served until it is: {why}")
        }
        Note::Refused(why) => format!(
            "{name}: refused this relay, which no longer serves it ({why}); `nightjar wake-relay --enroll` enrolls again"
        ),
        Note::Failed(why) => format!("{name}: {why}; enrollment kept, trying again later"),
        Note::Forgotten => format!("{name}: no longer paired here, no longer served"),
    }
}

fn devices_text(devices: usize) -> String {
    match devices {
        0 => "no device may wake it yet".into(),
        1 => "1 device may wake it".into(),
        n => format!("{n} devices may wake it"),
    }
}

fn nat_text(nat: Nat) -> &'static str {
    match nat {
        Nat::Unknown => "NAT unknown",
        Nat::None => "no NAT",
        Nat::Cone => "cone NAT",
        Nat::Symmetric => "symmetric NAT: only devices behind a cone NAT can reach this relay",
    }
}

fn wake_text(status: Status) -> &'static str {
    match status {
        Status::PacketSent => "magic packets sent",
        Status::WaitingForBoot => "waiting for the PC",
        Status::HostAwake => "the PC is awake",
        Status::TimedOut => "the PC did not answer in time",
        Status::RateLimited => "refused: too many wakes, the device has to wait",
        Status::RejectedAuth => "refused: the device may not wake it, or its keys are old",
        Status::RejectedReplay => "refused: a request seen before, or a clock that is off",
        Status::TargetNotOnLan => "refused: the PC is not on this network",
    }
}

/// `unix` as `HH:MM:SS UTC`.
fn clock(unix: u64) -> String {
    let day = unix % 86_400;
    format!(
        "{:02}:{:02}:{:02} UTC",
        day / 3600,
        day % 3600 / 60,
        day % 60
    )
}

/// What changed in the relay's status, a line each.
fn status_lines(old: &RelayStatus, new: &RelayStatus) -> Vec<String> {
    let mut lines = Vec::new();
    if (new.public != old.public || new.nat != old.nat) && !new.public.is_empty() {
        let public: Vec<String> = new.public.iter().map(ToString::to_string).collect();
        lines.push(format!(
            "public address {} ({})",
            public.join(", "),
            nat_text(new.nat)
        ));
    }
    if new.stun_error != old.stun_error
        && let Some(error) = &new.stun_error
    {
        lines.push(format!("STUN: {error}"));
    }
    if new.dht_error != old.dht_error {
        match &new.dht_error {
            Some(error) => lines.push(format!("DHT: {error}")),
            None => lines.push("DHT: publishing again".into()),
        }
    }
    if (new.targets, new.devices) != (old.targets, old.devices) {
        lines.push(format!(
            "serving {} PC(s) for {} device(s)",
            new.targets, new.devices
        ));
    }
    if new.published != old.published && new.published > 0 {
        lines.push(format!(
            "published for {} of {} device(s)",
            new.published, new.devices
        ));
    }
    if new.last_wake != old.last_wake
        && let Some((pc, at, status)) = &new.last_wake
    {
        lines.push(format!(
            "{} wake for {pc}: {}",
            clock(*at),
            wake_text(*status)
        ));
    }
    lines
}

fn identity(store: &Store) -> Result<Arc<ClientIdentity>, String> {
    match nightjar_core::hosts::IdentityStore::open(store.dir())
        .with_read_only(store.is_read_only())
        .load_existing_client()
    {
        Ok(Some(identity)) => Ok(Arc::new(identity)),
        Ok(None) => Err("this device has no pairing key yet; pair the PC in Nightjar first".into()),
        Err(e) => Err(format!(
            "this device's pairing key cannot be read ({e}); open Nightjar once"
        )),
    }
}

/// Runs `nightjar wake-relay`; its exit code.
pub fn run(relay: &WakeRelay, store: &Store) -> i32 {
    let fail = |why: String| {
        eprintln!("nightjar: wake-relay: {why}");
        1
    };
    let _lock = match try_acquire_process_lock(store.dir()) {
        Ok(Some(lock)) => lock,
        Ok(None) => {
            return fail(
                "another Nightjar wake relay is running in this settings folder; turn off \"Act as a LAN wake relay\" in Nightjar’s settings first".into(),
            );
        }
        Err(e) => return fail(e.to_string()),
    };
    let saved = match saved::load(store.dir()) {
        Ok(saved) => saved,
        Err(e) => return fail(e.to_string()),
    };
    let identity = match identity(store) {
        Ok(identity) => identity,
        Err(why) => return fail(why),
    };
    if relay.leave {
        return leave(&saved, &identity, store);
    }
    let mut driver = Driver::new(saved, relay.enroll);
    let lan: Vec<String> = interfaces()
        .iter()
        .map(|i| format!("{}/{}", i.ip, i.prefix))
        .collect();
    if lan.is_empty() {
        return fail("this device has no IPv4 network to send magic packets on".into());
    }
    let peer = match WakeRelayPeer::start(driver::relay_config(), Arc::new(UdpLan::new(interfaces)))
    {
        Ok(peer) => peer,
        Err(e) => return fail(format!("the relay's socket cannot open: {e}")),
    };
    catch_stop();
    say(&format!(
        "Nightjar wake relay on {}; Ctrl-C stops it and keeps the enrollments",
        lan.join(", ")
    ));
    let mut records = paired(store);
    let listed = |records: &[HostRecord]| -> Vec<(String, String)> {
        records
            .iter()
            .map(|record| (record.unique_id.clone(), display_name(record)))
            .collect()
    };
    for (name, note) in driver.set_hosts(&listed(&records)) {
        say(&note_line(&name, &note));
    }
    for host in driver.hosts() {
        match (host.devices, &host.refused) {
            (Some(devices), _) => say(&format!(
                "{}: enrolled before; {}",
                host.name,
                devices_text(devices)
            )),
            (None, Some(why)) => say(&note_line(&host.name, &Note::Refused(why.clone()))),
            (None, None) => {
                if let Some(why) = &host.off {
                    say(&note_line(&host.name, &Note::NotOffered(why.clone())));
                }
            }
        }
    }
    if records.is_empty() {
        say("no paired PC here yet: pair one in Nightjar, then start the relay again");
    }
    peer.set_enrollments(driver.enrollments());
    let mut shown = RelayStatus::default();
    while !stopped() {
        if !driver.due(driver::unix_now()).is_empty() {
            records = paired(store);
            for (name, note) in driver.set_hosts(&listed(&records)) {
                say(&note_line(&name, &note));
            }
            for (id, asking) in driver.due(driver::unix_now()) {
                let Some(record) = records.iter().find(|record| record.unique_id == id) else {
                    continue;
                };
                if stopped() {
                    break;
                }
                let outcome = ask(&identity, record, asking, relay.name.as_deref());
                if let Some(note) = driver.on_result(&id, outcome, driver::unix_now()) {
                    say(&note_line(&display_name(record), &note));
                }
            }
        }
        if driver.take_changed() {
            peer.set_enrollments(driver.enrollments());
            if let Err(e) = saved::save(store.dir(), &driver.saved()) {
                eprintln!(
                    "nightjar: wake-relay: {} cannot be saved: {e}",
                    saved::FILE_NAME
                );
            }
        }
        let status = peer.status();
        for line in status_lines(&shown, &status) {
            say(&line);
        }
        shown = status;
        std::thread::sleep(Duration::from_millis(500));
    }
    peer.stop();
    say("stopped; the PCs keep this relay enrolled");
    0
}

/// `--leave`: leaves every PC this relay is enrolled with, then deletes the saved file, which
/// holds the secrets. A PC that cannot be told keeps listing the relay until it is removed in
/// its console; the exit code is 1 then.
fn leave(saved: &saved::Saved, identity: &Arc<ClientIdentity>, store: &Store) -> i32 {
    let report = leave_saved(saved, identity, store.dir(), store.is_read_only());
    for step in &report.steps {
        match step {
            LeaveStep::Left { name } => say(&format!("{name}: left")),
            LeaveStep::AlreadyGone { name } => say(&format!("{name}: this relay was not enrolled there any more")),
            LeaveStep::NotPaired { name } => say(&format!(
                "{name}: not paired here any more; remove this relay in the PC's console"
            )),
            LeaveStep::Failed { name, why } => say(&format!(
                "{name}: could not leave ({why}); remove this relay in the PC's console"
            )),
        }
    }
    match &report.remove_error {
        None => say(&format!("{} deleted", saved::FILE_NAME)),
        Some(e) => eprintln!("nightjar: wake-relay: {} cannot be deleted: {e}", saved::FILE_NAME),
    }
    i32::from(report.had_failures)
}

#[cfg(test)]
mod tests;
