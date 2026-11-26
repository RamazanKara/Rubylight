//! USB forwarding through usbipd-win, for a Windows device another driver holds (a HID
//! controller, a fightstick, storage, a Bluetooth dongle, a class-driver composite): Windows
//! keeps its own driver on it, and usbipd-win lends the device only while it is attached.
//!
//! The user asks for it per device in the stream controls ("Forward with usbipd-win"); nothing
//! shares a device by itself. Sharing (`usbipd bind`) needs administrator rights, one Windows
//! prompt, and leaves the device's driver alone: usbipd-win takes the device only while a client
//! has imported it and gives it back when that connection closes. Nightjar imports it on
//! loopback ([`LOOPBACK`], never another address), checks that usbipd-win answered with the
//! device the user picked (busid, vendor and product), and relays the tunnel to that connection
//! as it does for Linux's usbip-host ([`nightjar_core::usb::UsbDevice::relay`]).
//!
//! Only what Nightjar shared is ever unshared: a device the user had shared already stays as it
//! was. Before `usbipd bind` the device goes into a journal in Nightjar's config directory
//! ([`Journal`]), and the share's GUID follows once bound. The attachment's end unshares it by
//! that GUID (`usbipd unbind --guid`, a second prompt) and forgets it; after a crash, the next
//! start unshares what the journal still lists ([`recover`]).

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use nightjar_core::usb::{wire, Completion, TcpTunnel, Transfer, Tunnel, UsbDevice};
use serde::{Deserialize, Serialize};

use crate::strings::{tr, trf};

#[cfg(windows)]
mod cli;
#[cfg(windows)]
pub use cli::{recover_in_background, Cli};

/// usbipd-win's USB/IP port, on this computer only.
pub const LOOPBACK: SocketAddr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 3240));
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// How long usbipd-win may take to answer the import (it restarts the device first).
const IMPORT_TIMEOUT: Duration = Duration::from_secs(15);

/// One device as `usbipd state` lists it (usbipd-win's `Usbipd.Automation.Device`).
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub struct StateDevice {
    /// usbipd-win's busid (`2-3`: its hub and port, not Linux's); None while unplugged.
    #[serde(default)]
    pub bus_id: Option<String>,
    /// The client it is attached to, if any.
    #[serde(default, rename = "ClientIPAddress")]
    pub client_ip_address: Option<serde_json::Value>,
    #[serde(default)]
    pub description: Option<String>,
    /// Windows' device instance id.
    pub instance_id: String,
    #[serde(default)]
    pub is_forced: bool,
    /// Set while it is shared (bound).
    #[serde(default)]
    pub persisted_guid: Option<String>,
}

impl StateDevice {
    pub fn shared(&self) -> bool {
        self.persisted_guid.is_some()
    }

    pub fn attached(&self) -> bool {
        self.client_ip_address.as_ref().is_some_and(|ip| !ip.is_null())
    }

    /// The busid usbipd-win imports it by: plugged in, behind a hub it supports.
    pub fn busid(&self) -> Option<&str> {
        self.bus_id.as_deref().filter(|busid| valid_busid(busid))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct State {
    devices: Vec<StateDevice>,
}

/// What `usbipd state` printed.
pub fn parse_state(json: &str) -> Result<Vec<StateDevice>, String> {
    serde_json::from_str::<State>(json).map(|state| state.devices).map_err(|e| state_error(&e))
}

fn state_error(e: &dyn std::fmt::Display) -> String {
    trf("usbipd-win’s device list could not be read: {}", &[&e.to_string()])
}

/// usbipd-win's `x-y` busid (`Usbipd.Automation.BusId`), the only form passed to it.
pub fn valid_busid(busid: &str) -> bool {
    let number = |s: &str| !s.is_empty() && s.len() <= 5 && !s.starts_with('0') && s.bytes().all(|b| b.is_ascii_digit());
    busid.split_once('-').is_some_and(|(bus, port)| number(bus) && number(port))
}

/// A GUID as usbipd-win prints it, the only form passed to it.
pub fn valid_guid(guid: &str) -> bool {
    let bare = guid.trim_start_matches('{').trim_end_matches('}');
    bare.len() == 36 && bare.bytes().enumerate().all(|(i, b)| if matches!(i, 8 | 13 | 18 | 23) { b == b'-' } else { b.is_ascii_hexdigit() })
}

/// The device usbipd-win lists for `instance_id` (Windows' device instance id, any case).
pub fn find<'a>(state: &'a [StateDevice], instance_id: &str) -> Option<&'a StateDevice> {
    state.iter().find(|device| device.instance_id.eq_ignore_ascii_case(instance_id))
}

