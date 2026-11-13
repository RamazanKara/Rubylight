//! What a game sees of the host's mouse input, and how long it takes to get there.
//!
//! `input_latency_probe watch [seconds] [out.csv]` (default 30 s, `input-latency.csv`) opens
//! a hidden window that takes raw mouse input in the background, as games do, and a low-level
//! mouse hook. Each WM_INPUT is written with its QueryPerformanceCounter time, the device that
//! sent it (none for SendInput's injected input; `VID_1209&PID_52B1` for the virtual mouse)
//! and its motion; each hook event with whether Windows flagged it injected (LLMHF_INJECTED).
//! At the end it prints the devices seen and the injected share.
//!
//! `input_latency_probe pair <host.log> <out.csv>` pairs the host's `input_timing` trace
//! (run the host with `RUST_LOG=info,input_timing=trace`: each relative move as it reaches the
//! injector, with its QPC time) with the WM_INPUT that carried the same motion, and prints the
//! host-side delay from a client's input arriving to the game's WM_INPUT: p50, p95 and p99.
//!
//! Move the client's mouse in small strokes while it watches. Both sides read the same QPC
//! clock, so the probe must run on the host PC, in the user's session.
use std::{
    collections::{BTreeMap, VecDeque},
    io::Write,
    sync::Mutex,
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::{HANDLE, HWND, LPARAM, LRESULT, WPARAM},
        System::{LibraryLoader::GetModuleHandleW, Performance::*},
        UI::{Input::*, WindowsAndMessaging::*},
    },
    core::{PCWSTR, w},
};

static ROWS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static NAMES: Mutex<BTreeMap<isize, String>> = Mutex::new(BTreeMap::new());

fn qpc() -> i64 {
    let mut now = 0;
    // SAFETY: QueryPerformanceCounter writes only to the live local.
    let _ = unsafe { QueryPerformanceCounter(&mut now) };
    now
}

fn frequency() -> i64 {
    let mut f = 0;
    // SAFETY: QueryPerformanceFrequency writes only to the live local.
    let _ = unsafe { QueryPerformanceFrequency(&mut f) };
    f.max(1)
}

/// The device's interface name, `injected` for none.
fn device_name(device: HANDLE) -> String {
    if device.is_invalid() || device.0.is_null() {
        return "injected".into();
    }
    let key = device.0 as isize;
    if let Some(name) = NAMES.lock().unwrap().get(&key) {
        return name.clone();
    }
    let mut size = 0u32;
    // SAFETY: a null buffer asks for the name's length in characters.
    unsafe { GetRawInputDeviceInfoW(Some(device), RIDI_DEVICENAME, None, &mut size) };
    let mut buffer = vec![0u16; size as usize + 1];
    // SAFETY: the buffer holds `size` characters.
    let read = unsafe {
        GetRawInputDeviceInfoW(
            Some(device),
            RIDI_DEVICENAME,
            Some(buffer.as_mut_ptr().cast()),
            &mut size,
        )
    };
    let name = if read == u32::MAX {
        format!("device {key:#x}")
    } else {
        String::from_utf16_lossy(&buffer[..read as usize])
            .trim_end_matches('\0')
            .to_owned()
    };
    NAMES.lock().unwrap().insert(key, name.clone());
    name
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_INPUT {
        let now = qpc();
        let mut input = RAWINPUT::default();
        let mut size = size_of::<RAWINPUT>() as u32;
        // SAFETY: the buffer is a RAWINPUT, large enough for a mouse's.
        let read = unsafe {
            GetRawInputData(
                HRAWINPUT(lparam.0 as _),
                RID_INPUT,
                Some((&mut input as *mut RAWINPUT).cast()),
                &mut size,
                size_of::<RAWINPUTHEADER>() as u32,
            )
        };
        if read != u32::MAX && input.header.dwType == RIM_TYPEMOUSE.0 {
            // SAFETY: a mouse's raw input holds RAWMOUSE.
            let mouse = unsafe { input.data.mouse };
            let name = device_name(input.header.hDevice);
            ROWS.lock().unwrap().push(format!(
                "wm_input,{now},{},{},{},{:#x}",
                name.replace(',', ";"),
                mouse.lLastX,
                mouse.lLastY,
                mouse.usFlags.0
            ));
        }
    }
    // SAFETY: the default procedure for the probe's own window.
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

unsafe extern "system" fn hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        // SAFETY: a low-level mouse hook's lParam points at MSLLHOOKSTRUCT.
        let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        ROWS.lock().unwrap().push(format!(
            "ll,{},{},{},{},{:#x}",
            qpc(),
            u8::from(info.flags & LLMHF_INJECTED != 0),
            info.pt.x,
            info.pt.y,
            wparam.0
        ));
    }
    // SAFETY: passes the event on along the hook chain.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

