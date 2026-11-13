//! The PC's own keyboard and mouse, told apart from what streams inject. Raw input from a device
//! names that device; input from `SendInput`, which the host injects for its clients, names none.
//! The host's own virtual devices (the USB/IP mouse and precision touchpad of
//! `butterpollo_core::virtual_hid`) do name one, and are a stream's input too, not the PC's.
//! While a guest holds the keyboard and mouse (`butterpollo_core::multiclient::Kbm`), someone at
//! the PC takes them back by using them.
use anyhow::{Context, Result};
use std::{
    collections::HashMap,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, WPARAM},
        System::Threading::GetCurrentThreadId,
        UI::{
            Input::{
                GetRawInputData, GetRawInputDeviceInfoW, HRAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER,
                RID_HEADER, RIDEV_INPUTSINK, RIDI_DEVICENAME, RegisterRawInputDevices,
            },
            WindowsAndMessaging::{
                CreateWindowExW, DestroyWindow, DispatchMessageW, GetMessageW, HWND_MESSAGE, MSG,
                PostThreadMessageW, WINDOW_EX_STYLE, WINDOW_STYLE, WM_INPUT, WM_QUIT,
            },
        },
    },
    core::w,
};

/// The most often `on_input` is called: a moving mouse sends hundreds of events a second.
const NOTICE_EVERY: Duration = Duration::from_millis(100);

/// Watches until dropped.
pub struct Watch {
    thread_id: u32,
    thread: Option<thread::JoinHandle<()>>,
}

impl Drop for Watch {
    fn drop(&mut self) {
        // SAFETY: posts a message to the watcher's own thread, which exits its loop on WM_QUIT.
        let _ = unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Calls `on_input` when someone uses the PC's own keyboard or mouse, at most every
/// [`NOTICE_EVERY`]. Input a program injects never calls it.
pub fn watch(on_input: impl Fn() + Send + 'static) -> Result<Watch> {
    let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<u32, String>>(1);
    let thread = thread::Builder::new()
        .name("local-input".into())
        .spawn(move || {
            let _ = crate::input::follow_input_desktop();
            // SAFETY: a message-only window of the system's STATIC class, owned by this thread and
            // destroyed below on every path.
            let window = match unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    w!("STATIC"),
                    w!("Rubylight local input"),
                    WINDOW_STYLE(0),
                    0,
                    0,
                    0,
                    0,
                    Some(HWND_MESSAGE),
                    None,
                    None,
                    None,
                )
            } {
                Ok(window) => window,
                Err(error) => {
                    let _ = ready_tx.send(Err(format!("creating its window: {error}")));
                    return;
                }
            };
            if let Err(error) = register(window) {
                // SAFETY: the window was created above by this thread.
                let _ = unsafe { DestroyWindow(window) };
                let _ = ready_tx.send(Err(format!("registering for raw input: {error}")));
                return;
            }
            // SAFETY: reads the calling thread's id.
            let _ = ready_tx.send(Ok(unsafe { GetCurrentThreadId() }));
            let mut last: Option<Instant> = None;
            // Whether each device handle seen is the PC's own (not one of the host's).
            let mut known = HashMap::new();
            let mut message = MSG::default();
            // SAFETY: `message` is a local MSG; GetMessageW returns 0 on WM_QUIT and -1 on error.
            while unsafe { GetMessageW(&mut message, None, 0, 0) }.0 > 0 {
                if message.message == WM_INPUT
                    && from_local_device(HRAWINPUT(message.lParam.0 as *mut _), &mut known)
                    && last.is_none_or(|at| at.elapsed() >= NOTICE_EVERY)
                {
                    last = Some(Instant::now());
                    on_input();
                }
                // SAFETY: dispatches the message this loop received; the system frees the raw
                // input's buffer in DefWindowProc.
                unsafe { DispatchMessageW(&message) };
            }
            // SAFETY: the window was created above by this thread.
            let _ = unsafe { DestroyWindow(window) };
        })
        .context("starting the local input watcher")?;
    let thread_id = ready_rx
        .recv_timeout(Duration::from_secs(5))
        .context("the local input watcher did not start")?
        .map_err(anyhow::Error::msg)?;
    Ok(Watch {
        thread_id,
        thread: Some(thread),
    })
}

