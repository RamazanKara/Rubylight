//! Owner liveness: which host processes are alive, and reaping the monitors of one that is
//! not.
//!
//! From punktfunk's pf-vdisplay `watchdog.rs` by Enrico Bühler (unom), MIT OR Apache-2.0.
//!
//! A monitor belongs to the process whose ADD created it. That owner ends when its last
//! control handle closes ([`evt_file_cleanup`]; a crash closes every handle, so this is
//! immediate) or when it sends no IOCTL for [`WATCHDOG_TIMEOUT_S`] ([`evt_timer`], a hung
//! host). Only that owner's monitors depart. The timer is a child of the device, so no tick
//! outlives it, and [`stop`] waits out a tick that is mid-reap.

use std::sync::Mutex;
use std::time::Duration;

use wdk_iddcx::nt_success;
use wdk_sys::{
    NTSTATUS, ULONG, WDF_OBJECT_ATTRIBUTES, WDF_TIMER_CONFIG, WDFDEVICE, WDFFILEOBJECT, WDFTIMER,
    call_unsafe_wdf_function_binding,
};

use crate::registry::lock;
use crate::worker::Sendable;

/// An owner must send an IOCTL within this window or its monitors are reaped. The host's
/// lease heartbeat runs every second. Reported over `IOCTL_GET_INFO`.
pub const WATCHDOG_TIMEOUT_S: u32 = 10;

const TICK_MS: u32 = 3000;
/// Silent ticks before a reap, rounded up so it never fires short of the timeout.
const TICKS_TO_REAP: u32 = (1000 * WATCHDOG_TIMEOUT_S).div_ceil(TICK_MS);

struct Owner {
    pid: u32,
    pings: u64,
    seen: u64,
    silent: u32,
}

static OWNERS: Mutex<Vec<Owner>> = Mutex::new(Vec::new());
/// Control handles that sent an IOCTL, with their process.
static HANDLES: Mutex<Vec<(usize, u32)>> = Mutex::new(Vec::new());

type SendTimer = Sendable<WDFTIMER>;
static TIMER: Mutex<Option<SendTimer>> = Mutex::new(None);

/// Create the timer, stopped, as a child of `device`, before any IOCTL can arrive.
pub fn create(device: WDFDEVICE) -> NTSTATUS {
    // AutomaticSerialization stays FALSE: the tick joins swap-chain workers, which must not
    // queue in front of the device's other callbacks.
    let mut cfg = WDF_TIMER_CONFIG {
        Size: core::mem::size_of::<WDF_TIMER_CONFIG>() as ULONG,
        EvtTimerFunc: Some(evt_timer),
        Period: TICK_MS,
        ..Default::default()
    };
    let mut attr = WDF_OBJECT_ATTRIBUTES {
        Size: core::mem::size_of::<WDF_OBJECT_ATTRIBUTES>() as ULONG,
        ParentObject: device.cast(),
        ExecutionLevel: wdk_sys::_WDF_EXECUTION_LEVEL::WdfExecutionLevelInheritFromParent,
        SynchronizationScope:
            wdk_sys::_WDF_SYNCHRONIZATION_SCOPE::WdfSynchronizationScopeInheritFromParent,
        ..Default::default()
    };
    let mut timer: WDFTIMER = core::ptr::null_mut();
    // SAFETY: `cfg` and `attr` are initialised locals; `timer` receives the handle.
    let status = unsafe {
        call_unsafe_wdf_function_binding!(WdfTimerCreate, &mut cfg, &mut attr, &mut timer)
    };
    dbglog!("[rlvd] watchdog WdfTimerCreate -> {status:#x}");
    if nt_success(status) {
        *lock(&TIMER) = Some(Sendable(timer));
    }
    status
}

