//! Virtual gamepads and touch through the host's injector, as a stream drives
//! them, timed from outside: a stand-in game writes XInput vibration or HID
//! output, and the control loop's side collects the injector's pad reports.
//! Sections (explicit):
//!   state      controller states at 500 Hz until XInputGetState shows each
//!   rate       vibration changing at 500 Hz for 6 s, then a stop: forward
//!              latency, reports, stop latency and any rumble after the stop
//!   burst      two rumble changes within 8 ms ending in a stop, 40 times
//!   dualsense  a lightbar and trigger report, then a rumble-only report, in
//!              one poll on a VHF DualSense, 20 times
//!   pinch      two fingers moving in turn, and one finger alone, injected
//!              into a small window of this probe's: frames and time per pass
//!   release    a click and a long press in absolute mode into that window,
//!              with absolute_left_release_delay 0 and 10: the buttons it saw
//! --pad=vhf|vigem (state, rate, burst; state also vhf_ds5 and usbip_ds5,
//! read from the DualSense's HID input reports), --fast-poll and
//! --driver-host-priority set gamepad_feedback_fast_poll and
//! gamepad_driver_host_priority, --load=none,normal,highest runs state, rate
//! and pinch beside the CPU-bound child of input_path_probe, --trace prints
//! the input module's logs (its `gamepad timings` lines). The probe takes the
//! stream's process setup (HIGH class, 1 ms timer, input thread priority).
//! Pads are plugged on a free XInput slot and removed at the end: run it while
//! no one streams and no other XInput pad is connected. Its reports are read
//! by polling every 100 us, so latencies leave out the control loop's wake.
use anyhow::{Context, Result, ensure};
use butterpollo_core::input::Input;
use butterpollo_windows::{
    capture::Priority,
    input::{Injector, PadReport},
};
use std::{
    io::{Read, Write},
    net::TcpStream,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        Storage::FileSystem::*,
        System::{
            Diagnostics::ToolHelp::*, IO::CancelIoEx, LibraryLoader::GetModuleHandleW, Threading::*,
        },
        UI::{HiDpi::*, Input::Pointer::*, WindowsAndMessaging::*},
    },
    core::{PCWSTR, w},
};

fn stats(name: &str, mut v: Vec<f64>) {
    if v.is_empty() {
        println!("{name:52} no samples");
        return;
    }
    v.sort_by(f64::total_cmp);
    let n = v.len();
    let mean = v.iter().sum::<f64>() / n as f64;
    println!(
        "{name:52} n={n:5} mean {mean:8.1} p50 {:8.1} p90 {:8.1} p99 {:8.1} max {:9.1} us",
        v[n / 2],
        v[n * 90 / 100],
        v[n * 99 / 100],
        v[n - 1]
    );
}

