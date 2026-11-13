//! The PC's own keyboard and mouse, told apart from what streams inject. Raw input from a device
//! names that device; input from `SendInput`, which the host injects for its clients, names none.
//! While a guest holds the keyboard and mouse (`butterpollo_core::multiclient::Kbm`), someone at
//! the PC takes them back by using them.
use anyhow::{Context, Result};
use std::{
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
                GetRawInputData, HRAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER, RID_HEADER,
                RIDEV_INPUTSINK, RegisterRawInputDevices,
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
            let mut message = MSG::default();
            // SAFETY: `message` is a local MSG; GetMessageW returns 0 on WM_QUIT and -1 on error.
            while unsafe { GetMessageW(&mut message, None, 0, 0) }.0 > 0 {
                if message.message == WM_INPUT
                    && from_device(HRAWINPUT(message.lParam.0 as *mut _))
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

/// Whether raw input `input` came from a device: `SendInput`'s carries no device handle.
fn from_device(input: HRAWINPUT) -> bool {
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
    read != u32::MAX && !header.hDevice.is_invalid()
}