/// Arm the watchdog once monitors can exist. Re-arming re-bases every owner's sample, so the
/// first tick after it never counts as silence.
pub fn start() {
    let held = lock(&TIMER);
    let Some(timer) = held.as_ref().map(|t| t.0) else {
        return;
    };
    if !crate::adapter::powered() {
        return;
    }
    for o in lock(&OWNERS).iter_mut() {
        o.seen = o.pings;
        o.silent = 0;
    }
    // SAFETY: `timer` is the live device-parented timer; a negative due time is relative.
    let _armed = unsafe {
        call_unsafe_wdf_function_binding!(WdfTimerStart, timer, -(i64::from(TICK_MS) * 10_000))
    };
}

/// Sleep suspends the clock, not the owners. Serialize start/stop so an async
/// adapter completion cannot rearm the timer after D0Exit paused it.
pub fn pause() {
    let held = lock(&TIMER);
    if let Some(timer) = held.as_ref().map(|t| t.0) {
        // SAFETY: the device-parented timer is live; wait for an active reap.
        let _ = unsafe { call_unsafe_wdf_function_binding!(WdfTimerStop, timer, 1) };
    }
}

/// Disarm on device cleanup and wait for a running tick, so no reap races the cleanup.
pub fn stop() {
    let taken = lock(&TIMER).take();
    let Some(Sendable(timer)) = taken else {
        return;
    };
    // SAFETY: the timer is still live (cleanup runs before the framework deletes children);
    // Wait = TRUE returns only after a running tick has returned.
    let _was_queued = unsafe { call_unsafe_wdf_function_binding!(WdfTimerStop, timer, 1) };
    lock(&OWNERS).clear();
    lock(&HANDLES).clear();
    dbglog!("[rlvd] watchdog: device cleanup, timer stopped");
}

/// Any IOCTL from `pid` proves it alive; remember the handle it came on.
pub fn ping(pid: u32, file_object: WDFFILEOBJECT) {
    {
        let mut owners = lock(&OWNERS);
        match owners.iter_mut().find(|o| o.pid == pid) {
            Some(o) => o.pings += 1,
            None => owners.push(Owner {
                pid,
                pings: 1,
                seen: 0,
                silent: 0,
            }),
        }
    }
    let key = file_object as usize;
    let mut handles = lock(&HANDLES);
    if !handles.iter().any(|(fo, _)| *fo == key) {
        handles.push((key, pid));
    }
}

/// `EvtFileCleanup`: a control handle is closing. If it was its process's last, that process
/// is gone and its monitors depart now.
pub unsafe extern "C" fn evt_file_cleanup(file_object: WDFFILEOBJECT) {
    let key = file_object as usize;
    let gone = {
        let mut handles = lock(&HANDLES);
        let Some(pos) = handles.iter().position(|(fo, _)| *fo == key) else {
            return;
        };
        let (_, pid) = handles.swap_remove(pos);
        (!handles.iter().any(|(_, p)| *p == pid)).then_some(pid)
    };
    if let Some(pid) = gone {
        lock(&OWNERS).retain(|o| o.pid != pid);
        let n = crate::monitor::reap_owner(pid, Duration::ZERO);
        dbglog!("[rlvd] owner {pid}: last control handle closed; departed {n} monitor(s)");
    }
}

/// One tick: depart the monitors of every owner silent for the whole window.
unsafe extern "C" fn evt_timer(_timer: WDFTIMER) {
    let stale: Vec<u32> = {
        let mut owners = lock(&OWNERS);
        let mut stale = Vec::new();
        for o in owners.iter_mut() {
            if o.seen != o.pings {
                o.seen = o.pings;
                o.silent = 0;
                continue;
            }
            o.silent += 1;
            if o.silent >= TICKS_TO_REAP {
                stale.push(o.pid);
            }
        }
        owners.retain(|o| !stale.contains(&o.pid));
        stale
    };
    for pid in stale {
        lock(&HANDLES).retain(|(_, p)| *p != pid);
        let n = crate::monitor::reap_owner(pid, Duration::from_secs(3));
        dbglog!(
            "[rlvd] watchdog: no IOCTL from owner {pid} in {WATCHDOG_TIMEOUT_S} s; departed {n} monitor(s)"
        );
    }
}