struct Load(Option<std::process::Child>);
impl Load {
    /// input_path_probe's spinners: one thread per logical CPU at the named
    /// thread priority in a normal-class child.
    fn start(kind: &str) -> Self {
        if kind == "none" {
            return Self(None);
        }
        let probe = std::env::current_exe()
            .unwrap()
            .with_file_name("input_path_probe.exe");
        let child = std::process::Command::new(probe)
            .arg(format!("--spin={kind}"))
            .spawn()
            .expect("input_path_probe.exe beside this probe");
        std::thread::sleep(Duration::from_millis(300));
        Self(Some(child))
    }
}
impl Drop for Load {
    fn drop(&mut self) {
        if let Some(c) = &mut self.0 {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

fn idle_host() -> Result<()> {
    let timeout = Duration::from_secs(2);
    let mut stream = TcpStream::connect_timeout(&"127.0.0.1:47989".parse()?, timeout)?;
    stream.set_read_timeout(Some(timeout))?;
    stream
        .write_all(b"GET /serverinfo HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    for name in ["RustHostSessionCount", "RustHostPendingSessionCount"] {
        let value = response
            .split_once(&format!("<{name}>"))
            .and_then(|(_, tail)| tail.split_once(&format!("</{name}>")))
            .map(|(value, _)| value);
        ensure!(value == Some("0"), "host is busy: {name}={value:?}");
    }
    Ok(())
}

struct XInput {
    _library: libloading::Library,
    get: unsafe extern "system" fn(u32, *mut [u16; 8]) -> u32,
    set: unsafe extern "system" fn(u32, *const [u16; 2]) -> u32,
}
impl XInput {
    fn load() -> Result<Self> {
        let system = std::env::var("SystemRoot").context("SystemRoot")?;
        // SAFETY: xinput1_4 from System32 has no loader requirements; the
        // signatures match XInputGetState (16-byte XINPUT_STATE) and
        // XInputSetState (two-u16 XINPUT_VIBRATION), and the library is kept.
        unsafe {
            let library = libloading::Library::new(format!("{system}\\System32\\xinput1_4.dll"))?;
            let get = *library.get(b"XInputGetState\0")?;
            let set = *library.get(b"XInputSetState\0")?;
            Ok(Self {
                _library: library,
                get,
                set,
            })
        }
    }
    /// (packet number, buttons) of a connected pad.
    fn state(&self, index: u32) -> Option<(u32, u16)> {
        let mut state = [0u16; 8];
        // SAFETY: index is in 0..4 and state has XINPUT_STATE's size and alignment.
        (unsafe { (self.get)(index, &mut state) } == 0)
            .then(|| (u32::from(state[0]) | u32::from(state[1]) << 16, state[2]))
    }
    fn connected(&self) -> Vec<u32> {
        (0..4).filter(|&i| self.state(i).is_some()).collect()
    }
    /// XInputSetState's result: 0, or a Win32 error.
    fn vibrate(&self, index: u32, low: u16, high: u16) -> u32 {
        // SAFETY: as in state; the vibration lives through the call.
        unsafe { (self.set)(index, &[low, high]) }
    }
}

/// CPU time of this process's threads with the given name, in ms.
fn thread_cpu_ms(name: &str) -> f64 {
    let mut total = 0u64;
    // SAFETY: the snapshot and thread handles are closed here; the out
    // structures are sized and initialized.
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) else {
            return f64::NAN;
        };
        let mut entry = THREADENTRY32 {
            dwSize: size_of::<THREADENTRY32>() as u32,
            ..Default::default()
        };
        let pid = GetCurrentProcessId();
        let mut more = Thread32First(snapshot, &mut entry).is_ok();
        while more {
            if entry.th32OwnerProcessID == pid
                && let Ok(thread) =
                    OpenThread(THREAD_QUERY_LIMITED_INFORMATION, false, entry.th32ThreadID)
            {
                if let Ok(description) = GetThreadDescription(thread)
                    && description.to_string().unwrap_or_default() == name
                {
                    let (mut c, mut e, mut k, mut u) = Default::default();
                    if GetThreadTimes(thread, &mut c, &mut e, &mut k, &mut u).is_ok() {
                        let ticks = |t: FILETIME| {
                            u64::from(t.dwHighDateTime) << 32 | u64::from(t.dwLowDateTime)
                        };
                        total += ticks(k) + ticks(u);
                    }
                    let _ =
                        windows::Win32::Foundation::LocalFree(Some(HLOCAL(description.0.cast())));
                }
                let _ = CloseHandle(thread);
            }
            more = Thread32Next(snapshot, &mut entry).is_ok();
        }
        let _ = CloseHandle(snapshot);
    }
    total as f64 / 1e4
}

fn controller(buttons: u32) -> Input {
    Input::Controller {
        id: 0,
        active: 1,
        buttons,
        left_trigger: 0,
        right_trigger: 0,
        sticks: [0; 4],
    }
}

/// The injector with the probe's switches, its pad plugged and on XInput.
fn plug(pad: &str, args: &[String], xinput: &XInput) -> Result<(Injector, u32)> {
    idle_host()?;
    ensure!(
        xinput.connected().is_empty(),
        "an XInput pad is already connected; refusing to write vibration"
    );
    let switch = |flag: &str| args.iter().any(|a| a == flag);
    let config = butterpollo_core::config::Config::parse(&format!(
        "gamepad_feedback_fast_poll = {}\ngamepad_driver_host_priority = {}\n",
        switch("--fast-poll"),
        switch("--driver-host-priority")
    ))?;
    let profile = if pad == "vigem" {
        "vigem_x360"
    } else {
        "vhf_xbox"
    };
    let mut injector = Injector::new_options("", profile, &config)?;
    injector.apply(&controller(0))?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let index = loop {
        let found = xinput.connected();
        ensure!(found.len() <= 1, "more than one new XInput pad");
        if let Some(&i) = found.first() {
            break i;
        }
        ensure!(Instant::now() < deadline, "the pad did not reach XInput");
        std::thread::sleep(Duration::from_millis(20));
    };
    // Let PnP, HID class and readers (Steam, GameInput) settle.
    std::thread::sleep(Duration::from_secs(2));
    println!(
        "{profile} on XInput {index}; fast poll {}, driver host priority {}",
        switch("--fast-poll"),
        switch("--driver-host-priority")
    );
    Ok((injector, index))
}

fn unplug(injector: Injector, xinput: &XInput) {
    drop(injector);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !xinput.connected().is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    println!("pad removed: {}", xinput.connected().is_empty());
}

fn spin_until(at: Instant) {
    while Instant::now() < at {
        std::hint::spin_loop();
    }
}

/// Rumble a report carries: Xbox reports (kind 4) start with the low and high
/// motors, little-endian, as HID rumble reports (kind 5) do.
fn rumble(report: &PadReport) -> Option<(u16, u16)> {
    match report {
        PadReport::Feedback { data, .. } if data.len() >= 4 => Some((
            u16::from_le_bytes([data[0], data[1]]),
            u16::from_le_bytes([data[2], data[3]]),
        )),
        _ => None,
    }
}

/// Collect the injector's pad reports with their arrival time until `until`,
/// refreshing every 8 ms as the control loop does.
fn collect(
    injector: &mut Injector,
    until: Instant,
    refresh_at: &mut Instant,
    out: &mut Vec<(Instant, PadReport)>,
) {
    while Instant::now() < until {
        if Instant::now() >= *refresh_at {
            *refresh_at = Instant::now() + Duration::from_millis(8);
            let _ = injector.refresh();
        }
        for report in injector.gamepad_reports() {
            out.push((Instant::now(), report));
        }
        spin_until(Instant::now() + Duration::from_micros(100));
    }
}

const LEVELS: [u16; 15] = [
    0x1000, 0x2000, 0x3000, 0x4000, 0x5000, 0x6000, 0x7000, 0x8000, 0x9000, 0xa000, 0xb000, 0xc000,
    0xd000, 0xe000, 0xf000,
];

fn state_section(pad: &str, args: &[String], loads: &[String]) -> Result<()> {
    println!("== {pad}: controller state to XInputGetState, 500 Hz ==");
    let xinput = XInput::load()?;
    let (mut injector, index) = plug(pad, args, &xinput)?;
    for load in loads {
        let _load = Load::start(load);
        let cpu = thread_cpu_ms("gamepads");
        let (mut latencies, mut missed) = (Vec::new(), 0);
        let mut refresh_at = Instant::now();
        let started = Instant::now();
        for i in 0..1500u32 {
            let buttons = if i % 2 == 0 { 0x1000 } else { 0 }; // A
            let t = Instant::now();
            injector.apply(&controller(buttons))?;
            let deadline = t + Duration::from_millis(500);
            loop {
                if xinput
                    .state(index)
                    .is_some_and(|(_, b)| u32::from(b) & 0x1000 == buttons)
                {
                    latencies.push(t.elapsed().as_secs_f64() * 1e6);
                    break;
                }
                if Instant::now() > deadline {
                    missed += 1;
                    break;
                }
                std::hint::spin_loop();
            }
            if Instant::now() >= refresh_at {
                refresh_at = Instant::now() + Duration::from_millis(8);
                let _ = injector.refresh();
            }
            let _ = injector.gamepad_reports().count();
            spin_until(t + Duration::from_millis(2));
        }
        stats(
            &format!("[{load}] state applied to seen by XInput"),
            latencies,
        );
        println!(
            "[{load}] missed (500 ms) {missed}; gamepads thread CPU {:.1} ms over {:.1} s",
            thread_cpu_ms("gamepads") - cpu,
            started.elapsed().as_secs_f64()
        );
    }
    injector.apply(&controller(0))?;
    unplug(injector, &xinput);
    Ok(())
}

/// The state section for a DualSense (`vhf_ds5`, or `usbip_ds5`: the
/// composite pad on USB/IP), seen in its HID input reports instead of XInput.
fn state_hid_section(profile: &str, args: &[String], loads: &[String]) -> Result<()> {
    use std::sync::atomic::AtomicU32;
    println!("== {profile}: controller state to the DualSense's HID input report, 500 Hz ==");
    idle_host()?;
    let switch = |flag: &str| args.iter().any(|a| a == flag);
    let config = butterpollo_core::config::Config::parse(&format!(
        "gamepad_driver_host_priority = {}\n",
        switch("--driver-host-priority")
    ))?;
    let before = hid_paths()?;
    let mut injector = Injector::new_options("", profile, &config)?;
    injector.apply(&controller(0))?;
    let hid = new_hid(&before, 0x054c, 0x0ce6)?;
    // Cross (Moonlight's A) in input report 0x01, and how many reports came.
    let cross = Arc::new(AtomicU32::new(0));
    let reader = {
        let (cross, hid) = (cross.clone(), hid.0 as usize);
        std::thread::spawn(move || {
            let _p = Priority::input();
            let mut buffer = [0u8; 64];
            loop {
                let mut read = 0;
                // SAFETY: the handle stays open until the device is gone and
                // this read fails; the buffer outlives the call.
                if unsafe {
                    ReadFile(
                        HANDLE(hid as *mut _),
                        Some(&mut buffer),
                        Some(&mut read),
                        None,
                    )
                }
                .is_err()
                {
                    break;
                }
                if buffer[0] == 0x01 && read >= 10 {
                    let reports = (cross.load(Ordering::Acquire) >> 1).wrapping_add(1);
                    cross.store(
                        reports << 1 | u32::from(buffer[8] & 0x20 != 0),
                        Ordering::Release,
                    );
                }
            }
        })
    };
    std::thread::sleep(Duration::from_secs(2));
    println!(
        "{profile}: driver host priority {}",
        switch("--driver-host-priority")
    );
    for load in loads {
        let _load = Load::start(load);
        let cpu = thread_cpu_ms("gamepads");
        let (mut latencies, mut missed) = (Vec::new(), 0);
        let mut refresh_at = Instant::now();
        let reports = cross.load(Ordering::Acquire) >> 1;
        for i in 0..1500u32 {
            let pressed = i % 2 == 0;
            let t = Instant::now();
            injector.apply(&controller(if pressed { 0x1000 } else { 0 }))?;
            let deadline = t + Duration::from_millis(500);
            loop {
                if (cross.load(Ordering::Acquire) & 1 != 0) == pressed {
                    latencies.push(t.elapsed().as_secs_f64() * 1e6);
                    break;
                }
                if Instant::now() > deadline {
                    missed += 1;
                    break;
                }
                std::hint::spin_loop();
            }
            if Instant::now() >= refresh_at {
                refresh_at = Instant::now() + Duration::from_millis(8);
                let _ = injector.refresh();
            }
            let _ = injector.gamepad_reports().count();
            spin_until(t + Duration::from_millis(2));
        }
        stats(
            &format!("[{load}] state applied to seen in the HID report"),
            latencies,
        );
        println!(
            "[{load}] missed (500 ms) {missed}; input reports {}; gamepads thread CPU {:.1} ms",
            (cross.load(Ordering::Acquire) >> 1).wrapping_sub(reports),
            thread_cpu_ms("gamepads") - cpu
        );
    }
    injector.apply(&controller(0))?;
    drop(injector);
    std::thread::sleep(Duration::from_millis(500));
    // SAFETY: the probe owns the handle; the reader's read has failed or
    // fails once it is closed.
    unsafe {
        let _ = CancelIoEx(hid, None);
        let _ = CloseHandle(hid);
    }
    let _ = reader.join();
    Ok(())
}

fn rate_section(pad: &str, args: &[String], loads: &[String]) -> Result<()> {
    println!("== {pad}: vibration changing at 500 Hz for 6 s, then a stop ==");
    let xinput = Arc::new(XInput::load()?);
    let (mut injector, index) = plug(pad, args, &xinput)?;
    // The driver's resolution: VHF Xbox reports keep XInput's value as a
    // percentage, ViGEm's X360 a byte.
    let tolerance = 0x0300;
    for load in loads {
        let _load = Load::start(load);
        let mut reports = Vec::new();
        let mut refresh_at = Instant::now();
        collect(
            &mut injector,
            Instant::now() + Duration::from_millis(100),
            &mut refresh_at,
            &mut reports,
        );
        reports.clear();
        let cpu = thread_cpu_ms("gamepads");
        let done = Arc::new(AtomicBool::new(false));
        let game = {
            let (xinput, done) = (xinput.clone(), done.clone());
            std::thread::spawn(move || {
                let _p = Priority::new();
                let mut writes = Vec::new();
                let mut failed = std::collections::BTreeMap::new();
                let start = Instant::now();
                let mut i = 0usize;
                while start.elapsed() < Duration::from_secs(6) {
                    let at = start + Duration::from_micros(2000 * i as u64);
                    spin_until(at);
                    let (low, high) = (LEVELS[i % 15], LEVELS[(i + 7) % 15]);
                    let result = xinput.vibrate(index, low, high);
                    if result != 0 {
                        *failed.entry(result).or_insert(0u32) += 1;
                    }
                    writes.push((Instant::now(), low, high));
                    i += 1;
                }
                xinput.vibrate(index, 0, 0);
                writes.push((Instant::now(), 0, 0));
                done.store(true, Ordering::Release);
                (writes, failed)
            })
        };
        while !done.load(Ordering::Acquire) {
            collect(
                &mut injector,
                Instant::now() + Duration::from_millis(5),
                &mut refresh_at,
                &mut reports,
            );
        }
        collect(
            &mut injector,
            Instant::now() + Duration::from_millis(500),
            &mut refresh_at,
            &mut reports,
        );
        let cpu = thread_cpu_ms("gamepads") - cpu;
        let (writes, failed) = game.join().unwrap();
        let stop_at = writes.last().unwrap().0;
        let near = |a: u16, b: u16| a.abs_diff(b) <= tolerance;
        let (mut latencies, mut unmatched, mut stop) = (Vec::new(), 0, None);
        let mut after_stop = Vec::new();
        for (at, report) in &reports {
            let Some((low, high)) = rumble(report) else {
                continue;
            };
            if stop.is_some() {
                after_stop.push((low, high));
                continue;
            }
            if (low, high) == (0, 0) && *at >= stop_at {
                stop = Some(at.duration_since(stop_at));
                continue;
            }
            match writes
                .iter()
                .rev()
                .filter(|(w, ..)| w <= at)
                .find(|(_, l, h)| near(*l, low) && near(*h, high))
            {
                Some((w, ..)) => latencies.push(at.duration_since(*w).as_secs_f64() * 1e6),
                None => unmatched += 1,
            }
        }
        let rumbles = reports.iter().filter(|(_, r)| rumble(r).is_some()).count();
        stats(&format!("[{load}] XInputSetState to pad report"), latencies);
        println!(
            "[{load}] writes {} reports {rumbles} unmatched {unmatched}; stop after {}; reports after the stop {} {:?}; gamepads thread CPU {cpu:.1} ms",
            writes.len(),
            stop.map_or("never".to_owned(), |d| format!(
                "{:.2} ms",
                d.as_secs_f64() * 1e3
            )),
            after_stop.len(),
            after_stop.iter().take(4).collect::<Vec<_>>()
        );
        println!(
            "[{load}] XInputSetState failures {failed:?}; XInput pads now {:?}",
            xinput.connected()
        );
    }
    unplug(injector, &xinput);
    Ok(())
}

fn burst_section(pad: &str, args: &[String]) -> Result<()> {
    println!("== {pad}: two rumble changes within 8 ms, then a stop, 40 times ==");
    let xinput = XInput::load()?;
    let (mut injector, index) = plug(pad, args, &xinput)?;
    let mut refresh_at = Instant::now();
    let (mut stops, mut stuck, mut changes_seen) = (Vec::new(), 0, Vec::new());
    for round in 0..40u32 {
        let mut reports = Vec::new();
        let offset = Duration::from_micros(u64::from(round % 8) * 1000);
        collect(
            &mut injector,
            Instant::now() + Duration::from_millis(30) + offset,
            &mut refresh_at,
            &mut reports,
        );
        reports.clear();
        let t = Instant::now();
        xinput.vibrate(index, 0xc000, 0x4000);
        spin_until(t + Duration::from_millis(3));
        xinput.vibrate(index, 0x4000, 0xc000);
        spin_until(t + Duration::from_millis(6));
        xinput.vibrate(index, 0, 0);
        let stop_at = Instant::now();
        collect(
            &mut injector,
            stop_at + Duration::from_millis(100),
            &mut refresh_at,
            &mut reports,
        );
        let rumbles: Vec<_> = reports
            .iter()
            .filter_map(|(at, r)| rumble(r).map(|v| (*at, v)))
            .collect();
        changes_seen.push(rumbles.iter().filter(|(_, v)| *v != (0, 0)).count() as f64);
        match rumbles.last() {
            Some((at, (0, 0))) => {
                stops.push(at.saturating_duration_since(stop_at).as_secs_f64() * 1e6)
            }
            other => {
                stuck += 1;
                println!("round {round}: motors left at {other:?}");
            }
        }
    }
    stats("stop write to stop report", stops);
    stats("non-zero rumble reports per burst (count)", changes_seen);
    println!("bursts ending with the motors running: {stuck}");
    unplug(injector, &xinput);
    Ok(())
}

const HID_INTERFACE: windows::core::GUID =
    windows::core::GUID::from_u128(0x4d1e55b2_f16f_11cf_88cb_001111000030);

fn hid_paths() -> Result<std::collections::BTreeSet<String>> {
    use windows::Win32::Devices::DeviceAndDriverInstallation::*;
    // SAFETY: as native_touch_tests::hid_paths: sized SetupAPI structures and
    // a u64-aligned detail buffer of the queried size; the set is destroyed.
    unsafe {
        let set = SetupDiGetClassDevsW(
            Some(&HID_INTERFACE),
            PCWSTR::null(),
            None,
            DIGCF_DEVICEINTERFACE | DIGCF_PRESENT,
        )?;
        let mut paths = std::collections::BTreeSet::new();
        for index in 0.. {
            let mut interface = SP_DEVICE_INTERFACE_DATA {
                cbSize: size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
                ..Default::default()
            };
            if SetupDiEnumDeviceInterfaces(set, None, &HID_INTERFACE, index, &mut interface)
                .is_err()
            {
                break;
            }
            let mut size = 0;
            let _ =
                SetupDiGetDeviceInterfaceDetailW(set, &interface, None, 0, Some(&mut size), None);
            if !(8..=65536).contains(&size) {
                continue;
            }
            let mut data = vec![0u64; (size as usize).div_ceil(8)];
            let detail = data.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
            (*detail).cbSize = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
            if SetupDiGetDeviceInterfaceDetailW(set, &interface, Some(detail), size, None, None)
                .is_ok()
            {
                paths.insert(
                    PCWSTR((*detail).DevicePath.as_ptr())
                        .to_string()?
                        .to_lowercase(),
                );
            }
        }
        let _ = SetupDiDestroyDeviceInfoList(set);
        Ok(paths)
    }
}

/// A new HID interface with the vendor and product, opened for writing.
fn new_hid(
    before: &std::collections::BTreeSet<String>,
    vendor: u16,
    product: u16,
) -> Result<HANDLE> {
    let system = std::env::var("SystemRoot").context("SystemRoot")?;
    // SAFETY: hid.dll from System32; HidD_GetAttributes takes HIDD_ATTRIBUTES,
    // which [u16; 6] matches in size and alignment (u32 size, then three u16).
    let library = unsafe { libloading::Library::new(format!("{system}\\System32\\hid.dll"))? };
    let get: libloading::Symbol<unsafe extern "system" fn(HANDLE, *mut [u16; 6]) -> u8> =
        unsafe { library.get(b"HidD_GetAttributes\0")? };
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        for path in hid_paths()?.difference(before) {
            let wide: Vec<_> = path.encode_utf16().chain([0]).collect();
            // SAFETY: a terminated path; the handle is closed unless returned.
            unsafe {
                let Ok(handle) = CreateFileW(
                    PCWSTR(wide.as_ptr()),
                    (GENERIC_READ | GENERIC_WRITE).0,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    None,
                    OPEN_EXISTING,
                    FILE_ATTRIBUTE_NORMAL,
                    None,
                ) else {
                    continue;
                };
                let mut attributes = [0u16; 6];
                attributes[0] = 12;
                if get(handle, &mut attributes) != 0
                    && attributes[2] == vendor
                    && attributes[3] == product
                {
                    return Ok(handle);
                }
                let _ = CloseHandle(handle);
            }
        }
        ensure!(
            Instant::now() < deadline,
            "the DualSense did not appear as HID"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn dualsense_section() -> Result<()> {
    println!("== VHF DualSense: lightbar and trigger report, then rumble-only, in one poll ==");
    idle_host()?;
    let before = hid_paths()?;
    let config = butterpollo_core::config::Config::default();
    let mut injector = Injector::new_options("", "vhf_ds5", &config)?;
    injector.apply(&controller(0))?;
    let hid = new_hid(&before, 0x054c, 0x0ce6)?;
    std::thread::sleep(Duration::from_secs(2));
    let mut refresh_at = Instant::now();
    let mut reports = Vec::new();
    collect(
        &mut injector,
        Instant::now() + Duration::from_millis(100),
        &mut refresh_at,
        &mut reports,
    );
    let (mut both, mut counts) = (0, Vec::new());
    for round in 0..20u8 {
        reports.clear();
        let write = |bytes: &[u8]| {
            let mut written = 0;
            // SAFETY: the handle is open for synchronous writes; the buffer
            // outlives the call.
            unsafe { WriteFile(hid, Some(bytes), Some(&mut written), None) }
        };
        // Report 0x02 (USB): flags 0x0c right and left trigger effects, flag
        // 0x04 in the second byte the lightbar; rumble-only: flags 0x03.
        let mut lights = [0u8; 48];
        lights[0] = 0x02;
        lights[1] = 0x0c;
        lights[2] = 0x04;
        lights[11] = 0x01; // right trigger: continuous resistance
        lights[12] = round;
        lights[13] = 0xff;
        lights[22] = 0x01;
        lights[23] = round;
        lights[24] = 0xff;
        lights[45] = 0x60 + round;
        lights[46] = 0x40;
        lights[47] = 0x80;
        let mut motors = [0u8; 48];
        motors[0] = 0x02;
        motors[1] = 0x03;
        motors[3] = 0x20 + round;
        motors[4] = 0x40 + round;
        write(&lights)?;
        write(&motors)?;
        collect(
            &mut injector,
            Instant::now() + Duration::from_millis(60),
            &mut refresh_at,
            &mut reports,
        );
        let feedback: Vec<_> = reports
            .iter()
            .filter_map(|(_, r)| match r {
                PadReport::Feedback { kind, data, .. } => Some((*kind, data.clone())),
                _ => None,
            })
            .collect();
        if round == 0 {
            for (kind, data) in &feedback {
                println!("round 0 report kind {kind:#06x} {data:02x?}");
            }
        }
        counts.push(feedback.len() as f64);
        let lights_seen = feedback.iter().any(|(_, d)| d.contains(&(0x60 + round)));
        let motors_seen = feedback
            .iter()
            .any(|(_, d)| d.contains(&(0x20 + round)) && d.contains(&(0x40 + round)));
        if lights_seen && motors_seen {
            both += 1;
        }
    }
    stats("feedback reports per pair (count)", counts);
    println!("pairs with the lights and the rumble both forwarded: {both}/20");
    // SAFETY: the probe owns the handle.
    unsafe {
        let _ = CloseHandle(hid);
    }
    injector.apply(&controller(0))?;
    drop(injector);
    std::thread::sleep(Duration::from_millis(500));
    Ok(())
}

/// A pointer message the probe's window received: (message, pointer id,
/// frame id, x, y, flags).
type PointerMessage = (u32, u32, u32, i32, i32, u32);
static POINTER: Mutex<Vec<PointerMessage>> = Mutex::new(Vec::new());

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if (WM_POINTERUPDATE..=WM_POINTERUP).contains(&msg) {
        let id = (wparam.0 & 0xffff) as u32;
        let mut info = POINTER_INFO::default();
        // SAFETY: the pointer id came with this message.
        if unsafe { GetPointerInfo(id, &mut info) }.is_ok() {
            POINTER.lock().unwrap().push((
                msg,
                id,
                info.frameId,
                info.ptPixelLocation.x,
                info.ptPixelLocation.y,
                info.pointerFlags.0,
            ));
        }
        return LRESULT(0);
    }
    if matches!(
        msg,
        WM_LBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONDOWN | WM_RBUTTONUP
    ) {
        BUTTONS.lock().unwrap().push((Instant::now(), msg));
        return LRESULT(0);
    }
    if msg == WM_MOUSEACTIVATE {
        return LRESULT(MA_NOACTIVATE as isize);
    }
    if msg == WM_DESTROY {
        // SAFETY: on the window's thread.
        unsafe { PostQuitMessage(0) };
        return LRESULT(0);
    }
    // SAFETY: the arguments came with the message.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Mouse buttons the probe's window received, with their arrival time.
static BUTTONS: Mutex<Vec<(Instant, u32)>> = Mutex::new(Vec::new());

/// A small topmost window in the upper left of the primary display (x 5-35 %,
/// y 10-40 %) that takes the probe's touches and clicks; every point is
/// checked to be on it before anything is injected, so nothing else on the
/// desktop (a dialog in the middle) gets them. It is closed when dropped.
struct ProbeWindow {
    hwnd: usize,
    ui: Option<std::thread::JoinHandle<()>>,
    /// The primary display's name, for the injector.
    name: String,
    monitor: RECT,
}
impl ProbeWindow {
    fn open() -> Result<Self> {
        // SAFETY: plain Win32 calls; the monitor info is sized.
        let (monitor, name) = unsafe {
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            let handle = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
            let mut info = MONITORINFOEXW::default();
            info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
            ensure!(GetMonitorInfoW(handle, (&mut info as *mut MONITORINFOEXW).cast()).as_bool());
            let end = info.szDevice.iter().position(|&c| c == 0).unwrap_or(32);
            (
                info.monitorInfo.rcMonitor,
                String::from_utf16_lossy(&info.szDevice[..end]),
            )
        };
        let (w, h) = (monitor.right - monitor.left, monitor.bottom - monitor.top);
        let (window_tx, window_rx) = std::sync::mpsc::channel();
        let ui = std::thread::spawn(move || {
            // SAFETY: the window class and window live on this thread, which
            // pumps their messages until WM_DESTROY.
            unsafe {
                let instance = GetModuleHandleW(None).unwrap();
                let class = WNDCLASSW {
                    lpfnWndProc: Some(window_proc),
                    hInstance: instance.into(),
                    lpszClassName: w!("PadFeedbackProbeTouch"),
                    hbrBackground: HBRUSH(GetStockObject(DKGRAY_BRUSH).0),
                    ..Default::default()
                };
                RegisterClassW(&class);
                let hwnd = CreateWindowExW(
                    WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                    w!("PadFeedbackProbeTouch"),
                    w!("touch probe"),
                    WS_POPUP | WS_VISIBLE,
                    monitor.left + w * 5 / 100,
                    monitor.top + h * 10 / 100,
                    w * 30 / 100,
                    h * 30 / 100,
                    None,
                    None,
                    Some(instance.into()),
                    None,
                )
                .unwrap();
                window_tx.send(hwnd.0 as usize).unwrap();
                let mut msg = MSG::default();
                while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        });
        let hwnd = window_rx.recv()?;
        std::thread::sleep(Duration::from_millis(500));
        Ok(Self {
            hwnd,
            ui: Some(ui),
            name,
            monitor,
        })
    }
    /// Fails unless each point (normalized on the display) is on this window.
    fn ensure_owns(&self, points: &[(f32, f32)]) -> Result<()> {
        let (w, h) = (
            (self.monitor.right - self.monitor.left) as f32,
            (self.monitor.bottom - self.monitor.top) as f32,
        );
        for &(x, y) in points {
            let point = POINT {
                x: self.monitor.left + (x * w) as i32,
                y: self.monitor.top + (y * h) as i32,
            };
            // SAFETY: plain Win32 calls.
            let root = unsafe { GetAncestor(WindowFromPoint(point), GA_ROOT) };
            ensure!(
                root.0 as usize == self.hwnd,
                "another window covers the probe's window at {x},{y}; nothing injected"
            );
        }
        Ok(())
    }
}
impl Drop for ProbeWindow {
    fn drop(&mut self) {
        // SAFETY: the window belongs to the UI thread; WM_CLOSE destroys it there.
        unsafe {
            let _ = PostMessageW(
                Some(HWND(self.hwnd as *mut _)),
                WM_CLOSE,
                WPARAM(0),
                LPARAM(0),
            );
        }
        if let Some(ui) = self.ui.take() {
            let _ = ui.join();
        }
    }
}

/// absolute_left_release_delay: a click and a long press (left release, then
/// a right press at once, as Moonlight sends it) in absolute mode, into the
/// probe's window. The cursor goes back where it was.
fn release_section() -> Result<()> {
    println!("== absolute mode: click and long press into this probe's window ==");
    let window = ProbeWindow::open()?;
    window.ensure_owns(&[(0.2, 0.25)])?;
    let mut cursor = POINT::default();
    // SAFETY: the point is writable.
    unsafe {
        let _ = GetCursorPos(&mut cursor);
    }
    for delay in [0, 10] {
        let config = butterpollo_core::config::Config::parse(&format!(
            "absolute_left_release_delay = {delay}\n"
        ))?;
        let mut injector = Injector::new_options(&window.name, "vhf", &config)?;
        let button = |button: u8, down: bool| Input::MouseButton { button, down };
        let middle = Input::Absolute {
            x: 200,
            y: 250,
            width: 1000,
            height: 1000,
        };
        for (label, events) in [
            ("click", vec![button(1, true), button(1, false)]),
            (
                "long press",
                vec![
                    button(1, true),
                    button(1, false),
                    button(3, true),
                    button(3, false),
                ],
            ),
        ] {
            for round in 0..5 {
                injector.apply(&middle)?;
                std::thread::sleep(Duration::from_millis(30));
                BUTTONS.lock().unwrap().clear();
                let t = Instant::now();
                // A press, then the rest of the gesture in the next pass.
                injector.apply(&events[0])?;
                injector.apply_all(&events[1..]);
                while t.elapsed() < Duration::from_millis(80) {
                    injector.due()?;
                    spin_until(Instant::now() + Duration::from_micros(200));
                }
                std::thread::sleep(Duration::from_millis(30));
                let seen: Vec<_> = BUTTONS
                    .lock()
                    .unwrap()
                    .iter()
                    .map(|(at, msg)| {
                        let name = match *msg {
                            WM_LBUTTONDOWN => "L down",
                            WM_LBUTTONUP => "L up",
                            WM_RBUTTONDOWN => "R down",
                            _ => "R up",
                        };
                        format!(
                            "{name} +{:.1} ms",
                            at.saturating_duration_since(t).as_secs_f64() * 1e3
                        )
                    })
                    .collect();
                if round < 2 {
                    println!("delay {delay} ms, {label} {round}: {}", seen.join(", "));
                }
            }
        }
        drop(injector);
    }
    // SAFETY: plain Win32 call.
    unsafe {
        let _ = SetCursorPos(cursor.x, cursor.y);
    }
    Ok(())
}

fn pinch_section(loads: &[String]) -> Result<()> {
    println!("== touch: two fingers and one finger, into this probe's window ==");
    let window = ProbeWindow::open()?;
    window.ensure_owns(&[(0.15, 0.25), (0.25, 0.25), (0.19, 0.27), (0.21, 0.27)])?;
    let name = window.name.clone();
    let loads = loads.to_vec();
    let worker = std::thread::spawn(move || -> Result<()> {
        let _p = Priority::input();
        let mut injector = Injector::new(&name, "vhf")?;
        let touch = |event: u8, id: u32, x: f32, y: f32| Input::Touch {
            event,
            id,
            x,
            y,
            pressure: 0.5,
            major: 0.,
            minor: 0.,
            rotation: u16::MAX,
        };
        for load in &loads {
            let _load = Load::start(load);
            for (label, fingers, twice) in [
                ("two fingers, A B per pass", 2u32, false),
                ("two fingers, A B A B per pass", 2, true),
                ("one finger", 1, false),
            ] {
                POINTER.lock().unwrap().clear();
                let start: Vec<_> = (0..fingers)
                    .map(|f| touch(1, 7 + f, 0.15 + 0.1 * f as f32, 0.25))
                    .collect();
                injector.apply_all(&start);
                std::thread::sleep(Duration::from_millis(20));
                let passes = 200;
                let mut times = Vec::new();
                for pass in 0..passes {
                    let step = pass as f32 / passes as f32 * 0.04;
                    let mut events = Vec::new();
                    for _ in 0..if twice { 2 } else { 1 } {
                        for f in 0..fingers {
                            let x = if f == 0 { 0.15 + step } else { 0.25 - step };
                            events.push(touch(3, 7 + f, x, 0.25 + step / 2.));
                        }
                    }
                    let t = Instant::now();
                    for error in injector.apply_all(&events) {
                        println!("touch failed: {error:#}");
                    }
                    times.push(t.elapsed().as_secs_f64() * 1e6);
                    spin_until(t + Duration::from_millis(4));
                }
                let end: Vec<_> = (0..fingers).map(|f| touch(2, 7 + f, 0.2, 0.25)).collect();
                injector.apply_all(&end);
                std::thread::sleep(Duration::from_millis(150));
                let seen = POINTER.lock().unwrap().clone();
                let updates: Vec<_> = seen.iter().filter(|m| m.0 == WM_POINTERUPDATE).collect();
                let frames: std::collections::BTreeSet<_> = updates.iter().map(|m| m.2).collect();
                // What a lone finger's window sees, without ids and frame ids,
                // to compare builds.
                let mut digest = 0xcbf29ce484222325u64;
                for m in &seen {
                    for v in [m.0 as i64, m.3 as i64, m.4 as i64, m.5 as i64] {
                        digest = (digest ^ v as u64).wrapping_mul(0x100000001b3);
                    }
                }
                stats(&format!("[{load}] {label}: apply_all per pass"), times);
                println!(
                    "[{load}] {label}: {} messages, {} updates in {} frames over {passes} passes ({:.2} frames per pass); window digest {digest:016x}",
                    seen.len(),
                    updates.len(),
                    frames.len(),
                    frames.len() as f64 / passes as f64
                );
            }
        }
        Ok(())
    });
    let result = worker.join().unwrap();
    drop(window);
    result
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--trace") {
        tracing_subscriber::fmt()
            .with_env_filter("butterpollo_windows::input=info")
            .init();
    }
    let loads: Vec<String> = args
        .iter()
        .find_map(|a| a.strip_prefix("--load="))
        .unwrap_or("none")
        .split(',')
        .map(str::to_owned)
        .collect();
    let pad = args
        .iter()
        .find_map(|a| a.strip_prefix("--pad="))
        .unwrap_or("vhf")
        .to_owned();
    butterpollo_windows::timing::disable_power_throttling();
    // SAFETY: process-wide settings the stream also makes; undone at exit.
    unsafe {
        let _ = windows::Win32::Media::timeBeginPeriod(1);
        let _ = SetPriorityClass(GetCurrentProcess(), HIGH_PRIORITY_CLASS);
    }
    println!("process: 1 ms timer resolution, HIGH priority class, no power throttling");
    let sections: Vec<String> = args
        .iter()
        .filter(|a| !a.starts_with("--"))
        .cloned()
        .collect();
    let run = std::thread::spawn(move || -> Result<()> {
        let _p = Priority::input();
        for section in &sections {
            match section.as_str() {
                "state" if pad.ends_with("_ds5") => state_hid_section(&pad, &args, &loads)?,
                "state" => state_section(&pad, &args, &loads)?,
                "rate" => rate_section(&pad, &args, &loads)?,
                "burst" => burst_section(&pad, &args)?,
                "dualsense" => dualsense_section()?,
                "pinch" => pinch_section(&loads)?,
                "release" => release_section()?,
                other => anyhow::bail!("unknown section {other}"),
            }
        }
        Ok(())
    });
    let result = run.join().unwrap();
    // SAFETY: pairs timeBeginPeriod above.
    unsafe {
        let _ = windows::Win32::Media::timeEndPeriod(1);
    }
    result
}
