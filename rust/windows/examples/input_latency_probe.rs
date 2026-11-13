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
//! injector, with its QPC time, and the route of the pass that delivered it) with the WM_INPUT
//! that carried the same motion on that route (injected moves with injected WM_INPUT, the
//! virtual mouse's with its device), and prints the host-side delay from a client's input
//! arriving to the game's WM_INPUT: p50, p95 and p99.
//!
//! Move the client's mouse in small strokes while it watches. Both sides read the same QPC
//! clock, so the probe must run on the host PC, in the user's session.
use std::{
    collections::BTreeMap,
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

/// The host's route for a pass of moves (`route=` of an `input_timing` "delivered" line).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Route {
    Injection,
    VirtualMouse,
}

/// The route whose moves reach the probe as `device`'s WM_INPUT: injected input has no device,
/// the virtual mouse is `VID_1209&PID_52B1`; None for any other (a real mouse), never paired.
fn device_route(device: &str) -> Option<Route> {
    if device == "injected" {
        Some(Route::Injection)
    } else if device.to_ascii_uppercase().contains("VID_1209&PID_52B1") {
        Some(Route::VirtualMouse)
    } else {
        None
    }
}

/// The relative moves of the host log as (qpc, x, y, route): each `input_timing` "arrived" move
/// takes the route of the "delivered" pass after it; moves no pass delivered are left out.
fn arrivals(log: &str) -> Vec<(i64, i32, i32, Route)> {
    let mut out = Vec::new();
    let mut pending = Vec::new();
    for line in log.lines().filter(|line| line.contains("input_timing")) {
        let field = |name: &str| {
            line.split_whitespace()
                .find_map(|word| word.strip_prefix(name))
                .map(|v| v.trim_end_matches(',').trim_matches('"'))
        };
        let number = |name: &str| field(name).and_then(|v| v.parse::<i64>().ok());
        if line.contains(" arrived") {
            if let (Some(at), Some(x), Some(y)) = (number("qpc="), number("x="), number("y=")) {
                pending.push((at, x as i32, y as i32));
            }
        } else if line.contains(" delivered") {
            let route = match field("route=") {
                Some("injection") => Route::Injection,
                Some("virtual_mouse") => Route::VirtualMouse,
                _ => {
                    pending.clear();
                    continue;
                }
            };
            out.extend(pending.drain(..).map(|(at, x, y)| (at, x, y, route)));
        }
    }
    out
}

/// Delays in microseconds from arrival to WM_INPUT: each WM_INPUT takes the latest arrival of
/// its motion on its route not yet taken, within 100 ms before it. Pairing from the WM_INPUT
/// back means an arrival that never reached the probe (a move injected while the virtual mouse
/// was plugged in, two moves merged into one report) stays unpaired instead of shifting every
/// later pair, which with a repeating +1, -1 pattern looked like 80 ms.
fn pair(arrivals: &[(i64, i32, i32)], inputs: &[(i64, i32, i32)], frequency: i64) -> Vec<f64> {
    let window = frequency / 10;
    let mut by_motion: BTreeMap<(i32, i32), Vec<i64>> = BTreeMap::new();
    for &(at, x, y) in arrivals {
        by_motion.entry((x, y)).or_default().push(at);
    }
    for times in by_motion.values_mut() {
        times.sort_unstable();
    }
    let mut taken: BTreeMap<(i32, i32), Vec<bool>> = by_motion
        .iter()
        .map(|(key, times)| (*key, vec![false; times.len()]))
        .collect();
    let mut delays = Vec::new();
    for &(t, x, y) in inputs {
        let (Some(times), Some(used)) = (by_motion.get(&(x, y)), taken.get_mut(&(x, y))) else {
            continue;
        };
        let mut j = times.partition_point(|&at| at <= t);
        while j > 0 && used[j - 1] {
            j -= 1;
        }
        if j > 0 && t - times[j - 1] <= window {
            used[j - 1] = true;
            delays.push((t - times[j - 1]) as f64 * 1e6 / frequency as f64);
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
    let arrivals = arrivals(&log);
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
        let Some(route) = device_route(&device) else {
            println!("{device}: {} WM_INPUT, not the host's", inputs.len());
            continue;
        };
        let mine: Vec<_> = arrivals
            .iter()
            .filter(|a| a.3 == route)
            .map(|&(at, x, y, _)| (at, x, y))
            .collect();
        let mut delays = pair(&mine, &inputs, frequency());
        delays.sort_by(f64::total_cmp);
        println!(
            "{device}: {} paired of {} WM_INPUT and {} arrivals on its route; arrival to WM_INPUT p50 {:.0} us, p95 {:.0} us, p99 {:.0} us",
            delays.len(),
            inputs.len(),
            mine.len(),
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
    fn arrivals_take_the_route_of_the_pass_that_delivered_them() {
        let log = "\
T TRACE input_timing: arrived qpc=1000 x=3 y=-2
T TRACE input_timing: delivered qpc=1010 route=\"injection\" inputs=1
INFO other: arrived qpc=1 x=1 y=1
T TRACE input_timing: arrived qpc=2000 x=1 y=0
T TRACE input_timing: arrived qpc=2001 x=-1 y=0
T TRACE input_timing: delivered qpc=2010 route=virtual_mouse inputs=2
T TRACE input_timing: arrived qpc=3000 x=1 y=0
";
        assert_eq!(
            arrivals(log),
            [
                (1000, 3, -2, Route::Injection),
                (2000, 1, 0, Route::VirtualMouse),
                (2001, -1, 0, Route::VirtualMouse)
            ],
            "the last was never delivered"
        );
        assert_eq!(device_route("injected"), Some(Route::Injection));
        assert_eq!(
            device_route(r"\\?\HID#VID_1209&PID_52B1&Col01#1&2#{378de44c}"),
            Some(Route::VirtualMouse)
        );
        assert_eq!(device_route(r"\\?\HID#VID_046D&PID_C08B#1"), None);
    }

    #[test]
    fn an_arrival_without_a_wm_input_does_not_shift_later_pairs() {
        // +1, -1 every 4 ms with 200 us delay; the second +1 never reached the probe.
        let frequency = 1_000_000;
        let arrivals: Vec<_> = (0..10)
            .map(|i| (i * 4000, if i % 2 == 0 { 1 } else { -1 }, 0))
            .collect();
        let inputs: Vec<_> = arrivals
            .iter()
            .filter(|&&(at, ..)| at != 8000)
            .map(|&(at, x, y)| (at + 200, x, y))
            .collect();
        let delays = pair(&arrivals, &inputs, frequency);
        assert_eq!(delays.len(), 9);
        assert!(delays.iter().all(|&d| d == 200.0), "{delays:?}");
    }

    #[test]
    fn arrivals_pair_with_the_wm_input_of_the_same_motion() {
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
