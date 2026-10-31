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
        libloading::Library::new(std::path::PathBuf::from(directory).join("System32/gdi32.dll"))?
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
