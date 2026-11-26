//! Controllers and USB forwarding on the desktop: a controller attached to the host over USB is
//! not also read here as a gamepad, and comes back as one as soon as the forwarding stops.
//!
//! A forwarded device holds a [`Claim`] from the moment Nightjar starts opening it until its
//! attachment ends. The device's close (detach, the stream ending, the host or the device
//! dropping the connection) releases it, and so do its drop and the end of the stream's USB
//! panel ([`Registry::release_owner`]), so a claim never outlives the forwarding; a crash ends
//! both, as the registry lives in this process only. [`Gamepads`](crate::gamepad::Gamepads)
//! looks at [`Registry::generation`] once per loop and pauses or resumes the pads [`plan`]
//! names: a pad is only ever paused while a claim matches it.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use nightjar_core::usb::{wire, Completion, Transfer, Tunnel, UsbDevice};

/// A USB device being forwarded, as the picker names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Forwarded {
    pub vid: u16,
    pub pid: u16,
    /// Linux's name for its port (`1-2.4`, [`crate::usb::busid`]).
    pub busid: String,
}

/// A local gamepad as SDL describes it before opening it: its vendor and product and, where
/// the platform tells, the busid of the USB device it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PadIdentity {
    pub vid: u16,
    pub pid: u16,
    pub busid: Option<String>,
}

#[derive(Default)]
struct Inner {
    /// (claim id, owner, device).
    claims: Mutex<Vec<(u64, u64, Forwarded)>>,
    next: AtomicU64,
    /// Bumped by every claim and release.
    generation: AtomicU64,
    /// The generation the gamepads last applied.
    applied: AtomicU64,
    /// Whether gamepads look at this registry at all (the desktop app's loops do).
    watched: AtomicBool,
}

/// The devices being forwarded now. The app uses [`Registry::global`] (the default); tests
/// make their own.
#[derive(Clone)]
pub struct Registry(Arc<Inner>);

impl Default for Registry {
    fn default() -> Self {
        Self::global()
    }
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registry").field("generation", &self.generation()).finish()
    }
}

impl Registry {
    /// A registry of its own (tests).
    pub fn new() -> Self {
        Self(Arc::default())
    }

    /// The registry the USB panel claims in and the gamepads read.
    pub fn global() -> Self {
        static GLOBAL: OnceLock<Registry> = OnceLock::new();
        GLOBAL.get_or_init(Registry::new).clone()
    }

    fn claims(&self) -> std::sync::MutexGuard<'_, Vec<(u64, u64, Forwarded)>> {
        self.0.claims.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn bump(&self) -> u64 {
        self.0.generation.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// A new owner id, for [`Self::claim`] and [`Self::release_owner`]: one per stream panel.
    pub fn owner(&self) -> u64 {
        self.0.next.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Marks `device` as forwarded by `owner` until the returned claim is released or dropped.
    pub fn claim(&self, owner: u64, device: Forwarded) -> Claim {
        let id = self.0.next.fetch_add(1, Ordering::Relaxed) + 1;
        eprintln!("nightjar: USB {}: forwarded; a controller it is pauses here", device.busid);
        self.claims().push((id, owner, device));
        let generation = self.bump();
        Claim { registry: self.clone(), id, generation, released: AtomicBool::new(false) }
    }

    fn release(&self, id: u64) {
        let removed = {
            let mut claims = self.claims();
            let before = claims.len();
            claims.retain(|(claim, _, _)| *claim != id);
            claims.len() != before
        };
        if removed { self.bump(); }
    }

    /// Releases every claim `owner` still holds: its stream ended, so none of them can still
    /// be forwarded, whatever still holds their devices.
    pub fn release_owner(&self, owner: u64) {
        let removed = {
            let mut claims = self.claims();
            let before = claims.len();
            claims.retain(|(_, by, _)| *by != owner);
            claims.len() != before
        };
        if removed {
            eprintln!("nightjar: USB: the stream's forwarding ended; its controllers come back");
            self.bump();
        }
    }

    /// Changes with every claim and release: one atomic load.
    pub fn generation(&self) -> u64 {
        self.0.generation.load(Ordering::Acquire)
    }

    /// The devices forwarded now.
    pub fn forwarded(&self) -> Vec<Forwarded> {
        self.claims().iter().map(|(_, _, device)| device.clone()).collect()
    }

    /// The gamepads applied `generation`.
    pub fn applied(&self, generation: u64) {
        self.0.watched.store(true, Ordering::Release);
        self.0.applied.fetch_max(generation, Ordering::AcqRel);
    }
}

/// A device being forwarded; released once, by [`Claim::release`] or its drop.
pub struct Claim {
    registry: Registry,
    id: u64,
    generation: u64,
    released: AtomicBool,
}

impl std::fmt::Debug for Claim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Claim").field("id", &self.id).field("released", &self.released.load(Ordering::Relaxed)).finish()
    }
}

impl Claim {
    pub fn release(&self) {
        if !self.released.swap(true, Ordering::AcqRel) {
            self.registry.release(self.id);
        }
    }

    /// Waits, at most `limit`, until the gamepads paused what this claim matches, so SDL has let
    /// go of the device before it is opened for forwarding. Returns at once when no gamepads
    /// look at the registry; whether they got to it in time.
    pub fn wait_applied(&self, limit: Duration) -> bool {
        if !self.registry.0.watched.load(Ordering::Acquire) { return true; }
        let until = Instant::now() + limit;
        while self.registry.0.applied.load(Ordering::Acquire) < self.generation {
            if Instant::now() >= until { return false; }
            std::thread::sleep(Duration::from_millis(4));
        }
        true
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        self.release();
    }
}

