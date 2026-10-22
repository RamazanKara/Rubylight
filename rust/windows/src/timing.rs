use anyhow::Result;
use std::{
    sync::atomic::{AtomicU32, Ordering},
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::*,
        System::{Power::*, Threading::*},
    },
    core::PCWSTR,
};

/// Paced waits start with this wake lead and never exceed the maximum.
const PACED_LEAD_INITIAL_US: u32 = 300;
const PACED_LEAD_MAX_US: u32 = 1000;
/// One high-resolution waitable timer per media worker. Packet pacing must not
/// pay the coarse scheduler tick for every UDP datagram. The second field is
/// the measured wake lateness of paced waits, in microseconds.
pub struct Timer(HANDLE, AtomicU32);
/// Each capture consumer owns a separate unnamed event. Resetting it cannot
/// consume another client's notification, and publishing before a wait is safe.
pub struct Signal(HANDLE);
/// Keep capture's display awake, as the C++ host does. The request belongs to
/// this thread and must be restored on the same thread, after capture ends.
pub struct DisplayAwake {
    previous: EXECUTION_STATE,
    _thread: std::marker::PhantomData<std::rc::Rc<()>>,
}
impl DisplayAwake {
    pub fn enter() -> Result<Self> {
        let required = ES_CONTINUOUS | ES_DISPLAY_REQUIRED;
        // SAFETY: SetThreadExecutionState takes only flags by value and affects only this thread.
        let previous = unsafe { SetThreadExecutionState(required) };
        anyhow::ensure!(
            previous.0 != 0,
            "Windows rejected the display-awake request"
        );
        let guard = Self {
            previous,
            _thread: std::marker::PhantomData,
        };
        // Preserve any other requirement already owned by this thread, e.g.
        // a caller keeping the system awake. Drop also handles this error path.
        if previous & !required != EXECUTION_STATE(0) {
            anyhow::ensure!(
                // SAFETY: Flags only, by value; this changes only the calling thread's execution
                // state.
                unsafe { SetThreadExecutionState(previous | required) }.0 != 0,
                "Windows rejected the combined execution-state request"
            );
        }
        Ok(guard)
    }
}
impl Drop for DisplayAwake {
    fn drop(&mut self) {
        // SAFETY: Flags only; the guard is !Send, so this restores the state on the thread that set
        // it.
        unsafe {
            SetThreadExecutionState(self.previous | ES_CONTINUOUS);
        }
    }
}
// Windows event operations are thread-safe; ownership keeps the handle alive.
// SAFETY: Signal owns its event handle until Drop, and event calls may come from any thread.
unsafe impl Send for Signal {}
// SAFETY: SetEvent, ResetEvent and waits are thread-safe kernel calls, and none needs `&mut self`.
unsafe impl Sync for Signal {}
impl Signal {
    pub fn new() -> Result<Self> {
        // SAFETY: Null attributes and name create a new unnamed event; Signal owns the handle and
        // closes it only in Drop.
        Ok(Self(unsafe {
            CreateEventW(None, true, false, PCWSTR::null())?
        }))
    }
    pub fn set(&self) -> Result<()> {
        // SAFETY: `self.0` is the event this Signal owns, open until Drop.
        unsafe {
            SetEvent(self.0)?;
        }
        Ok(())
    }
    /// The event, to hand to another process.
    pub(crate) fn raw(&self) -> HANDLE {
        self.0
    }
    /// Reset while holding the protected capture-image lock, before waiting.
    pub fn reset(&self) -> Result<()> {
        // SAFETY: `self.0` is the event this Signal owns, open until Drop.
        unsafe {
            ResetEvent(self.0)?;
        }
        Ok(())
    }
}
impl Drop for Signal {
    fn drop(&mut self) {
        // SAFETY: `self.0` is owned by this Signal and closed exactly once, here.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
impl Timer {
    pub fn new() -> Result<Self> {
        // SAFETY: Null attributes and name create a new unnamed timer; Timer owns the handle and
        // closes it only in Drop.
        unsafe {
            let timer = CreateWaitableTimerExW(
                None,
                PCWSTR::null(),
                CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                TIMER_ALL_ACCESS.0,
            )
            .or_else(|_| CreateWaitableTimerExW(None, PCWSTR::null(), 0, TIMER_ALL_ACCESS.0))?;
            Ok(Self(timer, AtomicU32::new(PACED_LEAD_INITIAL_US)))
        }
    }
    pub fn until(&self, deadline: Instant) {
        if self.wait_until(deadline, None, arm_timer).is_err() {
            // A failed kernel wait must not turn a busy media worker into a spin loop.
            std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
        }
    }
    /// A deadline wait that never returns early. Packet pacing uses
    /// `until_paced` instead: a yielding tail measured slower than the timer
    /// beside CPU-bound threads (`examples/pace_probe.rs`).
    pub fn until_precise(&self, deadline: Instant) {
        self.until(deadline);
    }
    /// Packet pacing without spinning: arm the timer early by the wake
    /// lateness this timer has measured (an average, at most 1 ms), so wakes
    /// land on the deadline on average. Returns up to that lead early; the
    /// pacer's accumulated deadline keeps the average rate exact, and an early
    /// send earns no extra credit.
    pub fn until_paced(&self, deadline: Instant) {
        let _ = self.wait_paced(deadline, None);
    }
    pub fn until_paced_or_signal(&self, deadline: Instant, signal: &Signal) -> Result<bool> {
        self.wait_paced(deadline, Some(signal))
    }
    fn wait_paced(&self, deadline: Instant, signal: Option<&Signal>) -> Result<bool> {
        let lead = Duration::from_micros(u64::from(self.1.load(Ordering::Relaxed)));
        let Some(target) = deadline.checked_sub(lead) else {
            return Ok(false);
        };
        if target <= Instant::now() {
            return Ok(false);
        }
        if let Some(signal) = signal {
            if self.until_or_signal(target, signal)? {
                return Ok(true);
            }
        } else {
            self.until(target);
        }
        let late = Instant::now().saturating_duration_since(target).as_micros() as u32;
        self.1.store(
            paced_lead(self.1.load(Ordering::Relaxed), late),
            Ordering::Relaxed,
        );
        Ok(false)
    }
    /// Frame claims remain interruptible throughout their timer wait.
    pub fn until_or_signal_precise(&self, deadline: Instant, signal: &Signal) -> Result<bool> {
        self.until_or_signal(deadline, signal)
    }
    /// Wait for capture or the precise encoder/static-frame deadline, without
    /// a coarse condition-variable timeout or a polling/spinning thread.
    pub fn until_or_signal(&self, deadline: Instant, signal: &Signal) -> Result<bool> {
        self.wait_until(deadline, Some(signal), arm_timer)
    }
    fn wait_until(
        &self,
        deadline: Instant,
        signal: Option<&Signal>,
        arm: impl Fn(HANDLE, &i64) -> windows::core::Result<()>,
    ) -> Result<bool> {
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(false);
            }
            // Round up so converting to Windows' 100 ns units never expires early.
            let ticks = -(remaining.as_nanos().div_ceil(100).min(i64::MAX as u128) as i64);
            let timeout = remaining
                .as_nanos()
                .div_ceil(1_000_000)
                .min(u128::from(u32::MAX - 1)) as u32;
            let armed = arm(self.0, &ticks).is_ok();
            // SAFETY: Every handle stays open for its owner's borrow. Signal comes first so a
            // simultaneous deadline and recovery notification preserves the notification.
            let result = unsafe {
                match (armed, signal) {
                    (true, Some(signal)) => {
                        WaitForMultipleObjects(&[signal.0, self.0], false, timeout)
                    }
                    (true, None) => WaitForSingleObject(self.0, timeout),
                    // A timer unavailable after creation must still allow capture/recovery to
                    // interrupt its fallback wait; do not replace this with an ordinary sleep.
                    (false, Some(signal)) => WaitForSingleObject(signal.0, timeout),
                    (false, None) => {
                        std::thread::sleep(remaining);
                        WAIT_TIMEOUT
                    }
                }
            };
            if signal.is_some() && result == WAIT_OBJECT_0 {
                return Ok(true);
            }
            if result != WAIT_TIMEOUT
                && result != WAIT_OBJECT_0
                && !(armed && signal.is_some() && result.0 == WAIT_OBJECT_0.0 + 1)
            {
                return Err(windows::core::Error::from_thread().into());
            }
            // Recheck the monotonic deadline after an early timer or bounded timeout.
        }
    }
}
/// One-eighth exponential average of measured lateness. A single long stall
/// moves the lead by at most 1/8 of the cap.
fn paced_lead(lead: u32, late_us: u32) -> u32 {
    let late = late_us.min(PACED_LEAD_MAX_US);
    ((lead * 7 + late) / 8).min(PACED_LEAD_MAX_US)
}
fn arm_timer(handle: HANDLE, ticks: &i64) -> windows::core::Result<()> {
    // SAFETY: The caller borrows the live timer, ticks outlives the call, and no completion
    // routine is registered.
    unsafe { SetWaitableTimer(handle, ticks, 0, None, None, false) }
}
impl Drop for Timer {
    fn drop(&mut self) {
        // SAFETY: `self.0` is owned by this Timer and closed exactly once, here.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
/// Process-wide scheduling for the lifetime of at least one stream: a 1 ms
/// system timer, high priority class, and no power throttling. Without the
/// priority class, media threads (base 9 at normal class) wait behind a busy
/// game's threads; PyroWave frames that fill most of a period on the wire then
/// fall to about 58 fps at 144 Hz (issue #42, 2.5.6-2.5.8). Without these,
/// driver waits (an AMF output query, a DXGI acquire) and Windows 11 timer
/// coalescing can add a scheduler tick to a frame. Like the C++ host it also
/// schedules the compositor under MMCSS, puts connected Wi-Fi adapters in
/// media streaming mode, and turns on Mouse Keys when no mouse is connected,
/// because Windows otherwise hides the cursor the stream should show.
/// Restored after the last stream.
pub struct StreamingScope(());
#[derive(Default)]
struct Streams {
    count: usize,
    priority: u32,
    power: Option<PROCESS_POWER_THROTTLING_STATE>,
    /// The WLAN client handle that holds media streaming mode; closing it ends it.
    wlan: Option<usize>,
    /// Mouse Keys as they were before the first stream turned them on.
    mouse_keys: Option<windows::Win32::UI::Accessibility::MOUSEKEYS>,
}
static STREAMS: std::sync::Mutex<Streams> = std::sync::Mutex::new(Streams {
    count: 0,
    priority: 0,
    power: None,
    wlan: None,
    mouse_keys: None,
});
/// Mouse Keys on, when Windows has no mouse and would hide the cursor;
/// returns the previous settings to restore.
fn force_cursor() -> Option<windows::Win32::UI::Accessibility::MOUSEKEYS> {
    use windows::Win32::UI::{Accessibility::*, WindowsAndMessaging::*};
    // SAFETY: Both MOUSEKEYS are locals with cbSize set, and they outlive the SystemParametersInfoW
    // calls that read and write them.
    unsafe {
        if GetSystemMetrics(SM_MOUSEPRESENT) != 0 {
            return None;
        }
        let mut previous = MOUSEKEYS {
            cbSize: size_of::<MOUSEKEYS>() as u32,
            ..Default::default()
        };
        let mut enabled = MOUSEKEYS {
            cbSize: size_of::<MOUSEKEYS>() as u32,
            dwFlags: MKF_MOUSEKEYSON | MKF_AVAILABLE,
            iMaxSpeed: 10,
            iTimeToMaxSpeed: 1000,
            ..Default::default()
        };
        let result = SystemParametersInfoW(
            SPI_GETMOUSEKEYS,
            0,
            Some((&mut previous as *mut MOUSEKEYS).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
        .and_then(|()| {
            SystemParametersInfoW(
                SPI_SETMOUSEKEYS,
                0,
                Some((&mut enabled as *mut MOUSEKEYS).cast()),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            )
        });
        match result {
            Ok(()) => {
                tracing::info!(
                    "no mouse connected; Mouse Keys are on while streaming so the cursor shows"
                );
                Some(previous)
            }
            Err(error) => {
                tracing::warn!(%error, "no mouse connected and Mouse Keys could not be turned on; the cursor may stay hidden");
                None
            }
        }
    }
}
/// Media streaming mode on every connected Wi-Fi adapter: fewer background
/// scans, which otherwise cause periodic loss and jitter. The returned WLAN
/// handle keeps it on until it is closed. wlanapi.dll is loaded at run time;
/// systems without it simply skip this.
fn wifi_streaming_mode() -> Option<usize> {
    use windows::Win32::{NetworkManagement::WiFi::*, System::LibraryLoader::*};
    type Open =
        unsafe extern "system" fn(u32, *const core::ffi::c_void, *mut u32, *mut HANDLE) -> u32;
    type Enumerate = unsafe extern "system" fn(
        HANDLE,
        *const core::ffi::c_void,
        *mut *mut WLAN_INTERFACE_INFO_LIST,
    ) -> u32;
    type Set = unsafe extern "system" fn(
        HANDLE,
        *const windows::core::GUID,
        WLAN_INTF_OPCODE,
        u32,
        *const core::ffi::c_void,
        *const core::ffi::c_void,
    ) -> u32;
    type Free = unsafe extern "system" fn(*const core::ffi::c_void);
    type Close = unsafe extern "system" fn(HANDLE, *const core::ffi::c_void) -> u32;
    // SAFETY: Each pointer is resolved from wlanapi.dll, never freed, with its documented
    // signature; `list` is non-null and freed only after its dwNumberOfItems entries are read.
    unsafe {
        let module = LoadLibraryW(windows::core::w!("wlanapi.dll")).ok()?;
        let open: Open =
            std::mem::transmute(GetProcAddress(module, windows::core::s!("WlanOpenHandle"))?);
        let enumerate: Enumerate = std::mem::transmute(GetProcAddress(
            module,
            windows::core::s!("WlanEnumInterfaces"),
        )?);
        let set: Set = std::mem::transmute(GetProcAddress(
            module,
            windows::core::s!("WlanSetInterface"),
        )?);
        let free: Free =
            std::mem::transmute(GetProcAddress(module, windows::core::s!("WlanFreeMemory"))?);
        let close: Close = std::mem::transmute(GetProcAddress(
            module,
            windows::core::s!("WlanCloseHandle"),
        )?);
        let (mut version, mut handle) = (0, HANDLE::default());
        if open(2, std::ptr::null(), &mut version, &mut handle) != 0 {
            return None;
        }
        let mut list = std::ptr::null_mut();
        if enumerate(handle, std::ptr::null(), &mut list) != 0 || list.is_null() {
            close(handle, std::ptr::null());
            return None;
        }
        let interfaces = std::slice::from_raw_parts(
            (&raw const (*list).InterfaceInfo).cast::<WLAN_INTERFACE_INFO>(),
            (*list).dwNumberOfItems as usize,
        );
        let enabled = windows::core::BOOL::from(true);
        let mut any = false;
        for interface in interfaces
            .iter()
            .filter(|i| i.isState == wlan_interface_state_connected)
        {
            if set(
                handle,
                &interface.InterfaceGuid,
                wlan_intf_opcode_media_streaming_mode,
                size_of::<windows::core::BOOL>() as u32,
                (&enabled as *const windows::core::BOOL).cast(),
                std::ptr::null(),
            ) == 0
            {
                any = true;
            }
        }
        free(list.cast());
        if !any {
            close(handle, std::ptr::null());
            return None;
        }
        tracing::info!("Wi-Fi adapter in media streaming mode while streaming");
        Some(handle.0 as usize)
    }
}
fn close_wlan(handle: usize) {
    use windows::Win32::System::LibraryLoader::*;
    type Close = unsafe extern "system" fn(HANDLE, *const core::ffi::c_void) -> u32;
    // SAFETY: wlanapi.dll was loaded and never freed, WlanCloseHandle has this signature, and
    // `handle` came from WlanOpenHandle; STREAMS hands it here only once.
    unsafe {
        if let Ok(module) = GetModuleHandleW(windows::core::w!("wlanapi.dll"))
            && let Some(close) = GetProcAddress(module, windows::core::s!("WlanCloseHandle"))
        {
            let close: Close = std::mem::transmute(close);
            close(HANDLE(handle as *mut _), std::ptr::null());
        }
    }
}
/// QueryPerformanceCounter in milliseconds, the clock the Windows test receiver reads as
/// `clock_ms()`. Zero if the counter is unavailable.
pub fn qpc_ms() -> f64 {
    let (mut ticks, mut frequency) = (0, 0);
    // SAFETY: both calls write only to the live locals.
    let ok = unsafe {
        windows::Win32::System::Performance::QueryPerformanceCounter(&mut ticks).is_ok()
            && windows::Win32::System::Performance::QueryPerformanceFrequency(&mut frequency)
                .is_ok()
    };
    if ok && frequency > 0 {
        ticks as f64 * 1000. / frequency as f64
    } else {
        0.
    }
}
/// Windows 11 otherwise ignores timer requests from a process without a
/// visible window, and may run it on efficiency cores: the host, and the
/// capture helper in the user's session.
pub fn disable_power_throttling() {
    let state = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED
            | PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION,
        StateMask: 0,
    };
    // SAFETY: `state` is an initialised local that outlives the call, the size passed is its size,
    // and GetCurrentProcess returns a pseudo handle.
    if let Err(error) = unsafe {
        SetProcessInformation(
            GetCurrentProcess(),
            ProcessPowerThrottling,
            (&state as *const PROCESS_POWER_THROTTLING_STATE).cast(),
            std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        )
    } {
        tracing::debug!(%error, "power throttling opt-out unavailable");
    }
}
impl StreamingScope {
    pub fn enter() -> Self {
        let mut streams = STREAMS.lock().unwrap();
        if streams.count == 0 {
            // A reconnect may reuse capture after the last stream restored the process priority.
            if let Err(error) = crate::gpu_priority::resume() {
                tracing::debug!(%error, "resuming GPU process scheduling failed");
            }
            // SAFETY: DwmEnableMMCSS takes only a BOOL.
            unsafe {
                let _ = windows::Win32::Graphics::Dwm::DwmEnableMMCSS(true);
            }
            streams.wlan = wifi_streaming_mode();
            if streams.mouse_keys.is_none() {
                streams.mouse_keys = force_cursor();
            }
            // SAFETY: GetCurrentProcess returns a pseudo handle that needs no closing, and the
            // other calls take only plain values.
            unsafe {
                let process = GetCurrentProcess();
                let mut power = PROCESS_POWER_THROTTLING_STATE {
                    Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
                    ..Default::default()
                };
                if GetProcessInformation(
                    process,
                    ProcessPowerThrottling,
                    (&mut power as *mut PROCESS_POWER_THROTTLING_STATE).cast(),
                    size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
                )
                .is_ok()
                {
                    streams.power = Some(power);
                }
                disable_power_throttling();
                let _ = windows::Win32::Media::timeBeginPeriod(1);
                streams.priority = GetPriorityClass(process);
                if let Err(error) = SetPriorityClass(process, HIGH_PRIORITY_CLASS) {
                    tracing::debug!(%error, "high process priority unavailable");
                }
            }
        }
        streams.count += 1;
        Self(())
    }
}
impl Drop for StreamingScope {
    fn drop(&mut self) {
        let mut streams = STREAMS.lock().unwrap();
        streams.count -= 1;
        if streams.count == 0 {
            // SAFETY: The process pseudo handle needs no closing, the priority and power state were
            // saved by enter, and the calls take only plain values.
            unsafe {
                let process = GetCurrentProcess();
                if streams.priority != 0 {
                    let _ = SetPriorityClass(process, PROCESS_CREATION_FLAGS(streams.priority));
                }
                let power = streams
                    .power
                    .take()
                    .unwrap_or(PROCESS_POWER_THROTTLING_STATE {
                        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
                        ..Default::default()
                    });
                if let Err(error) = SetProcessInformation(
                    process,
                    ProcessPowerThrottling,
                    (&power as *const PROCESS_POWER_THROTTLING_STATE).cast(),
                    size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
                ) {
                    tracing::debug!(%error, "restoring process power throttling failed");
                }
                if let Err(error) = crate::gpu_priority::restore() {
                    tracing::debug!(%error, "restoring GPU process scheduling failed");
                }
                let _ = windows::Win32::Media::timeEndPeriod(1);
                let _ = windows::Win32::Graphics::Dwm::DwmEnableMMCSS(false);
            }
            if let Some(handle) = streams.wlan.take() {
                close_wlan(handle);
            }
            // Keep the original settings until they are back, as the C++ host does.
            if let Some(mut previous) = streams.mouse_keys
                // SAFETY: `previous` is a local MOUSEKEYS, its cbSize set by force_cursor, that
                // outlives the call.
                && unsafe {
                    windows::Win32::UI::WindowsAndMessaging::SystemParametersInfoW(
                    windows::Win32::UI::WindowsAndMessaging::SPI_SETMOUSEKEYS,
                    0,
                    Some(
                        (&mut previous as *mut windows::Win32::UI::Accessibility::MOUSEKEYS).cast(),
                    ),
                    windows::Win32::UI::WindowsAndMessaging::SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
                )
                }
                .is_ok()
            {
                streams.mouse_keys = None;
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paced_lead_follows_lateness_and_stays_bounded() {
        assert_eq!(paced_lead(300, 300), 300);
        assert_eq!(paced_lead(0, 800), 100);
        assert_eq!(paced_lead(800, 0), 700);
        // One long stall moves the lead by at most an eighth of the cap.
        assert_eq!(paced_lead(0, u32::MAX), PACED_LEAD_MAX_US / 8);
        let mut lead = 0;
        for _ in 0..200 {
            lead = paced_lead(lead, u32::MAX);
        }
        assert!(lead <= PACED_LEAD_MAX_US);
    }
    #[test]
    fn paced_waits_return_no_earlier_than_their_lead() -> Result<()> {
        let timer = Timer::new()?;
        for _ in 0..20 {
            let deadline = Instant::now() + Duration::from_millis(2);
            timer.until_paced(deadline);
            let lead = Duration::from_micros(u64::from(PACED_LEAD_MAX_US));
            assert!(Instant::now() + lead >= deadline);
        }
        // An expired deadline returns at once.
        let start = Instant::now();
        timer.until_paced(start);
        assert!(start.elapsed() < Duration::from_millis(1));
        Ok(())
    }
    #[test]
    fn interrupted_paced_waits_preserve_the_adaptive_lead() -> Result<()> {
        let timer = Timer::new()?;
        let signal = Signal::new()?;
        for _ in 0..20 {
            let deadline = Instant::now() + Duration::from_millis(2);
            assert!(!timer.until_paced_or_signal(deadline, &signal)?);
            let lead = Duration::from_micros(u64::from(PACED_LEAD_MAX_US));
            assert!(Instant::now() + lead >= deadline);
        }
        let lead = timer.1.load(Ordering::Relaxed);
        signal.set()?;
        let start = Instant::now();
        assert!(timer.until_paced_or_signal(start + Duration::from_secs(2), &signal)?);
        assert!(start.elapsed() < Duration::from_secs(1));
        assert_eq!(timer.1.load(Ordering::Relaxed), lead);
        Ok(())
    }
    #[test]
    fn precise_waits_end_on_the_signal_or_at_the_deadline() -> Result<()> {
        let timer = Timer::new()?;
        let signal = Signal::new()?;
        let deadline = Instant::now() + Duration::from_millis(2);
        assert!(!timer.until_or_signal_precise(deadline, &signal)?);
        assert!(Instant::now() >= deadline);
        signal.set()?;
        let start = Instant::now();
        assert!(timer.until_or_signal_precise(start + Duration::from_secs(2), &signal)?);
        assert!(start.elapsed() < Duration::from_secs(1));
        Ok(())
    }
    #[test]
    fn short_and_expired_deadlines_do_not_return_early() -> Result<()> {
        let timer = Timer::new()?;
        for wait in [
            Duration::ZERO,
            Duration::from_micros(25),
            Duration::from_millis(2),
        ] {
            let deadline = Instant::now() + wait;
            timer.until_precise(deadline);
            assert!(Instant::now() >= deadline);
        }
        Ok(())
    }
    #[test]
    fn unavailable_timer_falls_back_to_a_deadline_wait() -> Result<()> {
        let timer = Timer::new()?;
        let deadline = Instant::now() + Duration::from_millis(2);
        assert!(!timer.wait_until(deadline, None, |_, _| {
            Err(windows::core::Error::from_hresult(E_FAIL))
        })?);
        assert!(Instant::now() >= deadline);
        Ok(())
    }
    #[test]
    fn unavailable_timer_still_allows_recovery_to_interrupt() -> Result<()> {
        let timer = Timer::new()?;
        let signal = std::sync::Arc::new(Signal::new()?);
        let publisher = signal.clone();
        let worker = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(2));
            publisher.set().unwrap();
        });
        let notified = timer.wait_until(
            Instant::now() + Duration::from_secs(2),
            Some(&signal),
            |_, _| Err(windows::core::Error::from_hresult(E_FAIL)),
        )?;
        worker.join().unwrap();
        assert!(notified);
        signal.reset()?;
        let deadline = Instant::now() + Duration::from_micros(250);
        assert!(!timer.wait_until(deadline, Some(&signal), |_, _| {
            Err(windows::core::Error::from_hresult(E_FAIL))
        })?);
        assert!(Instant::now() >= deadline);
        Ok(())
    }
    #[test]
    #[ignore = "temporarily keeps the interactive display awake"]
    fn display_awake_preserves_and_restores_thread_power_requirements() -> Result<()> {
        // SAFETY: SetThreadExecutionState takes only flags and affects only this test's thread.
        let previous = unsafe { SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED) };
        anyhow::ensure!(previous.0 != 0, "cannot set test execution state");
        let restore = DisplayAwake {
            previous,
            _thread: std::marker::PhantomData,
        };
        let before = ES_CONTINUOUS | ES_SYSTEM_REQUIRED;
        let required = before | ES_DISPLAY_REQUIRED;
        let first = DisplayAwake::enter()?;
        // SAFETY: SetThreadExecutionState takes only flags and affects only this test's thread.
        assert_eq!(unsafe { SetThreadExecutionState(required) }, required);
        let second = DisplayAwake::enter()?;
        drop(second);
        // SAFETY: SetThreadExecutionState takes only flags and affects only this test's thread.
        assert_eq!(unsafe { SetThreadExecutionState(required) }, required);
        drop(first);
        // SAFETY: SetThreadExecutionState takes only flags and affects only this test's thread.
        assert_eq!(unsafe { SetThreadExecutionState(before) }, before);
        drop(restore);
        Ok(())
    }
    #[test]
    #[ignore = "requires a GPU and briefly changes process scheduling and Wi-Fi streaming mode"]
    fn native_streaming_scope_is_restored_after_the_last_stream() -> Result<()> {
        let power = || -> Result<(u32, u32)> {
            let mut state = PROCESS_POWER_THROTTLING_STATE {
                Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
                ..Default::default()
            };
            // SAFETY: The process pseudo handle and initialized output with its exact size are valid.
            unsafe {
                GetProcessInformation(
                    GetCurrentProcess(),
                    ProcessPowerThrottling,
                    (&mut state as *mut PROCESS_POWER_THROTTLING_STATE).cast(),
                    size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
                )?;
            }
            Ok((state.ControlMask, state.StateMask))
        };
        let directory = std::env::var_os("SystemRoot").unwrap();
        // SAFETY: The absolute system library and the documented export signature are used;
        // the library outlives the symbol and every call.
        let library = unsafe {
            libloading::Library::new(
                std::path::PathBuf::from(directory).join("System32/gdi32.dll"),
            )?
        };
        // SAFETY: The optional export has the documented D3DKMT query signature.
        let get = unsafe {
            library
                .get::<unsafe extern "system" fn(*mut core::ffi::c_void, *mut i32) -> i32>(
                    b"D3DKMTGetProcessSchedulingPriorityClass\0",
                )
                .ok()
        };
        let gpu_priority = || {
            let get = get.as_ref()?;
            let mut priority = 0;
            // SAFETY: The symbol has the documented signature; priority remains live for the call.
            (unsafe { get(GetCurrentProcess().0, &mut priority) } >= 0).then_some(priority)
        };
        let original_power = power()?;
        let original_gpu = gpu_priority();
        // SAFETY: The pseudo handle is always valid for this process.
        let original_priority = unsafe { GetPriorityClass(GetCurrentProcess()) };
        let first = StreamingScope::enter();
        // SAFETY: The pseudo handle is always valid for this process.
        let active_priority = unsafe { GetPriorityClass(GetCurrentProcess()) };
        assert_eq!(active_priority, HIGH_PRIORITY_CLASS.0);
        let gpu = crate::capture::Device::new("")?;
        crate::gpu_priority::configure(&gpu, &Default::default())?;
        let active_power = power()?;
        let active_gpu = gpu_priority();
        let second = StreamingScope::enter();
        assert_eq!(STREAMS.lock().unwrap().count, 2);
        drop(second);
        assert_eq!(power()?, active_power);
        assert_eq!(gpu_priority(), active_gpu);
        drop(first);
        assert_eq!(power()?, original_power);
        // SAFETY: The pseudo handle is always valid; the last stream restores the original class.
        let restored_priority = unsafe { GetPriorityClass(GetCurrentProcess()) };
        assert_eq!(restored_priority, original_priority);
        if original_gpu.is_some() {
            assert_eq!(gpu_priority(), original_gpu);
        }
        // Capture is still alive, so reconnect must restore its configured GPU priority itself.
        let reconnect = StreamingScope::enter();
        assert_eq!(gpu_priority(), active_gpu);
        drop(reconnect);
        if original_gpu.is_some() {
            assert_eq!(gpu_priority(), original_gpu);
        }
        let streams = STREAMS.lock().unwrap();
        assert_eq!(streams.count, 0);
        assert!(streams.wlan.is_none());
        assert!(streams.mouse_keys.is_none());
        Ok(())
    }
    #[test]
    fn capture_before_wait_is_retained_and_each_consumer_has_its_own_wake() -> Result<()> {
        let timer = Timer::new()?;
        let first = Signal::new()?;
        let second = Signal::new()?;
        first.set()?;
        second.set()?;
        first.reset()?;
        // One consumer resetting its event must leave the other ready.
        assert!(timer.until_or_signal(Instant::now() + Duration::from_secs(1), &second)?);
        let due = Instant::now() + Duration::from_millis(2);
        assert!(!timer.until_or_signal(due, &first)?);
        assert!(Instant::now() >= due);
        // Reusing the same timer must not retain its previous expiration.
        second.reset()?;
        let due = Instant::now() + Duration::from_millis(2);
        assert!(!timer.until_or_signal(due, &second)?);
        assert!(Instant::now() >= due);
        Ok(())
    }
    #[test]
    fn capture_can_interrupt_a_later_static_repeat_deadline() -> Result<()> {
        let timer = Timer::new()?;
        let signal = std::sync::Arc::new(Signal::new()?);
        let publisher = signal.clone();
        let worker = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(2));
            publisher.set().unwrap();
        });
        assert!(timer.until_or_signal(Instant::now() + Duration::from_secs(1), &signal)?);
        worker.join().unwrap();
        Ok(())
    }
}
