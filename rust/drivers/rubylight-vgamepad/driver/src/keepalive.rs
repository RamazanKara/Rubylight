// Copyright (c) 2026 Chase Payne
// SPDX-License-Identifier: MIT
//
// Ported from Nonary/libvirtualgamepad driver/src/driver.cpp.
// Adapted for Rubylight by RamazanKara (https://github.com/RamazanKara/Rubylight).

//! PID periodic `WDFTIMER`, SC26 high-resolution waitable timer thread, and monotonic QPC clock.

use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};

use rubylight_vgamepad_proto::lifecycle::{PID_TICK_MS, SC26_TICK_MS};
use wdk_sys::{
    NTSTATUS, ULONG, WDF_OBJECT_ATTRIBUTES, WDF_TIMER_CONFIG, WDFDEVICE, WDFOBJECT, WDFTIMER,
    call_unsafe_wdf_function_binding,
};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::Threading::{
    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CancelWaitableTimer, CreateEventW,
    CreateWaitableTimerExW, GetCurrentThread, INFINITE, ResetEvent, SetEvent, SetThreadPriority,
    SetWaitableTimer, THREAD_PRIORITY_HIGHEST, TIMER_ALL_ACCESS, WaitForMultipleObjects,
};

use crate::device::{DeviceContext, lookup_device};
use crate::{STATUS_SUCCESS, nt_success};

/// Monotonic microsecond timestamp via `QueryPerformanceCounter`.
pub fn now_us() -> u64 {
    static FREQ: OnceLock<u64> = OnceLock::new();
    let freq = *FREQ.get_or_init(|| {
        let mut f = 0i64;
        // SAFETY: `f` is a valid stack i64 out pointer.
        unsafe {
            let _ = QueryPerformanceFrequency(&mut f);
        }
        u64::try_from(f).unwrap_or(10_000_000).max(1)
    });
    let mut counter = 0i64;
    // SAFETY: `counter` is a valid stack i64 out pointer.
    unsafe {
        let _ = QueryPerformanceCounter(&mut counter);
    }
    let ticks = u128::from(u64::try_from(counter).unwrap_or(0));
    ((ticks * 1_000_000) / u128::from(freq)) as u64
}

struct SendTimer(WDFTIMER);
// SAFETY: `WDFTIMER` is an opaque framework handle; WDF serializes start/stop/delete across threads.
unsafe impl Send for SendTimer {}
// SAFETY: Accessing `&SendTimer` only reads the opaque handle value passed to WDF DDIs.
unsafe impl Sync for SendTimer {}

pub struct PidTimer {
    timer: Mutex<Option<SendTimer>>,
}

impl PidTimer {
    pub fn new() -> Self {
        Self {
            timer: Mutex::new(None),
        }
    }

