//! Keeps a background stream's game windows on the stream's display without
//! activating them, so the person using the PC keeps their focus.
use butterpollo_core::background::{Rect, placement};
use std::{
    collections::{BTreeMap, BTreeSet},
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
};
use windows::{
    Win32::{
        Foundation::*, Graphics::Dwm::*, Graphics::Gdi::*, System::Threading::*,
        UI::WindowsAndMessaging::*,
    },
    core::BOOL,
};

/// How often one window is moved back before it is left alone: a game that
/// returns itself to the primary monitor on every frame or mode change must
/// not be fought for the whole stream.
const MOVE_LIMIT: u32 = 5;

#[derive(Default)]
pub struct Keeper {
    moves: BTreeMap<(isize, u32), u32>,
    /// Processes whose background throttling is already off.
    unthrottled: BTreeMap<u32, (OwnedHandle, PROCESS_POWER_THROTTLING_STATE)>,
}

impl Drop for Keeper {
    fn drop(&mut self) {
        for (process, previous) in self.unthrottled.values() {
            restore_throttle(process, previous);
        }
    }
}

/// What one pass did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Pass {
    /// Visible windows of the game, wherever they are.
    pub windows: usize,
    pub moved: usize,
    /// Windows not yet observed on the target, including moves still queued.
    pub pending: usize,
    /// Windows that kept leaving the stream's display and are left alone.
    pub given_up: usize,
    /// The game's largest window already on the target and its process.
    pub main: Option<(isize, u32)>,
    /// The game held a cursor clip that reached the stream's display, which
    /// would trap the PC user's mouse; it was released.
    pub released_clip: bool,
}

struct Candidate {
    window: HWND,
    pid: u32,
    rect: Rect,
    monitor: Rect,
    maximized: bool,
}

impl Keeper {
    /// Move every visible top-level window of `processes` that is off
    /// `target` onto it.
    pub fn pass(&mut self, processes: &BTreeSet<u32>, target: Rect) -> Pass {
        crate::input::follow_input_desktop();
        self.unthrottled.retain(|pid, (process, previous)| {
            if processes.contains(pid) {
                true
            } else {
                restore_throttle(process, previous);
                false
            }
        });
        for &pid in processes {
            if let std::collections::btree_map::Entry::Vacant(entry) = self.unthrottled.entry(pid)
                && let Some(previous) = unthrottle(pid)
            {
                entry.insert(previous);
            }
        }
        let candidates = windows_of(processes);
        let mut pass = Pass {
            windows: candidates.len(),
            main: candidates
                .iter()
                .filter(|c| placement(c.rect, c.monitor, target).is_none())
                .max_by_key(|c| i64::from(c.rect.width) * i64::from(c.rect.height))
                .map(|c| (c.window.0 as isize, c.pid)),
            released_clip: release_clip(target),
            ..Default::default()
        };
        let live: BTreeSet<_> = candidates
            .iter()
            .map(|c| (c.window.0 as isize, c.pid))
            .collect();
        self.moves.retain(|window, _| live.contains(window));
        for candidate in candidates {
            let Some(to) = placement(candidate.rect, candidate.monitor, target) else {
                continue;
            };
            pass.pending += 1;
            let count = self
                .moves
                .entry((candidate.window.0 as isize, candidate.pid))
                .or_default();
            if *count >= MOVE_LIMIT {
                pass.given_up += 1;
                continue;
            }
            *count += 1;
            if move_window(&candidate, to) {
                pass.moved += 1;
            }
        }
        pass
    }
}

/// Tell `window` it is the active window, though another program keeps the
/// real focus. Many games mute or slow down while inactive and decide that
/// from these messages; nothing changes for the window that has focus. Does
/// nothing while the game really is in front.
pub fn fake_focus(window: isize, processes: &BTreeSet<u32>, target: Rect) {
    let window = HWND(window as *mut _);
    // SAFETY: the handles are plain values; a closed window makes the calls
    // fail harmlessly, and the timeouts never wait on a hung game for long.
    unsafe {
        let foreground = GetForegroundWindow();
        let mut owner = 0;
        let thread = GetWindowThreadProcessId(foreground, Some(&mut owner));
        if processes.contains(&owner) {
            return;
        }
        for (message, wparam, lparam) in [
            (WM_ACTIVATEAPP, 1, thread as isize),
            (WM_NCACTIVATE, 1, 0),
            (WM_ACTIVATE, WA_ACTIVE as usize, 0),
            (WM_SETFOCUS, 0, 0),
        ] {
            let _ = SendMessageTimeoutW(
                window,
                message,
                WPARAM(wparam),
                LPARAM(lparam),
                SMTO_ABORTIFHUNG | SMTO_NORMAL,
                200,
                None,
            );
        }
    }
    // Activation messages can make the game clip the cursor immediately.
    // Release it before yielding rather than trapping the local user until
    // the next window-placement pass.
    release_clip(target);
}