fn shared_as(state: &[StateDevice], guid: &str) -> bool {
    state.iter().any(|device| device.persisted_guid.as_deref().is_some_and(|g| g.eq_ignore_ascii_case(guid)))
}

/// usbipd-win, as Nightjar runs it: `usbipd state`, and `usbipd bind` and `unbind` through
/// Windows' administrator prompt. A fake stands in for it in tests.
pub trait Usbipd: Send + Sync {
    fn state(&self) -> Result<Vec<StateDevice>, String>;
    /// `usbipd bind --busid <busid>`: shares it, leaving its driver as it is.
    fn bind(&self, busid: &str) -> Result<(), String>;
    /// `usbipd unbind --guid <guid>`: unshares that share, ending an attachment of it.
    fn unbind(&self, guid: &str) -> Result<(), String>;
}

/// One device Nightjar shared: `guid` is None between the journal's write and the bind's end.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Entry {
    pub instance_id: String,
    pub busid: String,
    #[serde(default)]
    pub guid: Option<String>,
}

#[derive(Default, Serialize, Deserialize)]
struct JournalFile {
    version: u32,
    entries: Vec<Entry>,
}

/// One read-modify-write of a journal at a time in this process.
static JOURNAL: Mutex<()> = Mutex::new(());

/// The devices Nightjar shared through usbipd-win and has not unshared yet
/// (`usbipd-journal.json`). Written, and flushed to disk, before each change it records.
#[derive(Clone, Debug)]
pub struct Journal {
    path: PathBuf,
}

impl Journal {
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The journal in Nightjar's config directory.
    pub fn real() -> Result<Self, String> {
        crate::config::config_dir().map(|dir| Self::at(dir.join("usbipd-journal.json"))).map_err(|e| journal_error(&e))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// What it lists: nothing when there is no journal. One that can't be read is an error,
    /// never taken as empty.
    pub fn entries(&self) -> Result<Vec<Entry>, String> {
        let _held = JOURNAL.lock().unwrap_or_else(PoisonError::into_inner);
        self.read()
    }

    fn read(&self) -> Result<Vec<Entry>, String> {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => serde_json::from_str::<JournalFile>(&text).map(|file| file.entries).map_err(|e| journal_error(&e)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(journal_error(&e)),
        }
    }

    /// Writes `entries` whole (a temporary file, flushed, then renamed over the journal); no
    /// journal at all once nothing is left.
    fn write(&self, entries: &[Entry]) -> Result<(), String> {
        if entries.is_empty() {
            return match std::fs::remove_file(&self.path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(journal_error(&e)),
                _ => Ok(()),
            };
        }
        let file = JournalFile { version: 1, entries: entries.to_vec() };
        let text = serde_json::to_string_pretty(&file).map_err(|e| journal_error(&e))?;
        if let Some(dir) = self.path.parent() { std::fs::create_dir_all(dir).map_err(|e| journal_error(&e))?; }
        let temporary = self.path.with_extension("json.tmp");
        let written = std::fs::File::create(&temporary).and_then(|mut out| {
            out.write_all(text.as_bytes())?;
            out.sync_all()
        });
        let renamed = written.and_then(|()| std::fs::rename(&temporary, &self.path));
        if renamed.is_err() { let _ = std::fs::remove_file(&temporary); }
        renamed.map_err(|e| journal_error(&e))
    }

    /// Records `entry`, replacing what it had for the same device.
    pub fn record(&self, entry: Entry) -> Result<(), String> {
        let _held = JOURNAL.lock().unwrap_or_else(PoisonError::into_inner);
        let mut entries = self.read()?;
        entries.retain(|e| !e.instance_id.eq_ignore_ascii_case(&entry.instance_id));
        entries.push(entry);
        self.write(&entries)
    }

    /// Forgets the device: Nightjar shares it no longer.
    pub fn forget(&self, instance_id: &str) -> Result<(), String> {
        let _held = JOURNAL.lock().unwrap_or_else(PoisonError::into_inner);
        let mut entries = self.read()?;
        let before = entries.len();
        entries.retain(|e| !e.instance_id.eq_ignore_ascii_case(instance_id));
        if entries.len() == before { return Ok(()); }
        self.write(&entries)
    }
}

fn journal_error(e: &dyn std::fmt::Display) -> String {
    trf("Nightjar’s record of the devices it shared through usbipd-win can’t be used: {}", &[&e.to_string()])
}

/// How the device came to be shared for an attachment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Share {
    /// Nightjar shared it, and unshares it afterwards.
    Ours { instance_id: String, guid: String },
    /// The user had shared it already: it stays shared.
    Theirs,
}