/// Raw input for the mouse and keyboard, also while another window has the focus.
fn register(window: HWND) -> windows::core::Result<()> {
    let devices = [
        // Generic desktop page: mouse (2) and keyboard (6).
        RAWINPUTDEVICE {
            usUsagePage: 1,
            usUsage: 2,
            dwFlags: RIDEV_INPUTSINK,
            hwndTarget: window,
        },
        RAWINPUTDEVICE {
            usUsagePage: 1,
            usUsage: 6,
            dwFlags: RIDEV_INPUTSINK,
            hwndTarget: window,
        },
    ];
    // SAFETY: the array lives across the call and its element size is passed.
    unsafe { RegisterRawInputDevices(&devices, size_of::<RAWINPUTDEVICE>() as u32) }
}

/// Whether raw input `input` came from one of the PC's own devices: not `SendInput`'s, which
/// carries no device handle, and not the host's virtual mouse or touchpad, which carry a stream's
/// input (a guest's moves counted as the PC's took its keyboard and mouse back every time).
/// `known` caches each handle's answer.
fn from_local_device(input: HRAWINPUT, known: &mut HashMap<isize, bool>) -> bool {
    let Some(device) = device_of(input) else {
        return false;
    };
    // A device plugged in later can be given a handle again; a handful of handles at most.
    if known.len() > 64 {
        known.clear();
    }
    *known
        .entry(device.0 as isize)
        .or_insert_with(|| !hosts_own(&device_name(device)))
}

/// Whether raw input device `name` (its interface path, `\\?\HID#VID_1209&PID_52B1&Col01#...`)
/// is one of the host's own virtual devices.
fn hosts_own(name: &str) -> bool {
    use butterpollo_core::virtual_hid::{MOUSE_PRODUCT, TOUCHPAD_PRODUCT, VENDOR};
    let name = name.to_ascii_uppercase();
    [MOUSE_PRODUCT, TOUCHPAD_PRODUCT]
        .iter()
        .any(|product| name.contains(&format!("VID_{VENDOR:04X}&PID_{product:04X}")))
}

/// The device's interface path, empty when it cannot be read (then counted as the PC's own).
fn device_name(device: windows::Win32::Foundation::HANDLE) -> String {
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
    if read == u32::MAX {
        return String::new();
    }
    String::from_utf16_lossy(&buffer[..(read as usize).min(buffer.len())])
        .trim_end_matches('\0')
        .to_owned()
}

/// The device raw input `input` came from; None for `SendInput`'s, which names none.
fn device_of(input: HRAWINPUT) -> Option<windows::Win32::Foundation::HANDLE> {
    let mut header = RAWINPUTHEADER::default();
    let mut size = size_of::<RAWINPUTHEADER>() as u32;
    // SAFETY: reads only the header into a local of the size passed.
    let read = unsafe {
        GetRawInputData(
            input,
            RID_HEADER,
            Some((&raw mut header).cast()),
            &mut size,
            size_of::<RAWINPUTHEADER>() as u32,
        )
    };
    (read != u32::MAX && !header.hDevice.is_invalid() && !header.hDevice.0.is_null())
        .then_some(header.hDevice)
}

#[cfg(test)]
mod tests {
    use super::hosts_own;

    #[test]
    fn the_hosts_virtual_mouse_and_touchpad_are_not_the_pcs_own() {
        assert!(hosts_own(
            r"\\?\HID#VID_1209&PID_52B1&Col01#1&2d595ca7&0&0000#{378de44c-56ef-11d1-bc8c-00a0c91405dd}"
        ));
        assert!(hosts_own(r"\\?\hid#vid_1209&pid_52b2&col02#1&1#{4d1e55b2}"));
        assert!(!hosts_own(
            r"\\?\HID#VID_046D&PID_C08B&MI_00#7&1#{378de44c}"
        ));
        assert!(!hosts_own(""), "an unreadable name counts as the PC's own");
    }
}