/// A game that believes it is active may clip the cursor to its window. On
/// the isolated display that would trap the PC user's mouse there.
fn release_clip(target: Rect) -> bool {
    // SAFETY: plain out-pointers to locals; ClipCursor(None) frees the cursor.
    unsafe {
        let mut clip = RECT::default();
        if GetClipCursor(&mut clip).is_err() {
            return false;
        }
        let reaches = clip.left < target.x + target.width
            && clip.right > target.x
            && clip.top < target.y + target.height
            && clip.bottom > target.y;
        let whole = clip.left <= GetSystemMetrics(SM_XVIRTUALSCREEN)
            && clip.top <= GetSystemMetrics(SM_YVIRTUALSCREEN)
            && clip.right
                >= GetSystemMetrics(SM_XVIRTUALSCREEN) + GetSystemMetrics(SM_CXVIRTUALSCREEN)
            && clip.bottom
                >= GetSystemMetrics(SM_YVIRTUALSCREEN) + GetSystemMetrics(SM_CYVIRTUALSCREEN);
        reaches && !whole && ClipCursor(None).is_ok()
    }
}

/// Windows 11 throttles processes it considers in the background (EcoQoS,
/// coarse timers); a background stream's game is in the background by design.
pub(crate) fn unthrottle(pid: u32) -> Option<(OwnedHandle, PROCESS_POWER_THROTTLING_STATE)> {
    // SAFETY: the newly opened handle has one owner; both structures are
    // live locals of the sizes passed to Windows.
    unsafe {
        let process = OwnedHandle::from_raw_handle(
            OpenProcess(
                PROCESS_SET_INFORMATION | PROCESS_QUERY_LIMITED_INFORMATION,
                false,
                pid,
            )
            .ok()?
            .0,
        );
        let handle = HANDLE(process.as_raw_handle());
        let mut previous = PROCESS_POWER_THROTTLING_STATE {
            Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            ..Default::default()
        };
        GetProcessInformation(
            handle,
            ProcessPowerThrottling,
            (&mut previous as *mut PROCESS_POWER_THROTTLING_STATE).cast(),
            std::mem::size_of_val(&previous) as u32,
        )
        .ok()?;
        let state = PROCESS_POWER_THROTTLING_STATE {
            Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED
                | PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION,
            StateMask: 0,
        };
        SetProcessInformation(
            handle,
            ProcessPowerThrottling,
            (&state as *const PROCESS_POWER_THROTTLING_STATE).cast(),
            std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        )
        .ok()?;
        Some((process, previous))
    }
}

pub(crate) fn restore_throttle(process: &OwnedHandle, previous: &PROCESS_POWER_THROTTLING_STATE) {
    let mask =
        PROCESS_POWER_THROTTLING_EXECUTION_SPEED | PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION;
    // SAFETY: the held process handle prevents PID reuse; both state
    // structures live through the calls and have the declared sizes.
    unsafe {
        let handle = HANDLE(process.as_raw_handle());
        let mut current = PROCESS_POWER_THROTTLING_STATE {
            Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            ..Default::default()
        };
        if GetProcessInformation(
            handle,
            ProcessPowerThrottling,
            (&mut current as *mut PROCESS_POWER_THROTTLING_STATE).cast(),
            std::mem::size_of_val(&current) as u32,
        )
        .is_err()
            || current.ControlMask & mask != mask
            || current.StateMask & mask != 0
        {
            return;
        }
        current.ControlMask = (current.ControlMask & !mask) | (previous.ControlMask & mask);
        current.StateMask = (current.StateMask & !mask) | (previous.StateMask & mask);
        let _ = SetProcessInformation(
            handle,
            ProcessPowerThrottling,
            (&current as *const PROCESS_POWER_THROTTLING_STATE).cast(),
            std::mem::size_of_val(&current) as u32,
        );
    }
}