    pub fn create(&self, device: WDFDEVICE) -> NTSTATUS {
        let mut cfg = WDF_TIMER_CONFIG {
            Size: core::mem::size_of::<WDF_TIMER_CONFIG>() as ULONG,
            EvtTimerFunc: Some(evt_pid_timer),
            Period: PID_TICK_MS,
            AutomaticSerialization: wdk_sys::FALSE as u8,
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
        // SAFETY: `cfg` and `attr` are initialized locals; `timer` receives the new child handle.
        let status = unsafe {
            call_unsafe_wdf_function_binding!(WdfTimerCreate, &mut cfg, &mut attr, &mut timer)
        };
        if nt_success(status) && !timer.is_null() {
            *self.timer.lock().unwrap() = Some(SendTimer(timer));
            return STATUS_SUCCESS;
        }
        status
    }

    pub fn start(&self) {
        let timer = self.timer.lock().unwrap().as_ref().map(|t| t.0);
        let Some(timer) = timer else {
            return;
        };
        // SAFETY: `timer` is live and parented to the device; negative due time is relative in 100ns units.
        let _ = unsafe {
            call_unsafe_wdf_function_binding!(
                WdfTimerStart,
                timer,
                -(i64::from(PID_TICK_MS) * 10_000)
            )
        };
    }

    /// Stop the periodic PID timer. `wait = true` must never be passed while holding any lock
    /// acquired inside `evt_pid_timer` (such as `state_lock`).
    pub fn stop(&self, wait: bool) {
        let timer = self.timer.lock().unwrap().as_ref().map(|t| t.0);
        if let Some(timer) = timer {
            // SAFETY: `timer` is live and parented to the device.
            let _ = unsafe {
                call_unsafe_wdf_function_binding!(
                    WdfTimerStop,
                    timer,
                    if wait { wdk_sys::TRUE } else { wdk_sys::FALSE } as u8
                )
            };
        }
    }
}

unsafe extern "C" fn evt_pid_timer(timer: WDFTIMER) {
    // SAFETY: `timer` is valid for the duration of `EvtTimerFunc`.
    let parent: WDFOBJECT =
        unsafe { call_unsafe_wdf_function_binding!(WdfTimerGetParentObject, timer) };
    if let Some(dev) = lookup_device(parent.cast())
        && !dev.on_pid_tick()
    {
        // Inside `EvtTimerFunc`: stop asynchronously (`Wait = FALSE`).
        dev.pid_timer.stop(false);
        // Re-check in case a write callback started an effect between `on_pid_tick` and `stop`.
        if dev.any_pid_playing() {
            dev.pid_timer.start();
        }
    }
}

struct OwnedWinHandle(HANDLE);
// SAFETY: Win32 event and waitable timer handles are process-global tokens closed once in `Drop`.
unsafe impl Send for OwnedWinHandle {}
// SAFETY: Win32 synchronization handles are thread-safe to signal (`SetEvent` / `SetWaitableTimer`).
unsafe impl Sync for OwnedWinHandle {}

impl Drop for OwnedWinHandle {
    fn drop(&mut self) {
        if !self.0.is_invalid() && !self.0.0.is_null() {
            // SAFETY: Sole owner of `self.0`.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

pub struct Sc26Keepalive {
    stop_event: Option<OwnedWinHandle>,
    timer: Option<OwnedWinHandle>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl Sc26Keepalive {
    pub fn new() -> Self {
        // SAFETY: Creating unnamed manual-reset event and high-resolution waitable timer handles.
        unsafe {
            let stop_event = CreateEventW(None, true, false, None)
                .ok()
                .map(OwnedWinHandle);
            let timer = CreateWaitableTimerExW(
                None,
                None,
                CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                TIMER_ALL_ACCESS.0,
            )
            .or_else(|_| CreateWaitableTimerExW(None, None, 0, TIMER_ALL_ACCESS.0))
            .ok()
            .map(OwnedWinHandle);
            Self {
                stop_event,
                timer,
                thread: Mutex::new(None),
            }
        }
    }

    pub fn has_timer(&self) -> bool {
        self.stop_event.is_some() && self.timer.is_some()
    }

    pub fn arm(&self, delay_us: u64) -> bool {
        let Some(timer) = &self.timer else {
            return false;
        };
        let due_100ns = -(i64::try_from(delay_us.max(1)).unwrap_or(4_000) * 10);
        // SAFETY: `timer.0` lives until `Self::drop` (worker joined before drop).
        unsafe { SetWaitableTimer(timer.0, &due_100ns, 0, None, None, false).is_ok() }
    }

    pub fn ensure_started(&self, dev: &Arc<DeviceContext>) {
        if !self.has_timer() {
            dev.clear_sc26_timer_running();
            return;
        }
        let mut guard = self.thread.lock().unwrap();
        if let Some(prev) = guard.take() {
            // Join a prior worker that exited when its last Steam Controller went away.
            let _ = prev.join();
        }
        let dev_clone = Arc::clone(dev);
        let stop_raw = self.stop_event.as_ref().unwrap().0.0 as usize;
        let timer_raw = self.timer.as_ref().unwrap().0.0 as usize;
        // SAFETY: `stop_raw` is a live Win32 manual-reset event handle owned by `self`.
        unsafe {
            let _ = ResetEvent(HANDLE(stop_raw as *mut _));
        }
        if !self.arm(u64::from(SC26_TICK_MS) * 1000) {
            dev.clear_sc26_timer_running();
            return;
        }
        let handle = thread::Builder::new()
            .name("sc26-keepalive".into())
            .spawn(move || sc26_loop(dev_clone, stop_raw, timer_raw));
        match handle {
            Ok(h) => *guard = Some(h),
            Err(_) => {
                // SAFETY: `timer_raw` is our live waitable timer.
                unsafe {
                    let _ = CancelWaitableTimer(HANDLE(timer_raw as *mut _));
                }
                dev.clear_sc26_timer_running();
            }
        }
    }

    /// Signal stop and join the keepalive worker thread. Must be called outside `state_lock`.
    pub fn stop_and_join(&self) {
        if let Some(ev) = &self.stop_event {
            // SAFETY: `ev.0` lives until `Self::drop`.
            unsafe {
                let _ = SetEvent(ev.0);
            }
        }
        let join = self.thread.lock().unwrap().take();
        if let Some(h) = join {
            let _ = h.join();
        }
        if let Some(timer) = &self.timer {
            // SAFETY: `timer.0` lives until `Self::drop`.
            unsafe {
                let _ = CancelWaitableTimer(timer.0);
            }
        }
    }
}

fn sc26_loop(dev: Arc<DeviceContext>, stop_raw: usize, timer_raw: usize) {
    // SAFETY: Raising priority of the current keepalive worker thread; failure is non-fatal.
    unsafe {
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST);
    }
    let handles = [HANDLE(stop_raw as *mut _), HANDLE(timer_raw as *mut _)];

    loop {
        // SAFETY: Both handles belong to `dev.sc26_keepalive`, which waits for this thread to join before closing them.
        let wait = unsafe { WaitForMultipleObjects(&handles, false, INFINITE) };
        if wait == WAIT_OBJECT_0 {
            break;
        }
        if !dev.on_sc26_keepalive_tick() {
            break;
        }
    }

    // SAFETY: `handles[1]` is our live waitable timer.
    unsafe {
        let _ = CancelWaitableTimer(handles[1]);
    }
}