/// Shares `device` for an attachment. One the user shared already is used as it is; otherwise
/// it is journaled, bound (Windows asks for administrator rights) and its GUID journaled.
pub fn share(usbipd: &dyn Usbipd, journal: &Journal, device: &StateDevice) -> Result<Share, String> {
    let busid = device.busid().ok_or_else(not_listed)?;
    if device.attached() { return Err(tr("Another computer has this device attached through usbipd-win.").to_owned()); }
    if device.shared() { return Ok(Share::Theirs); }
    let instance_id = device.instance_id.clone();
    journal.record(Entry { instance_id: instance_id.clone(), busid: busid.to_owned(), guid: None })?;
    if let Err(why) = usbipd.bind(busid) {
        // Whatever the bind did is what the journal keeps: forgotten only once surely not shared.
        match usbipd.state().map(|state| find(&state, &instance_id).is_some_and(StateDevice::shared)) {
            Ok(false) => journal.forget(&instance_id)?,
            Ok(true) => eprintln!("nightjar: USB: usbipd-win shared {instance_id} although the bind failed; unsharing it at the next start"),
            Err(e) => eprintln!("nightjar: USB: after a failed bind, usbipd-win's list could not be read ({e}); the next start checks {instance_id}"),
        }
        return Err(why);
    }
    let guid = usbipd.state()?.iter().find(|d| d.instance_id.eq_ignore_ascii_case(&instance_id)).and_then(|d| d.persisted_guid.clone())
        .filter(|guid| valid_guid(guid))
        .ok_or_else(|| trf("usbipd-win could not share the device: {}", &[&"no share after the bind"]))?;
    journal.record(Entry { instance_id: instance_id.clone(), busid: busid.to_owned(), guid: Some(guid.clone()) })?;
    eprintln!("nightjar: USB: shared {instance_id} through usbipd-win ({guid})");
    Ok(Share::Ours { instance_id, guid })
}

/// Unshares what [`share`] shared, by its GUID, and forgets it. Nothing is unbound when that
/// share is gone already (the user unshared it, or shared it again under another GUID).
pub fn unshare(usbipd: &dyn Usbipd, journal: &Journal, instance_id: &str, guid: &str) -> Result<(), String> {
    if shared_as(&usbipd.state()?, guid) {
        usbipd.unbind(guid)?;
        eprintln!("nightjar: USB: unshared {instance_id} ({guid}); Windows has it with its own driver");
    }
    journal.forget(instance_id)
}