/// A forwarded device and its claim: closing or dropping it releases the claim, after the
/// device let go.
pub struct Arbitrated {
    device: Arc<dyn UsbDevice>,
    claim: Claim,
}

impl Arbitrated {
    pub fn new(device: Arc<dyn UsbDevice>, claim: Claim) -> Arc<dyn UsbDevice> {
        Arc::new(Self { device, claim })
    }
}

impl UsbDevice for Arbitrated {
    fn describe(&self) -> wire::Device {
        self.device.describe()
    }

    fn class(&self) -> u8 {
        self.device.class()
    }

    fn submit(&self, transfer: Transfer, done: Completion) {
        self.device.submit(transfer, done);
    }

    fn unlink(&self, seqnum: u32) {
        self.device.unlink(seqnum);
    }

    fn close(&self) {
        self.device.close();
        self.claim.release();
    }

    fn relay(&self) -> Option<std::io::Result<Arc<dyn Tunnel>>> {
        self.device.relay()
    }
}

/// Which pads to pause while `forwarded` devices are attached to the host, by position in
/// `pads` (every local gamepad, open or paused). A pad whose busid is known pauses when a
/// device of its vendor and product is forwarded from that busid. One whose busid isn't known
/// pauses only when it is the one pad of its vendor and product here and exactly one such
/// device is forwarded: a second controller of the same model never goes dark on a guess.
pub fn paused(pads: &[PadIdentity], forwarded: &[Forwarded]) -> Vec<bool> {
    let same = |pad: &PadIdentity, device: &Forwarded| (pad.vid, pad.pid) == (device.vid, device.pid);
    pads.iter()
        .map(|pad| match &pad.busid {
            Some(busid) => forwarded.iter().any(|device| same(pad, device) && device.busid == *busid),
            None => {
                let model = |p: &&PadIdentity| (p.vid, p.pid) == (pad.vid, pad.pid);
                let models_here = pads.iter().filter(model).count();
                let models_forwarded = forwarded.iter().filter(|device| same(pad, device)).count();
                // A busid-known pad of this model that is forwarded accounts for the claim.
                let accounted = pads.iter().filter(model).any(|p| p.busid.as_ref().is_some_and(|b| forwarded.iter().any(|d| same(p, d) && d.busid == *b)));
                (pad.vid, pad.pid) != (0, 0) && models_here == 1 && models_forwarded == 1 && !accounted
            }
        })
        .collect()
}

/// What the gamepads do after the forwarded devices changed: the open pads to pause and the
/// paused ones to reopen, by their ids.
#[derive(Debug, PartialEq, Eq)]
pub struct Plan<K> {
    pub pause: Vec<K>,
    pub resume: Vec<K>,
}

/// The [`Plan`] for `open` and `paused` pads (id and identity) while `forwarded` are attached.
pub fn plan<K: Copy>(open: &[(K, PadIdentity)], paused_pads: &[(K, PadIdentity)], forwarded: &[Forwarded]) -> Plan<K> {
    let identities: Vec<PadIdentity> = open.iter().chain(paused_pads).map(|(_, identity)| identity.clone()).collect();
    let pause = paused(&identities, forwarded);
    let (open_pause, paused_pause) = pause.split_at(open.len());
    Plan {
        pause: open.iter().zip(open_pause).filter(|(_, p)| **p).map(|((id, _), _)| *id).collect(),
        resume: paused_pads.iter().zip(paused_pause).filter(|(_, p)| !**p).map(|((id, _), _)| *id).collect(),
    }
}

/// The busid of the USB device a sysfs path lies under: the step right before that device's
/// first interface (`.../usb1/1-2/1-2.4/1-2.4:1.0/...` is `1-2.4`).
pub fn busid_in_sysfs_path(path: &str) -> Option<String> {
    let steps: Vec<&str> = path.split('/').collect();
    steps.windows(2).find_map(|pair| {
        let (device, next) = (pair[0], pair[1]);
        let looks_like_busid = device.split_once('-').is_some_and(|(bus, ports)| {
            !bus.is_empty() && bus.bytes().all(|b| b.is_ascii_digit()) && !ports.is_empty() && ports.split('.').all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
        });
        (looks_like_busid && next.strip_prefix(device).is_some_and(|rest| rest.starts_with(':'))).then(|| device.to_owned())
    })
}

/// The busid of the USB device behind SDL's path for a gamepad (`/dev/input/event5`,
/// `/dev/hidraw3`), read from sysfs.
#[cfg(target_os = "linux")]
pub fn busid_of_path(path: &str) -> Option<String> {
    let name = std::path::Path::new(path).file_name()?.to_str()?;
    let class = if name.starts_with("hidraw") { "hidraw" } else if name.starts_with("event") || name.starts_with("js") { "input" } else { return None };
    let real = std::fs::canonicalize(format!("/sys/class/{class}/{name}")).ok()?;
    busid_in_sysfs_path(real.to_str()?)
}

/// Other desktops don't tell which port a gamepad is on: [`paused`] goes by vendor and product.
#[cfg(not(target_os = "linux"))]
pub fn busid_of_path(_path: &str) -> Option<String> {
    None
}

#[cfg(test)]
mod tests;
