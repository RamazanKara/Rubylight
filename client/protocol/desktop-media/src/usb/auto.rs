//! The stream's automatic attach: the devices on the `usb_auto` list are attached whenever they
//! are plugged in, and retried after a cooldown when that fails. One whose password prompt the
//! user cancelled (Linux's USB/IP server needs root) is left alone until the user attaches it
//! by hand or ticks Automatic again: the prompt never comes back by itself.

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use super::{attach_tracked, list_with, Local, Panel, Route};
use crate::strings::trf;

/// How often the automatic attach looks for newly plugged-in devices.
const AUTO_POLL: Duration = Duration::from_secs(2);
/// Wait before retrying an automatic attach that failed while still plugged in.
pub(super) const AUTO_RETRY_COOLDOWN: Duration = Duration::from_secs(10);

/// What the automatic list remembers a device by: its vendor and product id, so that it
/// matches on any port (Android's UsbForwarding.autoKey writes the same).
pub fn auto_key(vid: u16, pid: u16) -> String {
    format!("{vid:04x}:{pid:04x}")
}

thread_local! {
    /// Password prompts cancelled or refused on this thread.
    static REFUSED: Cell<u64> = const { Cell::new(0) };
}

/// Counts a password prompt the user cancelled, or polkit refused, on this thread (pkexec's
/// exit 126 or 127).
#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
pub(crate) fn note_password_refused() {
    REFUSED.with(|refused| refused.set(refused.get() + 1));
}

/// Runs `open` for `local`. When it showed a password prompt that was cancelled or refused,
/// `local` goes on `quiet`: the automatic attach doesn't ask for it again.
pub(super) fn hushed_on_refusal<T>(quiet: &Mutex<Vec<String>>, local: &Local, open: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    let before = REFUSED.with(Cell::get);
    let result = open();
    if result.is_err() && REFUSED.with(Cell::get) != before {
        let key = auto_key(local.vid, local.pid);
        eprintln!("nightjar: USB {}: the password prompt was cancelled; not attaching {key} automatically until it is attached by hand", local.busid);
        let mut quiet = quiet.lock().unwrap_or_else(PoisonError::into_inner);
        if !quiet.contains(&key) { quiet.push(key); }
    }
    result
}

/// The devices of `local` to attach automatically now: on the list and not on `quiet`,
/// forwardable, not attached or being opened, and either unseen since plug-in (`tried` has no
/// entry) or past their failed retry cooldown (`Some(retry_at) <= now`). `tried` forgets
/// unplugged devices.
pub(super) fn auto_due(
    local: &[Local],
    auto: &[String],
    quiet: &[String],
    busy: impl Fn(&str) -> bool,
    tried: &mut HashMap<String, Option<Instant>>,
    now: Instant,
) -> Vec<Local> {
    tried.retain(|busid, _| local.iter().any(|l| l.busid == *busid));
    let mut due = Vec::new();
    for device in local {
        let key = auto_key(device.vid, device.pid);
        if device.blocked.is_some() || !auto.contains(&key) || quiet.contains(&key) { continue; }
        if busy(&device.busid) { continue; }
        match tried.get(&device.busid) {
            Some(None) => continue,
            Some(Some(retry_at)) if now < *retry_at => continue,
            _ => {}
        }
        tried.insert(device.busid.clone(), None);
        due.push(device.clone());
    }
    due
}

impl Panel {
    fn auto(&self) -> MutexGuard<'_, Vec<String>> {
        self.auto.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Lets the automatic attach ask for `local`'s password again: the user acted on it.
    pub(super) fn unhush(&self, local: &Local) {
        let key = auto_key(local.vid, local.pid);
        self.quiet.lock().unwrap_or_else(PoisonError::into_inner).retain(|k| *k != key);
    }

    /// Whether `local` is attached automatically when it is plugged in.
    pub fn is_auto(&self, local: &Local) -> bool {
        self.auto().contains(&auto_key(local.vid, local.pid))
    }

    /// Adds `local` to the automatic list or takes it off; the thread attaches an added one.
    pub fn set_auto(&mut self, local: &Local, on: bool) {
        let key = auto_key(local.vid, local.pid);
        if on { self.unhush(local); }
        {
            let mut auto = self.auto();
            if on == auto.contains(&key) { return; }
            if on { auto.push(key); } else { auto.retain(|k| *k != key); }
        }
        self.auto_changed = true;
        if on { self.start_auto(); }
    }

    /// The automatic list after the user changed it, once, for saving.
    pub fn take_auto_change(&mut self) -> Option<Vec<String>> {
        std::mem::take(&mut self.auto_changed).then(|| self.auto().clone())
    }

    /// Starts the thread that attaches the automatic list's devices, while the list has any.
    pub(super) fn start_auto(&mut self) {
        if self.auto_thread.is_some() || self.auto().is_empty() { return; }
        let (stop, stopped) = std::sync::mpsc::channel::<()>();
        let (forwarder, opening, auto, quiet, owner) = (self.forwarder.clone(), self.opening.clone(), self.auto.clone(), self.quiet.clone(), self.owner);
        let spawned = std::thread::Builder::new().name("usb-auto".into()).spawn(move || {
            let mut tried = HashMap::new();
            let mut context: Option<rusb::Context> = None;
            loop {
                let wanted = auto.lock().unwrap_or_else(PoisonError::into_inner).clone();
                let hushed = quiet.lock().unwrap_or_else(PoisonError::into_inner).clone();
                if context.is_none() {
                    match rusb::Context::new() {
                        Ok(c) => context = Some(c),
                        Err(e) => eprintln!("nightjar: USB: {}", trf("USB devices could not be listed: {}", &[&e])),
                    }
                }
                if let Some(ctx) = context.as_ref() {
                    match list_with(ctx) {
                        Ok(local) => {
                            let busy = |busid: &str| {
                                opening.lock().unwrap_or_else(PoisonError::into_inner).busids.iter().any(|b| b == busid)
                                    || forwarder.list().iter().any(|a| a.is_active() && a.busid == busid)
                            };
                            let now = Instant::now();
                            for device in auto_due(&local, &wanted, &hushed, busy, &mut tried, now) {
                                if !opening.lock().unwrap_or_else(PoisonError::into_inner).try_start(&device.busid) {
                                    continue;
                                }
                                eprintln!("nightjar: USB {}: attaching {} automatically", device.busid, device.name);
                                if !attach_tracked(&forwarder, owner, Route::Direct, &opening, &quiet, &device) {
                                    tried.insert(device.busid, Some(Instant::now() + AUTO_RETRY_COOLDOWN));
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("nightjar: USB: {e}");
                            context = None;
                        }
                    }
                }
                // The channel closes with the panel: the stream ended.
                if !matches!(stopped.recv_timeout(AUTO_POLL), Err(std::sync::mpsc::RecvTimeoutError::Timeout)) { return; }
            }
        });
        match spawned {
            Ok(_) => self.auto_thread = Some(stop),
            Err(e) => self.error = Some(trf("USB devices can't be attached automatically: {}", &[&e])),
        }
    }
}