/// After a crash: unshares every device the journal still lists, by its GUID or, when the bind
/// never reported one, by the share the device has now (the journal shows it was not shared
/// before Nightjar's bind). An entry whose share is gone is forgotten; one whose unbind fails
/// stays for the next start. The number unshared, or why the journal or usbipd-win's list
/// could not be read.
pub fn recover(usbipd: &dyn Usbipd, journal: &Journal) -> Result<usize, String> {
    let entries = journal.entries()?;
    if entries.is_empty() { return Ok(0); }
    let state = usbipd.state()?;
    let mut unshared = 0;
    for entry in entries {
        let guid = entry.guid.clone().or_else(|| find(&state, &entry.instance_id).and_then(|d| d.persisted_guid.clone()));
        match guid.filter(|guid| valid_guid(guid) && shared_as(&state, guid)) {
            Some(guid) => match usbipd.unbind(&guid) {
                Ok(()) => {
                    eprintln!("nightjar: USB: unshared {} ({guid}), left shared by Nightjar before it stopped", entry.instance_id);
                    journal.forget(&entry.instance_id)?;
                    unshared += 1;
                }
                Err(why) => eprintln!("nightjar: USB: {} stays shared through usbipd-win until the next start: {why}", entry.instance_id),
            },
            None => journal.forget(&entry.instance_id)?,
        }
    }
    Ok(unshared)
}

fn not_listed() -> String {
    tr("usbipd-win doesn’t list this device. Press Refresh and attach again.").to_owned()
}

/// Imports `busid` from usbipd-win at `addr`, which must be on this computer: the connection,
/// after usbipd-win answered with the device whose `vid:pid` the user picked, and that device.
pub fn import(addr: SocketAddr, busid: &str, vid: u16, pid: u16) -> Result<(TcpStream, wire::Device), String> {
    if !addr.ip().is_loopback() {
        return Err(trf("usbipd-win could not share the device: {}", &[&format!("{addr} is not on this computer")]));
    }
    let request = wire::import_request(busid).ok_or_else(not_listed)?;
    let stream = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT).map_err(|e| match e.kind() {
        std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::TimedOut => tr("usbipd-win isn’t running: start its service, then retry.").to_owned(),
        _ => trf("usbipd-win could not share the device: {}", &[&e.to_string()]),
    })?;
    let failed = |e: std::io::Error| trf("usbipd-win could not share the device: {}", &[&e.to_string()]);
    if !stream.peer_addr().map_err(failed)?.ip().is_loopback() {
        return Err(trf("usbipd-win could not share the device: {}", &[&"not on this computer"]));
    }
    stream.set_read_timeout(Some(IMPORT_TIMEOUT)).map_err(failed)?;
    stream.set_write_timeout(Some(IMPORT_TIMEOUT)).map_err(failed)?;
    (&stream).write_all(&request).map_err(failed)?;
    let mut header = [0; wire::OP_HEADER_BYTES];
    (&stream).read_exact(&mut header).map_err(failed)?;
    let header = wire::OpHeader::decode(&header);
    if header.version != wire::VERSION || header.code != wire::OP_REP_IMPORT {
        return Err(trf("usbipd-win could not share the device: {}", &[&format!("unexpected reply {:04x}/{:04x}", header.version, header.code)]));
    }
    if header.status != 0 {
        return Err(trf("usbipd-win refused the device (status {}).", &[&header.status]));
    }
    let mut device = [0; wire::DEVICE_BYTES];
    (&stream).read_exact(&mut device).map_err(failed)?;
    let device = wire::Device::decode(&device).ok_or_else(|| tr("usbipd-win answered for another device, so Nightjar did not use it.").to_owned())?;
    if device.busid != busid || (device.vendor, device.product) != (vid, pid) {
        eprintln!("nightjar: USB: usbipd-win answered {} {:04x}:{:04x} for {busid} {vid:04x}:{pid:04x}", device.busid, device.vendor, device.product);
        return Err(tr("usbipd-win answered for another device, so Nightjar did not use it.").to_owned());
    }
    stream.set_read_timeout(None).map_err(failed)?;
    stream.set_write_timeout(None).map_err(failed)?;
    Ok((stream, device))
}

/// What the user picked, as [`open_with`] needs it.
#[derive(Clone, Debug)]
pub struct Picked {
    /// Nightjar's busid for it (Linux's form), which the attachment goes by.
    pub busid: String,
    pub vid: u16,
    pub pid: u16,
    /// Windows' device instance id, which usbipd-win lists it by.
    pub instance_id: String,
    /// bDeviceClass, or the first interface's class when that is 0.
    pub class: u8,
}