fn watch(seconds: u64, out: &str) -> windows::core::Result<()> {
    // SAFETY: the window class, window, raw input registration and hook are the probe's own
    // and live until it returns.
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            lpszClassName: w!("RubylightInputLatencyProbe"),
            ..Default::default()
        };
        RegisterClassW(&class);
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("RubylightInputLatencyProbe"),
            PCWSTR::null(),
            WINDOW_STYLE::default(),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(instance.into()),
            None,
        )?;
        let device = RAWINPUTDEVICE {
            usUsagePage: 0x01,
            usUsage: 0x02,
            dwFlags: RIDEV_INPUTSINK,
            hwndTarget: hwnd,
        };
        RegisterRawInputDevices(&[device], size_of::<RAWINPUTDEVICE>() as u32)?;
        let mouse_hook = SetWindowsHookExW(WH_MOUSE_LL, Some(hook), Some(instance.into()), 0)?;
        println!("watching raw mouse input for {seconds} s; move the client's mouse");
        let end = Instant::now() + Duration::from_secs(seconds);
        let mut message = MSG::default();
        while Instant::now() < end {
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            let _ = MsgWaitForMultipleObjects(None, false, 10, QS_ALLINPUT);
        }
        let _ = UnhookWindowsHookEx(mouse_hook);
        let _ = DestroyWindow(hwnd);
    }
    let rows = std::mem::take(&mut *ROWS.lock().unwrap());
    let mut file = std::fs::File::create(out).expect("creating the output file");
    writeln!(file, "kind,qpc,device_or_injected,x,y,flags").unwrap();
    let mut devices: BTreeMap<String, usize> = BTreeMap::new();
    let (mut hooked, mut injected) = (0usize, 0usize);
    for row in &rows {
        writeln!(file, "{row}").unwrap();
        let fields: Vec<&str> = row.split(',').collect();
        match fields[0] {
            "wm_input" => *devices.entry(fields[2].to_owned()).or_default() += 1,
            _ => {
                hooked += 1;
                injected += usize::from(fields[2] == "1");
            }
        }
    }
    println!("WM_INPUT by device:");
    for (device, count) in &devices {
        println!("  {count:6}  {device}");
    }
    println!("low-level hook: {hooked} events, {injected} flagged injected (LLMHF_INJECTED)");
    println!("rows written to {out}; frequency {} Hz", frequency());
    Ok(())
}

/// `qpc=`, `x=` and `y=` of an `input_timing` "arrived" line of the host log.
fn arrival(line: &str) -> Option<(i64, i32, i32)> {
    if !line.contains("input_timing") || !line.contains("arrived") {
        return None;
    }
    let field = |name: &str| {
        line.split_whitespace()
            .find_map(|word| word.strip_prefix(name))
            .and_then(|v| v.trim_end_matches(',').parse::<i64>().ok())
    };
    Some((field("qpc=")?, field("x=")? as i32, field("y=")? as i32))
}

/// Delays in microseconds from each arrival to the first later WM_INPUT with its motion,
/// within 100 ms. The host merges moves of one pass, so an arrival may match a WM_INPUT
/// carrying the sum of several: those are skipped, not guessed.
fn pair(arrivals: &[(i64, i32, i32)], inputs: &[(i64, i32, i32)], frequency: i64) -> Vec<f64> {
    let window = frequency / 10;
    let mut pending: VecDeque<(i64, i32, i32)> = inputs.iter().copied().collect();
    let mut delays = Vec::new();
    for &(at, x, y) in arrivals {
        while pending.front().is_some_and(|&(t, ..)| t < at) {
            pending.pop_front();
        }
        if let Some(index) = pending
            .iter()
            .take_while(|&&(t, ..)| t - at <= window)
            .position(|&(_, dx, dy)| (dx, dy) == (x, y))
        {
            let (t, ..) = pending.remove(index).unwrap();
            delays.push((t - at) as f64 * 1e6 / frequency as f64);
        }
    }
    delays
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

fn report(log: &str, csv: &str) {
    let log = std::fs::read_to_string(log).expect("reading the host log");
    let arrivals: Vec<_> = log.lines().filter_map(arrival).collect();
    let csv = std::fs::read_to_string(csv).expect("reading the probe's rows");
    let mut by_device: BTreeMap<String, Vec<(i64, i32, i32)>> = BTreeMap::new();
    for line in csv.lines().skip(1) {
        let f: Vec<&str> = line.split(',').collect();
        if f.len() >= 5 && f[0] == "wm_input" {
            let (Ok(t), Ok(x), Ok(y)) = (f[1].parse(), f[3].parse(), f[4].parse()) else {
                continue;
            };
            by_device
                .entry(f[2].to_owned())
                .or_default()
                .push((t, x, y));
        }
    }
    println!("{} host arrivals", arrivals.len());
    for (device, inputs) in by_device {
        let mut delays = pair(&arrivals, &inputs, frequency());
        delays.sort_by(f64::total_cmp);
        println!(
            "{device}: {} paired of {} WM_INPUT; arrival to WM_INPUT p50 {:.0} us, p95 {:.0} us, p99 {:.0} us",
            delays.len(),
            inputs.len(),
            percentile(&delays, 0.5),
            percentile(&delays, 0.95),
            percentile(&delays, 0.99)
        );
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("pair") if args.len() == 3 => report(&args[1], &args[2]),
        Some("watch") | None => {
            let seconds = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(30);
            let out = args.get(2).map_or("input-latency.csv", String::as_str);
            if let Err(error) = watch(seconds, out) {
                eprintln!("input_latency_probe: {error}");
                std::process::exit(1);
            }
        }
        _ => {
            eprintln!(
                "usage: input_latency_probe watch [seconds] [out.csv] | pair <host.log> <out.csv>"
            );
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arrivals_pair_with_the_wm_input_of_the_same_motion() {
        let line = "2026-11-13T01:02:03.456Z TRACE input_timing: arrived qpc=1000 x=3 y=-2";
        assert_eq!(arrival(line), Some((1000, 3, -2)));
        assert_eq!(arrival("INFO other: arrived qpc=1 x=1 y=1"), None);
        let frequency = 1_000_000;
        let arrivals = [(1000, 3, -2), (2000, 1, 1), (900_000, 5, 5)];
        let inputs = [
            (1300, 3, -2),
            (2100, 9, 9),
            (2500, 1, 1),
            (900_000 + 200_000, 5, 5),
        ];
        let delays = pair(&arrivals, &inputs, frequency);
        assert_eq!(
            delays,
            [300.0, 500.0],
            "the last is past 100 ms and stays unpaired"
        );
        assert_eq!(percentile(&[1.0, 2.0, 3.0], 0.5), 2.0);
    }
}