fn windows_of(processes: &BTreeSet<u32>) -> Vec<Candidate> {
    struct Scan<'a> {
        processes: &'a BTreeSet<u32>,
        found: Vec<Candidate>,
        count: usize,
    }
    unsafe extern "system" fn visit(window: HWND, context: LPARAM) -> BOOL {
        // SAFETY: EnumWindows passes the `&mut Scan` from windows_of as `context`, which outlives
        // the enumeration; every out-pointer is a live local of the size passed.
        unsafe {
            let scan = &mut *(context.0 as *mut Scan<'_>);
            scan.count += 1;
            if scan.count > 4096 {
                return false.into();
            }
            let mut pid = 0;
            GetWindowThreadProcessId(window, Some(&mut pid));
            if !scan.processes.contains(&pid)
                || !IsWindowVisible(window).as_bool()
                || IsIconic(window).as_bool()
            {
                return true.into();
            }
            let mut cloaked = 0u32;
            let _ =
                DwmGetWindowAttribute(window, DWMWA_CLOAKED, (&mut cloaked as *mut u32).cast(), 4);
            let ex = GetWindowLongPtrW(window, GWL_EXSTYLE) as u32;
            // Tool windows are tooltips, menus and overlays that follow
            // their owner; owned windows (dialogs) are placed by their owner.
            if cloaked != 0
                || ex & WS_EX_TOOLWINDOW.0 != 0
                || !GetWindow(window, GW_OWNER).unwrap_or_default().is_invalid()
            {
                return true.into();
            }
            let mut rect = RECT::default();
            if GetWindowRect(window, &mut rect).is_err()
                || rect.right - rect.left < 64
                || rect.bottom - rect.top < 64
            {
                return true.into();
            }
            let mut info = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            let monitor = MonitorFromWindow(window, MONITOR_DEFAULTTONEAREST);
            if !GetMonitorInfoW(monitor, &mut info).as_bool() {
                return true.into();
            }
            scan.found.push(Candidate {
                window,
                pid,
                rect: rect_of(rect),
                monitor: rect_of(info.rcMonitor),
                maximized: IsZoomed(window).as_bool(),
            });
            true.into()
        }
    }
    let mut scan = Scan {
        processes,
        found: Vec::new(),
        count: 0,
    };
    if processes.is_empty() {
        return scan.found;
    }
    // SAFETY: `scan` outlives the synchronous EnumWindows call that reads it through `visit`.
    unsafe {
        let _ = EnumWindows(Some(visit), LPARAM((&mut scan as *mut Scan<'_>) as isize));
    }
    scan.found
}

fn rect_of(rect: RECT) -> Rect {
    Rect {
        x: rect.left,
        y: rect.top,
        width: rect.right - rect.left,
        height: rect.bottom - rect.top,
    }
}

fn move_window(candidate: &Candidate, to: Rect) -> bool {
    // SAFETY: the window handle came from EnumWindows; a window that closed
    // since makes these calls fail harmlessly. The asynchronous forms never
    // wait for a hung game's message loop.
    unsafe {
        if candidate.maximized {
            // Maximizing shows and activates the window, so a maximized game
            // is restored without activation and sized to the display instead.
            let _ = ShowWindowAsync(candidate.window, SW_SHOWNOACTIVATE);
        }
        let moved = SetWindowPos(
            candidate.window,
            None,
            to.x,
            to.y,
            to.width,
            to.height,
            SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_ASYNCWINDOWPOS,
        );
        if let Err(error) = &moved {
            tracing::debug!(%error, "background stream window could not be moved");
        }
        moved.is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::{Context, Result};

    #[test]
    #[ignore = "temporarily changes this test process's power throttling"]
    fn background_power_settings_are_restored_without_overwriting_later_changes() -> Result<()> {
        let power = || -> Result<PROCESS_POWER_THROTTLING_STATE> {
            let mut state = PROCESS_POWER_THROTTLING_STATE {
                Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
                ..Default::default()
            };
            // SAFETY: the output is initialized and the pseudo handle belongs to this test process.
            unsafe {
                GetProcessInformation(
                    GetCurrentProcess(),
                    ProcessPowerThrottling,
                    (&mut state as *mut PROCESS_POWER_THROTTLING_STATE).cast(),
                    std::mem::size_of_val(&state) as u32,
                )?;
            }
            Ok(state)
        };
        let set_power = |state: &PROCESS_POWER_THROTTLING_STATE| -> Result<()> {
            // SAFETY: the live input has the declared size; only this test process is changed.
            unsafe {
                SetProcessInformation(
                    GetCurrentProcess(),
                    ProcessPowerThrottling,
                    (state as *const PROCESS_POWER_THROTTLING_STATE).cast(),
                    std::mem::size_of_val(state) as u32,
                )?;
            }
            Ok(())
        };
        let before = power()?;
        let mut keeper = Keeper::default();
        keeper.unthrottled.insert(
            std::process::id(),
            unthrottle(std::process::id()).context("process power opt-out unavailable")?,
        );
        let active = power()?;
        let mask = PROCESS_POWER_THROTTLING_EXECUTION_SPEED
            | PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION;
        assert_eq!(active.ControlMask & mask, mask);
        assert_eq!(active.StateMask & mask, 0);
        drop(keeper);
        let restored = power()?;
        assert_eq!(
            (restored.ControlMask, restored.StateMask),
            (before.ControlMask, before.StateMask)
        );

        let mut keeper = Keeper::default();
        keeper.unthrottled.insert(
            std::process::id(),
            unthrottle(std::process::id()).context("process power opt-out unavailable")?,
        );
        let changed = PROCESS_POWER_THROTTLING_STATE {
            StateMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
            ..active
        };
        set_power(&changed)?;
        drop(keeper);
        let retained = power()?;
        set_power(&before)?;
        assert_eq!(
            (retained.ControlMask, retained.StateMask),
            (changed.ControlMask, changed.StateMask)
        );
        Ok(())
    }
}
