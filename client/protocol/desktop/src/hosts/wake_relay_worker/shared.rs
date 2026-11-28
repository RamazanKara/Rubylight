//! Shared LAN wake-relay helpers used by both `nightjar wake-relay` CLI and the in-process
//! GUI wake-relay worker (`hosts/wake_relay_worker.rs`): network interfaces, LAN host address
//! filtering, relay lock (`wake-relay.lock`), enrollment RPC, and shared Leave logic.

use std::io;
use std::net::{IpAddr, Ipv4Addr};
use std::path::Path;
use std::sync::Arc;

use nightjar_core::http::native_api::error::ErrorCode;
use nightjar_core::http::native_api::wake_relay::RelayEnroll;
use nightjar_core::http::{ClientIdentity, Error};
use nightjar_core::wake_relay::driver::{self, Ask, Interface, Outcome, is_lan};
use nightjar_core::wake_relay::saved;
use nightjar_desktop_config::config::{HostRecord, Store};

/// Advisory file lock guarding `wake-relay.json` across GUI Nightjar and `nightjar wake-relay`.
pub(crate) struct RelayProcessLock(
    #[allow(dead_code, reason = "holds OS file lock while alive")] std::fs::File,
);

pub(crate) fn try_acquire_process_lock(dir: &Path) -> io::Result<Option<RelayProcessLock>> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join("wake-relay.lock");
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true);
    // OS intent: restrict wake-relay lock file to owner on POSIX systems.
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(RelayProcessLock(file))),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(error)) => Err(error),
    }
}

/// This device's IPv4 interfaces that are up, without loopback, for magic packets.
pub(crate) fn interfaces() -> Vec<Interface> {
    match if_addrs::get_if_addrs() {
        Ok(list) => list
            .iter()
            .filter(|iface| iface.is_oper_up() && !iface.is_loopback())
            .filter_map(|iface| match &iface.addr {
                if_addrs::IfAddr::V4(v4) => interface_of(v4.ip, v4.netmask),
                if_addrs::IfAddr::V6(_) => None,
            })
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// An interface from its address and netmask; `None` for loopback, unspecified, or non-prefix masks.
pub(crate) fn interface_of(ip: Ipv4Addr, netmask: Ipv4Addr) -> Option<Interface> {
    let mask = u32::from(netmask);
    let prefix = mask.leading_ones();
    if ip.is_loopback() || ip.is_unspecified() || mask.checked_shl(prefix).unwrap_or(0) != 0 {
        return None;
    }
    Some(Interface {
        ip,
        prefix: u8::try_from(prefix).ok()?,
    })
}

/// Whether the relay asks a PC at `address`: a LAN IPv4 address or a hostname.
pub(crate) fn on_this_network(address: &str) -> bool {
    let trimmed = address.trim();
    if trimmed.is_empty() {
        return false;
    }
    let unbracketed = trimmed
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(trimmed);
    let (candidate, stripped_zone) = match unbracketed.split_once('%') {
        Some((host, _)) if host.contains(':') => (host, true),
        _ => (unbracketed, false),
    };
    match candidate.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) if !stripped_zone => is_lan(ip),
        Ok(IpAddr::V4(_)) | Ok(IpAddr::V6(_)) => false,
        Err(_) => true,
    }
}

/// The PC's addresses the relay asks it at, in the order a poll tries them.
fn addresses(record: &HostRecord) -> Vec<String> {
    let shared = record.to_shared_fields();
    let reply = nightjar_core::hosts::poll::parse_internet_reply(record.internet_play.as_deref());
    crate::hosts::poll_addresses(record)
        .into_iter()
        .filter(|address| on_this_network(address))
        .filter(|address| {
            !nightjar_core::hosts::poll::away_address(&shared, reply.as_ref(), address)
        })
        .collect()
}

pub(crate) fn display_name(record: &HostRecord) -> String {
    record
        .alias
        .clone()
        .filter(|alias| !alias.trim().is_empty())
        .unwrap_or_else(|| record.name.clone())
}

/// Saved paired PCs in `dir`.
pub(crate) fn paired_in(dir: &Path, read_only: bool) -> Vec<HostRecord> {
    nightjar_core::hosts::HostStore::open(dir)
        .with_read_only(read_only)
        .load_records::<HostRecord, _>(crate::config::keep_readable_settings_value)
        .into_iter()
        .filter(|record| record.server_certificate.is_some() && !record.unique_id.is_empty())
        .collect()
}