/// Shares `picked` through `usbipd` when the user hadn't, and imports it from `addr`. A share
/// Nightjar made is undone at once when the import fails.
pub fn open_with(usbipd: Arc<dyn Usbipd>, journal: Journal, addr: SocketAddr, picked: &Picked) -> Result<Arc<Shared>, String> {
    let state = usbipd.state()?;
    let device = find(&state, &picked.instance_id).ok_or_else(not_listed)?;
    let theirs = device.busid().ok_or_else(not_listed)?.to_owned();
    let share = share(&*usbipd, &journal, device)?;
    let (stream, mut described) = match import(addr, &theirs, picked.vid, picked.pid) {
        Ok(imported) => imported,
        Err(why) => {
            if let Share::Ours { instance_id, guid } = &share
                && let Err(e) = unshare(&*usbipd, &journal, instance_id, guid) {
                    eprintln!("nightjar: USB: unsharing {instance_id} after the failed import failed: {e}; the next start retries");
                }
            return Err(why);
        }
    };
    // The attachment goes by Nightjar's own busid; usbipd-win's numbers stay for its USB/IP.
    described.busid = picked.busid.clone();
    let connection = Arc::new(TcpTunnel::new(stream).map_err(|e| trf("usbipd-win could not share the device: {}", &[&e.to_string()]))?);
    let ours = match share {
        Share::Ours { instance_id, guid } => Some(Unshare { usbipd, journal, instance_id, guid }),
        Share::Theirs => None,
    };
    eprintln!("nightjar: USB {}: imported from usbipd-win as {theirs}", picked.busid);
    Ok(Arc::new(Shared { described, class: picked.class, connection, relayed: AtomicBool::new(false), ours: Mutex::new(ours) }))
}

/// A share to undo once the attachment ends.
struct Unshare {
    usbipd: Arc<dyn Usbipd>,
    journal: Journal,
    instance_id: String,
    guid: String,
}

impl Unshare {
    fn run(self) {
        if let Err(why) = unshare(&*self.usbipd, &self.journal, &self.instance_id, &self.guid) {
            eprintln!("nightjar: USB: {} stays shared through usbipd-win until the next start: {why}", self.instance_id);
        }
    }
}

/// A device usbipd-win lends, served to the host on the connection the tunnel is relayed to.
pub struct Shared {
    described: wire::Device,
    class: u8,
    connection: Arc<TcpTunnel>,
    relayed: AtomicBool,
    ours: Mutex<Option<Unshare>>,
}

impl std::fmt::Debug for Shared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shared").field("busid", &self.described.busid).finish_non_exhaustive()
    }
}

impl Shared {
    /// Gives the device back: closing the connection ends usbipd-win's attachment, which gives
    /// Windows the device back, and a share Nightjar made is undone. `wait` waits for that
    /// (tests); else a thread does it, as it waits for Windows' prompt.
    pub fn release(&self, wait: bool) {
        self.connection.close();
        let Some(unshare) = self.ours.lock().unwrap_or_else(PoisonError::into_inner).take() else { return };
        if wait { return unshare.run(); }
        if let Err(e) = std::thread::Builder::new().name("usbipd-unbind".into()).spawn(move || unshare.run()) {
            eprintln!("nightjar: USB: no thread to unshare the device ({e}); the next start does");
        }
    }
}

impl UsbDevice for Shared {
    fn describe(&self) -> wire::Device {
        self.described.clone()
    }

    fn class(&self) -> u8 {
        self.class
    }

    fn submit(&self, _: Transfer, done: Completion) {
        // Never called: the server relays the tunnel to usbipd-win instead.
        done.complete(nightjar_core::usb::Reply::failed(wire::EPROTO));
    }

    fn unlink(&self, _: u32) {}

    fn close(&self) {
        self.release(false);
    }

    fn relay(&self) -> Option<std::io::Result<Arc<dyn Tunnel>>> {
        if self.relayed.swap(true, Ordering::SeqCst) {
            return Some(Err(std::io::Error::other("the device was served once already")));
        }
        Some(Ok(self.connection.clone()))
    }
}

impl Drop for Shared {
    fn drop(&mut self) {
        self.release(false);
    }
}

#[cfg(test)]
mod tests;