pub(crate) fn paired(store: &Store) -> Vec<HostRecord> {
    paired_in(store.dir(), store.is_read_only())
}

/// Asks the PC; the first address that answers at all decides.
pub(crate) fn ask(
    identity: &Arc<ClientIdentity>,
    record: &HostRecord,
    asking: Ask,
    name: Option<&str>,
) -> Outcome {
    let mut last = Outcome::Unreachable("no address of the PC on this network".into());
    for address in addresses(record) {
        let client = match crate::hosts::client_for(identity, record, &address) {
            Ok(client) => client,
            Err(why) => {
                last = Outcome::Unreachable(why);
                continue;
            }
        };
        if asking == Ask::Enroll {
            match driver::offered(client.native_capabilities()) {
                Ok(()) => {}
                Err(outcome @ Outcome::Unreachable(_)) => {
                    last = outcome;
                    continue;
                }
                Err(outcome) => return outcome,
            }
        }
        match driver::outcome(client.enroll_wake_relay(&asking.request(name))) {
            outcome @ Outcome::Unreachable(_) => last = outcome,
            outcome => return outcome,
        }
    }
    last
}

/// Whether a `leave` failed only because the PC no longer has this relay.
pub(crate) fn left_already(error: &Error) -> bool {
    match error {
        Error::Api { status, error } => *status == 404 || error.code == ErrorCode::NotFound,
        Error::NotFound(_) | Error::Status { code: 404, .. } => true,
        _ => false,
    }
}

/// Leaves one PC: whether it had this relay enrolled, or why it could not be told.
fn leave_one(identity: &Arc<ClientIdentity>, record: &HostRecord) -> Result<bool, String> {
    let mut last = Err("no address of the PC on this network".to_owned());
    for address in addresses(record) {
        let client = match crate::hosts::client_for(identity, record, &address) {
            Ok(client) => client,
            Err(why) => {
                last = Err(why);
                continue;
            }
        };
        let error = match client.enroll_wake_relay(&RelayEnroll::Leave) {
            Ok(_) => return Ok(true),
            Err(error) => error,
        };
        if left_already(&error) {
            return Ok(false);
        }
        match driver::of_error(&error) {
            Outcome::Unreachable(why) => last = Err(why),
            Outcome::Refused(why) | Outcome::Failed(why) | Outcome::NotOffered(why) => {
                return Err(why);
            }
            Outcome::Assigned(_) => return Err("the PC answered with an assignment".into()),
        }
    }
    last
}

pub(crate) enum LeaveStep {
    Left { name: String },
    AlreadyGone { name: String },
    NotPaired { name: String },
    Failed { name: String, why: String },
}

pub(crate) struct LeaveReport {
    pub(crate) steps: Vec<LeaveStep>,
    pub(crate) remove_error: Option<String>,
    pub(crate) had_failures: bool,
}

/// Shared leave routine used by both `nightjar wake-relay --leave` and GUI Leave.
pub(crate) fn leave_saved(
    saved: &saved::Saved,
    identity: &Arc<ClientIdentity>,
    dir: &Path,
    read_only: bool,
) -> LeaveReport {
    let records = paired_in(dir, read_only);
    let mut steps = Vec::new();
    let mut had_failures = false;
    for host in saved.hosts.iter().filter(|host| host.assignment.is_some()) {
        let Some(record) = records.iter().find(|record| record.unique_id == host.host) else {
            had_failures = true;
            steps.push(LeaveStep::NotPaired {
                name: host.name.clone(),
            });
            continue;
        };
        let name = display_name(record);
        match leave_one(identity, record) {
            Ok(true) => steps.push(LeaveStep::Left { name }),
            Ok(false) => steps.push(LeaveStep::AlreadyGone { name }),
            Err(why) => {
                had_failures = true;
                steps.push(LeaveStep::Failed { name, why });
            }
        }
    }
    let remove_error = match saved::remove(dir) {
        Ok(()) => None,
        Err(e) => {
            had_failures = true;
            Some(e.to_string())
        }
    };
    LeaveReport {
        steps,
        remove_error,
        had_failures,
    }
}
