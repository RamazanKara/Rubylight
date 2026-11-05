//! The PC's real desktop for a Nucleus session: processes, top-level
//! windows, monitors, moving windows without activating them, and the click
//! that loads the host's profile in Nucleus Co-op's window.
use super::instances::{Desktop, MessageBox};
use anyhow::{Result, bail, ensure};
use butterpollo_core::nucleus::window::{Rect, Window};
use butterpollo_core::steam::Process;
use std::collections::{BTreeMap, BTreeSet};
use std::os::windows::io::OwnedHandle;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use windows::Win32::{
    Foundation::*, Graphics::Dwm::*, Graphics::Gdi::*, System::Threading::*,
    UI::WindowsAndMessaging::*,
};
use windows::core::BOOL;

/// EnumWindows stops after this many windows, as the background keeper does.
const WINDOW_LIMIT: usize = 4096;
/// `MK_LBUTTON`: the left button is down.
const LEFT_BUTTON: usize = 0x0001;
/// How long a pass waits for a game window to take its frame's removal,
/// which waits on the game's message loop: a hung game never runs it.
const UNFRAME_WAIT: Duration = Duration::from_millis(250);

/// The desktop of the signed-in user. Dropping it gives every process it
/// unthrottled its power settings back.
#[derive(Default)]
pub struct Win32 {
    unthrottled: BTreeMap<u32, (OwnedHandle, PROCESS_POWER_THROTTLING_STATE)>,
    /// Whether a frame's removal still waits on its window ([`take_frame`]).
    unframing: Arc<AtomicBool>,
}

impl Drop for Win32 {
    fn drop(&mut self) {
        for (process, previous) in self.unthrottled.values() {
            crate::window_placement::restore_throttle(process, previous);
        }
    }
}

fn rect_of(rect: RECT) -> Rect {
    Rect {
        x: rect.left,
        y: rect.top,
        width: rect.right - rect.left,
        height: rect.bottom - rect.top,
    }
}

fn hwnd(id: u64) -> HWND {
    HWND(id as usize as *mut _)
}

/// What a window's frame is made of: its title bar, sizing border and their
/// buttons. A thin border alone is no frame.
const FRAME: WINDOW_STYLE = WINDOW_STYLE(
    WS_CAPTION.0 | WS_THICKFRAME.0 | WS_SYSMENU.0 | WS_MINIMIZEBOX.0 | WS_MAXIMIZEBOX.0,
);

/// Whether a window with `style` has a title bar or a sizing border.
pub(crate) fn framed(style: u32) -> bool {
    style & WS_CAPTION.0 == WS_CAPTION.0 || style & WS_THICKFRAME.0 != 0
}

/// `style` without its frame ([`FRAME`]).
pub(crate) fn unframed(style: u32) -> u32 {
    style & !FRAME.0
}

/// Takes the frame from `window` ([`unframed`]). Windows waits for the
/// window's thread to take the change, without end.
fn take_frame(window: u64) -> Result<()> {
    let window = hwnd(window);
    // SAFETY: the handle is a plain value from EnumWindows; a window that closed since makes the
    // calls fail.
    unsafe {
        let style = GetWindowLongPtrW(window, GWL_STYLE) as u32;
        ensure!(style != 0, "the window closed");
        if !framed(style) {
            return Ok(());
        }
        SetLastError(WIN32_ERROR(0));
        let previous = SetWindowLongPtrW(window, GWL_STYLE, unframed(style) as isize);
        // Zero is also the previous style of none, so the error says.
        if previous == 0 && GetLastError() != WIN32_ERROR(0) {
            return Err(windows::core::Error::from_thread().into());
        }
        // Windows lays the window out again without its frame.
        SetWindowPos(
            window,
            None,
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED
                | SWP_NOMOVE
                | SWP_NOSIZE
                | SWP_NOACTIVATE
                | SWP_NOZORDER
                | SWP_NOOWNERZORDER
                | SWP_ASYNCWINDOWPOS,
        )?;
    }
    Ok(())
}

/// Every top-level window, or only those of `processes` when given.
fn top_level(processes: Option<&BTreeSet<u32>>) -> Result<Vec<Window>> {
    struct Scan<'a> {
        processes: Option<&'a BTreeSet<u32>>,
        found: Vec<Window>,
    }
    unsafe extern "system" fn visit(window: HWND, context: LPARAM) -> BOOL {
        // SAFETY: EnumWindows passes the `&mut Scan` from top_level as `context`, which outlives
        // the enumeration; every out-pointer is a live local of the size passed.
        unsafe {
            let scan = &mut *(context.0 as *mut Scan<'_>);
            if scan.found.len() >= WINDOW_LIMIT {
                return false.into();
            }
            let mut pid = 0;
            GetWindowThreadProcessId(window, Some(&mut pid));
            if scan.processes.is_some_and(|p| !p.contains(&pid)) {
                return true.into();
            }
            let mut rect = RECT::default();
            if GetWindowRect(window, &mut rect).is_err() {
                return true.into();
            }
            let mut cloaked = 0u32;
            let _ =
                DwmGetWindowAttribute(window, DWMWA_CLOAKED, (&mut cloaked as *mut u32).cast(), 4);
            let ex = GetWindowLongPtrW(window, GWL_EXSTYLE) as u32;
            let style = GetWindowLongPtrW(window, GWL_STYLE) as u32;
            let shown = IsWindowVisible(window).as_bool() && cloaked == 0;
            let iconic = IsIconic(window).as_bool();
            scan.found.push(Window {
                id: window.0 as usize as u64,
                pid,
                rect: rect_of(rect),
                // A minimised window shows nothing to capture; a cloaked one
                // (another virtual desktop, a suspended app) neither.
                visible: shown && !iconic,
                minimized: shown && iconic,
                owned: !GetWindow(window, GW_OWNER).unwrap_or_default().is_invalid(),
                tool: ex & WS_EX_TOOLWINDOW.0 != 0,
                framed: framed(style),
            });
            true.into()
        }
    }
    let mut scan = Scan {
        processes,
        found: Vec::new(),
    };
    // SAFETY: `scan` outlives the synchronous EnumWindows call that reads it through `visit`.
    unsafe { EnumWindows(Some(visit), LPARAM((&mut scan as *mut Scan<'_>) as isize))? };
    Ok(scan.found)
}

/// A control's text, whole. Another process's control answers WM_GETTEXT
/// itself (GetWindowText reads only captions there); a hung one is skipped.
fn text_of(window: HWND) -> Option<String> {
    let send = |message: u32, size: usize, buffer: *mut u16| {
        let mut answer = 0usize;
        // SAFETY: a buffer passed outlives the call and `size` is its length in characters; the
        // system copies the text across processes. The timeout never waits long on a hung window.
        let sent = unsafe {
            SendMessageTimeoutW(
                window,
                message,
                WPARAM(size),
                LPARAM(buffer as isize),
                SMTO_ABORTIFHUNG | SMTO_NORMAL,
                200,
                Some(&mut answer),
            )
        };
        (sent.0 != 0).then_some(answer)
    };
    // A message box's text runs to a few hundred characters (OneDrive's
    // folder warning in Nucleus Co-op); the cap only bounds a broken answer.
    let length = send(WM_GETTEXTLENGTH, 0, std::ptr::null_mut())?.min(TEXT_LIMIT);
    let mut buffer = vec![0u16; length + 1];
    let copied = send(WM_GETTEXT, buffer.len(), buffer.as_mut_ptr())?;
    Some(String::from_utf16_lossy(&buffer[..copied.min(length)]))
}

/// Seconds without local input after which nobody uses the PC
/// ([`Desktop::pc_idle`]).
const PC_IDLE_SECONDS: u32 = 10;

/// The longest text read from a control, in characters.
const TEXT_LIMIT: usize = 8192;

/// A window's class name.
fn class_of(window: HWND) -> Option<String> {
    let mut buffer = [0u16; 256];
    // SAFETY: the buffer outlives the call, which writes at most its length.
    let copied = unsafe { GetClassNameW(window, &mut buffer) };
    (copied > 0).then(|| String::from_utf16_lossy(&buffer[..copied as usize]))
}

/// The class of a dialog, which a message box is.
const DIALOG_CLASS: &str = "#32770";
/// How the class of a WinForms window starts (`WindowsForms10.Window.8.app...`).
const FORM_CLASS: &str = "WindowsForms10.";

/// What a WinForms window of another process is called in its program
/// (`WM_GETCONTROLNAME`). The answer is written at the pointer sent, so it
/// takes memory in that process.
fn control_name(window: HWND) -> Option<String> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;
    use windows::Win32::System::Memory::{
        MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAllocEx, VirtualFreeEx,
    };
    const CHARS: usize = 64;
    let mut pid = 0;
    // SAFETY: `pid` is a live u32 and the name a static string; a closed window fails the calls,
    // the handle opened has one owner, and the memory read is the buffer allocated here, as long
    // as it is, into a local of that size.
    unsafe {
        let message = RegisterWindowMessageW(windows::core::w!("WM_GETCONTROLNAME"));
        GetWindowThreadProcessId(window, Some(&mut pid));
        let process = OwnedHandle::from_raw_handle(
            OpenProcess(
                PROCESS_VM_OPERATION | PROCESS_VM_READ | PROCESS_VM_WRITE,
                false,
                pid,
            )
            .ok()?
            .0,
        );
        let process = HANDLE(process.as_raw_handle());
        let remote = VirtualAllocEx(
            process,
            None,
            CHARS * 2,
            MEM_COMMIT | MEM_RESERVE,
            PAGE_READWRITE,
        );
        if remote.is_null() {
            return None;
        }
        let mut written = 0usize;
        let sent = SendMessageTimeoutW(
            window,
            message,
            WPARAM(CHARS),
            LPARAM(remote as isize),
            SMTO_ABORTIFHUNG | SMTO_NORMAL,
            200,
            Some(&mut written),
        );
        // A window that didn't answer in time still will, into this buffer:
        // it stays allocated then, for the life of a process that small.
        if sent.0 == 0 {
            return None;
        }
        let mut name = [0u16; CHARS];
        let read = ReadProcessMemory(process, remote, name.as_mut_ptr().cast(), CHARS * 2, None);
        let _ = VirtualFreeEx(process, remote, 0, MEM_RELEASE);
        read.ok()?;
        // Its length with the terminator; none or more than fits on failure.
        (1..=CHARS)
            .contains(&written)
            .then(|| String::from_utf16_lossy(&name[..written - 1]))
    }
}

/// The visible descendants of `parent`.
fn children(parent: HWND) -> Vec<HWND> {
    unsafe extern "system" fn visit(window: HWND, context: LPARAM) -> BOOL {
        // SAFETY: EnumChildWindows passes the `&mut Vec<HWND>` from children as `context`, which
        // outlives the enumeration.
        unsafe {
            let found = &mut *(context.0 as *mut Vec<HWND>);
            if found.len() >= WINDOW_LIMIT {
                return false.into();
            }
            if IsWindowVisible(window).as_bool() {
                found.push(window);
            }
            true.into()
        }
    }
    let mut found = Vec::new();
    // SAFETY: `found` outlives the synchronous enumeration that fills it through `visit`.
    unsafe {
        let _ = EnumChildWindows(
            Some(parent),
            Some(visit),
            LPARAM((&mut found as *mut Vec<HWND>) as isize),
        );
    }
    found
}

/// How long a raised Nucleus Co-op may take to come above the window over it.
const RAISE_WAIT: std::time::Duration = std::time::Duration::from_secs(1);
/// How [`Raised`] moves a window: in the order only, never activated, and
/// queued so a hung owner never holds the host.
const RAISE_FLAGS: SET_WINDOW_POS_FLAGS = SET_WINDOW_POS_FLAGS(
    SWP_NOMOVE.0 | SWP_NOSIZE.0 | SWP_NOACTIVATE.0 | SWP_NOOWNERZORDER.0 | SWP_ASYNCWINDOWPOS.0,
);

/// A top-level window raised above every other for a click, never
/// activated; dropped, it goes back below the window that was over it.
/// Its owned windows (a box it shows) never move: raised with them, an
/// owner stays below them and out of the topmost windows.
struct Raised {
    window: HWND,
    /// The window just above it before, and whether that one was topmost
    /// then; None at the top.
    below: Option<(HWND, bool)>,
    /// Whether it stayed above the windows that do not before.
    topmost: bool,
}
/// Whether `window` stays above the windows that do not (`WS_EX_TOPMOST`).
///
/// # Safety
/// `window` is any handle; a closed one answers false.
unsafe fn topmost(window: HWND) -> bool {
    // SAFETY: a plain query of a window handle.
    unsafe { GetWindowLongPtrW(window, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST.0 != 0 }
}
impl Raised {
    fn new(window: HWND) -> Result<Self> {
        // SAFETY: plain queries and an asynchronous move of a window handle, which never waits for
        // its owner's message loop; a closed window fails them.
        unsafe {
            let below = GetWindow(window, GW_HWNDPREV)
                .ok()
                .filter(|above| !above.is_invalid())
                .map(|above| (above, topmost(above)));
            let topmost = topmost(window);
            SetWindowPos(window, Some(HWND_TOPMOST), 0, 0, 0, 0, RAISE_FLAGS)?;
            Ok(Self {
                window,
                below,
                topmost,
            })
        }
    }
}
impl Drop for Raised {
    fn drop(&mut self) {
        let flags = RAISE_FLAGS;
        // SAFETY: as in Raised::new.
        unsafe {
            // A window that was not topmost goes to the top of the windows
            // that are not: where it was when only topmost windows were over
            // it. Below a window of its own kind it then takes its place
            // again. The calls are queued in order.
            if !self.topmost
                && let Err(error) =
                    SetWindowPos(self.window, Some(HWND_NOTOPMOST), 0, 0, 0, 0, flags)
            {
                tracing::warn!(%error, "Nucleus Co-op stays above the PC's other windows after the click on its profile entry");
                return;
            }
            let Some((above, above_topmost)) = self.below else {
                return;
            };
            if above_topmost != self.topmost {
                return;
            }
            let back = IsWindow(Some(above)).as_bool()
                && SetWindowPos(self.window, Some(above), 0, 0, 0, 0, flags).is_ok();
            if !back {
                tracing::warn!(
                    "the window over Nucleus Co-op closed during the click on its profile entry, so Nucleus Co-op stays above the PC's other windows"
                );
            }
        }
    }
}

/// Where a click on a label lands, in its client coordinates: near its left
/// edge, where the text starts, and halfway down.
pub(crate) fn click_point(width: i32, height: i32) -> (i32, i32) {
    (4.min(width / 2), height / 2)
}

impl Desktop for Win32 {
    fn processes(&mut self) -> Result<Vec<Process>> {
        crate::process::processes()
    }
    fn image_path(&mut self, pid: u32) -> Option<String> {
        crate::process::image_path(pid)
    }
    fn windows(&mut self) -> Result<Vec<Window>> {
        // A SYSTEM host's thread sees the user's windows only on their desktop.
        crate::input::follow_input_desktop();
        top_level(None)
    }
    fn monitors(&mut self) -> Result<Vec<Rect>> {
        unsafe extern "system" fn visit(
            _monitor: HMONITOR,
            _dc: HDC,
            rect: *mut RECT,
            context: LPARAM,
        ) -> BOOL {
            // SAFETY: EnumDisplayMonitors passes the `&mut Vec<Rect>` from monitors as `context`,
            // which outlives the enumeration, and a valid monitor rectangle.
            unsafe {
                (*(context.0 as *mut Vec<Rect>)).push(rect_of(*rect));
            }
            true.into()
        }
        let mut found: Vec<Rect> = Vec::new();
        // SAFETY: `found` outlives the synchronous enumeration that fills it through `visit`.
        let listed = unsafe {
            EnumDisplayMonitors(
                None,
                None,
                Some(visit),
                LPARAM((&mut found as *mut Vec<Rect>) as isize),
            )
        };
        ensure!(listed.as_bool(), "Windows did not list the PC's monitors");
        Ok(found)
    }
    fn virtual_screen(&mut self) -> Rect {
        // SAFETY: GetSystemMetrics has no preconditions.
        unsafe {
            Rect {
                x: GetSystemMetrics(SM_XVIRTUALSCREEN),
                y: GetSystemMetrics(SM_YVIRTUALSCREEN),
                width: GetSystemMetrics(SM_CXVIRTUALSCREEN),
                height: GetSystemMetrics(SM_CYVIRTUALSCREEN),
            }
        }
    }
    fn place(&mut self, window: u64, to: Rect) -> Result<()> {
        let window = hwnd(window);
        // SAFETY: the handle is a plain value from EnumWindows; a window that closed since makes
        // the calls fail. The asynchronous forms never wait for a hung game's message loop.
        unsafe {
            if IsZoomed(window).as_bool() || IsIconic(window).as_bool() {
                // Maximising shows and activates a window; restore it without activation,
                // as a game that minimised itself when it lost the focus (#56).
                let _ = ShowWindowAsync(window, SW_SHOWNOACTIVATE);
            }
            SetWindowPos(
                window,
                None,
                to.x,
                to.y,
                to.width,
                to.height,
                SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOOWNERZORDER | SWP_ASYNCWINDOWPOS,
            )?;
        }
        Ok(())
    }
    fn unframe(&mut self, window: u64) -> Result<()> {
        let handle = hwnd(window);
        // SAFETY: the handle is a plain value from EnumWindows; a window that closed since makes
        // the calls fail. Neither waits on the window's message loop.
        let style = unsafe {
            ensure!(
                !IsHungAppWindow(handle).as_bool(),
                "the window is not responding"
            );
            GetWindowLongPtrW(handle, GWL_STYLE) as u32
        };
        ensure!(style != 0, "the window closed");
        if !framed(style) {
            return Ok(());
        }
        // The change waits on the game's message loop, so it is made on a
        // thread of its own: a game that hangs then holds that thread, never
        // the pass, and no other change starts until it is taken.
        ensure!(
            !self.unframing.swap(true, Ordering::AcqRel),
            "an earlier window has not yet taken its frame's removal"
        );
        struct Done(Arc<AtomicBool>);
        impl Drop for Done {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Release);
            }
        }
        let done = Done(self.unframing.clone());
        let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("nucleus-unframe".into())
            .spawn(move || {
                let _done = done;
                // As the host's own threads: on the user's desktop.
                crate::input::follow_input_desktop();
                let _ = result_tx.send(take_frame(window));
            })?;
        match result_rx.recv_timeout(UNFRAME_WAIT) {
            Ok(result) => result,
            Err(_) => bail!(
                "the window did not take its frame's removal within {} ms",
                UNFRAME_WAIT.as_millis()
            ),
        }
    }
    fn unthrottle(&mut self, processes: &BTreeSet<u32>) {
        self.unthrottled.retain(|pid, (process, previous)| {
            if processes.contains(pid) {
                true
            } else {
                crate::window_placement::restore_throttle(process, previous);
                false
            }
        });
        for &pid in processes {
            if let std::collections::btree_map::Entry::Vacant(entry) = self.unthrottled.entry(pid)
                && let Some(previous) = crate::window_placement::unthrottle(pid)
            {
                entry.insert(previous);
            }
        }
    }
    fn label(&mut self, processes: &BTreeSet<u32>, text: &str) -> Result<Option<u64>> {
        if processes.is_empty() {
            return Ok(None);
        }
        crate::input::follow_input_desktop();
        let mut found = Vec::new();
        for window in top_level(Some(processes))? {
            if !window.visible {
                continue;
            }
            for child in children(hwnd(window.id)) {
                if text_of(child).as_deref() == Some(text) {
                    found.push(child.0 as usize as u64);
                }
            }
        }
        match found.as_slice() {
            [] => Ok(None),
            [one] => Ok(Some(*one)),
            _ => bail!(
                "Nucleus Co-op shows {} entries titled {text}: delete the extra profiles of the game in Nucleus Co-op",
                found.len()
            ),
        }
    }
    fn button(&mut self, processes: &BTreeSet<u32>, name: &str) -> Result<Option<u64>> {
        if processes.is_empty() {
            return Ok(None);
        }
        crate::input::follow_input_desktop();
        for window in top_level(Some(processes))? {
            if !window.visible {
                continue;
            }
            for child in children(hwnd(window.id)) {
                let button = class_of(child).is_some_and(|class| {
                    class.starts_with(FORM_CLASS) && class.contains(".BUTTON.")
                });
                if button && control_name(child).as_deref() == Some(name) {
                    return Ok(Some(child.0 as usize as u64));
                }
            }
        }
        Ok(None)
    }
    fn to_front(&mut self, window: u64) -> bool {
        crate::lossless::to_front_quietly(hwnd(window))
    }
    fn pc_idle(&mut self) -> bool {
        crate::input::follow_input_desktop();
        // SAFETY: GetForegroundWindow takes nothing; a null window is none.
        let front = unsafe { GetForegroundWindow() };
        let shell = front.is_invalid()
            || class_of(front).is_some_and(|class| {
                ["Progman", "WorkerW", "Shell_TrayWnd"].contains(&class.as_str())
            });
        shell || crate::wake::idle_seconds().is_ok_and(|idle| idle >= PC_IDLE_SECONDS)
    }
    fn click(&mut self, label: u64) -> Result<()> {
        let label = hwnd(label);
        let mut client = RECT::default();
        // SAFETY: plain out-pointers to live locals; a closed window fails the calls.
        unsafe { GetClientRect(label, &mut client)? };
        let (x, y) = click_point(client.right - client.left, client.bottom - client.top);
        let mut point = POINT { x, y };
        ensure!(
            // SAFETY: as above.
            unsafe { ClientToScreen(label, &mut point) }.as_bool(),
            "Nucleus Co-op's profile entry closed"
        );
        // WinForms reports a click only when the point is still the label's
        // window as it handles the release (WindowFromPoint). A window over
        // Nucleus Co-op, the PC's game or a stream's, is passed by raising
        // Nucleus Co-op above it for the click, never activated, and putting
        // it back after (#56).
        // SAFETY: a plain query of a window handle.
        let _raised = if unsafe { WindowFromPoint(point) } == label {
            None
        } else {
            // SAFETY: as above; a closed label has no root.
            let root = unsafe { GetAncestor(label, GA_ROOT) };
            ensure!(!root.is_invalid(), "Nucleus Co-op's profile entry closed");
            let raised = Raised::new(root)?;
            let deadline = std::time::Instant::now() + RAISE_WAIT;
            // SAFETY: as above.
            while unsafe { WindowFromPoint(point) } != label {
                ensure!(
                    std::time::Instant::now() < deadline,
                    "another window stays over Nucleus Co-op's profile list on the PC, even with Nucleus Co-op raised above the others"
                );
                std::thread::sleep(std::time::Duration::from_millis(15));
            }
            Some(raised)
        };
        let position = LPARAM(((y as u16 as u32) << 16 | x as u16 as u32) as isize);
        let send = |message: u32, buttons: usize, wait: u32| {
            // SAFETY: no pointer is passed; the timeout never waits long on a hung Nucleus Co-op.
            unsafe {
                SendMessageTimeoutW(
                    label,
                    message,
                    WPARAM(buttons),
                    position,
                    SMTO_ABORTIFHUNG | SMTO_NORMAL,
                    wait,
                    None,
                )
            }
            .0 != 0
        };
        // Sent, not posted: Nucleus Co-op handles both while it is raised.
        ensure!(
            send(WM_LBUTTONDOWN, LEFT_BUTTON, 1000),
            "Nucleus Co-op did not answer the click on its profile entry"
        );
        // The release runs the click, and Nucleus Co-op loads the profile in
        // it, a message box of its own included: one that takes longer than
        // the wait is under way. One that never ran shows as the profile
        // still listed, and the session clicks it again.
        if !send(WM_LBUTTONUP, 0, 1000) {
            tracing::info!("Nucleus Co-op is still handling the click on its profile entry");
        }
        Ok(())
    }
    fn message_boxes(&mut self, processes: &BTreeSet<u32>) -> Result<Vec<MessageBox>> {
        crate::input::follow_input_desktop();
        // A box shown with MB_SERVICE_NOTIFICATION is raised as a hard error
        // (NtRaiseHardError), which csrss.exe shows, not the process that
        // asked; MB_DEFAULT_DESKTOP_ONLY, which waits for the default desktop,
        // takes the same way. Nucleus Co-op 2.4.2 uses WinForms'
        // DefaultDesktopOnly for a Settings.ini it can't read and for its
        // End-key reminder. Either owner is listed: with no processes, the
        // boxes Windows shows alone. EnumWindows lists only this thread's
        // desktop, the console's, so a csrss.exe owning a window there is
        // this session's.
        let mut owners = processes.clone();
        owners.extend(
            crate::process::processes()?
                .into_iter()
                .filter(|p| p.name.eq_ignore_ascii_case("csrss.exe"))
                .map(|p| p.pid),
        );
        let mut found = Vec::new();
        for window in top_level(Some(&owners))? {
            let dialog = hwnd(window.id);
            if !window.visible || class_of(dialog).as_deref() != Some(DIALOG_CLASS) {
                continue;
            }
            // The text is a Static control; its icon is another, without text.
            let text: Vec<String> = children(dialog)
                .into_iter()
                .filter(|&child| class_of(child).is_some_and(|c| c.eq_ignore_ascii_case("Static")))
                .filter_map(text_of)
                .filter(|text| !text.trim().is_empty())
                .collect();
            found.push(MessageBox {
                window: window.id,
                caption: text_of(dialog).unwrap_or_default(),
                text: text.join(" "),
            });
        }
        Ok(found)
    }
    fn close_forms(&mut self, processes: &BTreeSet<u32>) -> Result<Vec<MessageBox>> {
        use windows::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled;
        crate::input::follow_input_desktop();
        let mut closed = Vec::new();
        for window in top_level(Some(processes))? {
            let form = hwnd(window.id);
            // A native owned window (a tooltip, a message box) is no form.
            if !window.visible
                || !window.owned
                || !class_of(form).is_some_and(|class| class.starts_with(FORM_CLASS))
            {
                continue;
            }
            // ShowDialog disables every other window of its thread: the
            // owner, the forms open beside it and a modal form below it.
            // Only the one it shows stays enabled.
            // SAFETY: plain queries of window handles; one that closed meanwhile answers null or
            // false.
            let modal = unsafe {
                let owner = GetWindow(form, GW_OWNER).unwrap_or_default();
                !owner.is_invalid()
                    && !IsWindowEnabled(owner).as_bool()
                    && IsWindowEnabled(form).as_bool()
            };
            if !modal {
                continue;
            }
            let text: Vec<String> = children(form)
                .into_iter()
                .filter_map(text_of)
                .filter(|text| !text.trim().is_empty())
                .collect();
            let shown = MessageBox {
                window: window.id,
                caption: text_of(form).unwrap_or_default(),
                text: text.join(" "),
            };
            // SAFETY: posting never waits for the form's message loop.
            match unsafe { PostMessageW(Some(form), WM_CLOSE, WPARAM(0), LPARAM(0)) } {
                Ok(()) => closed.push(shown),
                // It closed since it was listed.
                Err(error) if error.code() == ERROR_INVALID_WINDOW_HANDLE.to_hresult() => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(closed)
    }
    fn owner(&mut self, window: u64) -> Result<u32> {
        let mut pid = 0;
        // SAFETY: a plain query of a window handle; one that closed meanwhile leaves `pid` 0.
        unsafe { GetWindowThreadProcessId(hwnd(window), Some(&mut pid)) };
        ensure!(pid != 0, "the box closed");
        Ok(pid)
    }
    fn forced_aslr(&mut self, pid: u32) -> Result<bool> {
        forced_aslr(pid)
    }
}

/// Whether Windows forces process `pid` to load its images at random
/// addresses: `PROCESS_MITIGATION_ASLR_POLICY`'s EnableForceRelocateImages.
fn forced_aslr(pid: u32) -> Result<bool> {
    use anyhow::Context;
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    const FORCE_RELOCATE_IMAGES: u32 = 1 << 1;
    let mut flags = 0u32;
    // SAFETY: the newly opened handle has one owner; `flags` is a live u32, the size of the
    // policy's flags union.
    unsafe {
        let process = OwnedHandle::from_raw_handle(
            OpenProcess(PROCESS_QUERY_INFORMATION, false, pid)
                .with_context(|| format!("opening process {pid}"))?
                .0,
        );
        GetProcessMitigationPolicy(
            HANDLE(process.as_raw_handle()),
            ProcessASLRPolicy,
            (&mut flags as *mut u32).cast(),
            std::mem::size_of::<u32>(),
        )
        .with_context(|| format!("reading process {pid}'s ASLR policy"))?;
    }
    Ok(flags & FORCE_RELOCATE_IMAGES != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_label_is_clicked_where_its_text_starts() {
        assert_eq!(click_point(120, 24), (4, 12));
        // A label narrower than eight pixels is clicked in its middle.
        assert_eq!(click_point(6, 10), (3, 5));
        assert_eq!(click_point(0, 0), (0, 0));
    }

    /// #59: a process's Mandatory ASLR policy is read, and one that can't be
    /// opened is an error, never "not forced".
    #[test]
    fn a_processes_mandatory_aslr_is_read_or_an_error() {
        let ours = forced_aslr(std::process::id());
        assert!(ours.is_ok(), "{ours:?}");
        assert!(forced_aslr(0).is_err());
    }

    /// Nucleus Co-op is raised above the window over its profile list for the
    /// click, a topmost one too, and goes back below it after (#56).
    #[test]
    fn a_covered_window_is_raised_for_the_click_and_put_back() {
        struct Made(Vec<HWND>);
        impl Drop for Made {
            fn drop(&mut self) {
                for &window in &self.0 {
                    // SAFETY: a window this thread made.
                    let _ = unsafe { DestroyWindow(window) };
                }
            }
        }
        let make = |ex: WINDOW_EX_STYLE, owner: Option<HWND>| {
            // SAFETY: the class and text are NUL-terminated strings that outlive the call; the
            // owner, when given, is a window this thread made.
            unsafe {
                CreateWindowExW(
                    WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | ex,
                    &windows::core::HSTRING::from("Static"),
                    &windows::core::HSTRING::from("Rubylight raise test"),
                    WS_POPUP | WS_VISIBLE,
                    -32000,
                    -32000,
                    300,
                    200,
                    owner,
                    None,
                    None,
                    None,
                )
            }
            .unwrap()
        };
        // Whether `upper` is anywhere above `lower`.
        let above = |upper: HWND, lower: HWND| {
            let mut window = lower;
            // SAFETY: plain queries of window handles.
            while let Ok(prev) = unsafe { GetWindow(window, GW_HWNDPREV) } {
                if prev.is_invalid() {
                    break;
                }
                if prev == upper {
                    return true;
                }
                window = prev;
            }
            false
        };
        for over in [WINDOW_EX_STYLE::default(), WS_EX_TOPMOST] {
            let nucleus = make(Default::default(), None);
            // A box of its own over it stays where it is.
            let shown = make(Default::default(), Some(nucleus));
            let game = make(over, None);
            let _made = Made(vec![game, shown, nucleus]);
            // SAFETY: windows this thread made.
            unsafe {
                assert!(above(shown, nucleus) && above(game, shown));
                assert!(!topmost(nucleus));
                let raised = Raised::new(nucleus).unwrap();
                assert!(topmost(nucleus), "{over:?}");
                assert!(above(nucleus, game), "{over:?}");
                assert!(!topmost(shown) && above(game, shown), "{over:?}");
                drop(raised);
                assert!(!topmost(nucleus) && !topmost(shown), "{over:?}");
                assert_eq!(GetWindow(nucleus, GW_HWNDPREV).unwrap(), shown);
                assert!(above(game, shown), "{over:?}");
                assert_eq!(topmost(game), over == WS_EX_TOPMOST);
            }
        }
    }

    /// #56: a game window's title bar and sizing border are listed and
    /// taken away; its other styles stay.
    #[test]
    fn a_windows_frame_is_listed_and_taken_away() {
        assert!(framed(WS_OVERLAPPEDWINDOW.0));
        assert!(framed(WS_POPUP.0 | WS_THICKFRAME.0));
        assert!(!framed(WS_POPUP.0 | WS_BORDER.0));
        assert_eq!(
            unframed((WS_OVERLAPPEDWINDOW | WS_VISIBLE | WS_CLIPCHILDREN).0),
            (WS_VISIBLE | WS_CLIPCHILDREN).0
        );
        // SAFETY: the class and text are NUL-terminated strings that outlive the call.
        let window = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                &windows::core::HSTRING::from("Static"),
                &windows::core::HSTRING::from("Rubylight frame test"),
                WS_OVERLAPPEDWINDOW,
                -32000,
                -32000,
                300,
                200,
                None,
                None,
                None,
                None,
            )
        }
        .unwrap();
        let listed = || {
            top_level(Some(&BTreeSet::from([std::process::id()])))
                .unwrap()
                .into_iter()
                .find(|w| w.id == window.0 as usize as u64)
                .unwrap()
        };
        assert!(listed().framed);
        let mut desktop = Win32::default();
        let id = window.0 as usize as u64;
        // This thread owns the window and runs no message loop while the
        // pass waits, as a hung game: the pass gives up in time instead of
        // waiting without end, and starts no second change meanwhile.
        let before = std::time::Instant::now();
        let error = desktop.unframe(id).unwrap_err().to_string();
        assert!(
            error.contains("did not take its frame's removal"),
            "{error}"
        );
        assert!(before.elapsed() < Duration::from_secs(2));
        let error = desktop.unframe(id).unwrap_err().to_string();
        assert!(error.contains("has not yet taken"), "{error}");
        // Once its messages run, the window takes the change.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while desktop.unframing.load(Ordering::Acquire) || listed().framed {
            assert!(std::time::Instant::now() < deadline, "the frame stayed");
            let mut message = MSG::default();
            // SAFETY: `message` is a live local; only this thread's messages are run.
            unsafe {
                while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        // Once is enough; again changes nothing, and waits on no thread.
        desktop.unframe(id).unwrap();
        // SAFETY: a window this thread made.
        unsafe { DestroyWindow(window).unwrap() };
        assert!(desktop.unframe(window.0 as usize as u64).is_err());
    }

    #[test]
    fn the_desktop_lists_this_pc_s_windows_and_monitors() {
        let mut desktop = Win32::default();
        let processes = desktop.processes().unwrap();
        assert!(processes.iter().any(|p| p.pid == std::process::id()));
        let monitors = desktop.monitors().unwrap();
        let screen = desktop.virtual_screen();
        // A desktop without a monitor (a service test run) has an empty screen.
        for monitor in &monitors {
            assert!(
                i64::from(monitor.x) >= i64::from(screen.x) && monitor.right() <= screen.right(),
                "{monitor:?} outside {screen:?}"
            );
        }
        desktop.windows().unwrap();
        // No process of this test has a window labelled like the profile.
        assert_eq!(
            desktop
                .label(&BTreeSet::from([std::process::id()]), "Rubylight")
                .unwrap(),
            None
        );
        // Nor a message box; one Windows shows for another program may be
        // up.
        let own = BTreeSet::from([std::process::id()]);
        let before = desktop.message_boxes(&own).unwrap();
        let new = |desktop: &mut Win32| -> Vec<MessageBox> {
            let mut boxes = desktop.message_boxes(&own).unwrap();
            boxes.retain(|b| !before.contains(b));
            boxes
        };

        // A box as MessageBox makes it, off every screen and never active:
        // a dialog with its icon and its text, a few hundred characters as
        // OneDrive's warning in Nucleus Co-op. A plain window beside it is
        // no message box. In this test, not a test of its own: the empty
        // list above would see them.
        struct Made(Vec<HWND>);
        impl Drop for Made {
            fn drop(&mut self) {
                for &window in self.0.iter().rev() {
                    // SAFETY: a window this thread made; one its parent took along fails harmlessly.
                    let _ = unsafe { DestroyWindow(window) };
                }
            }
        }
        let make = |class: &str, text: &str, ex: WINDOW_EX_STYLE, style, parent| {
            // SAFETY: the class and text are NUL-terminated strings that outlive the call; the
            // parent, when given, is a window this thread made.
            unsafe {
                CreateWindowExW(
                    ex,
                    &windows::core::HSTRING::from(class),
                    &windows::core::HSTRING::from(text),
                    style,
                    -32000,
                    -32000,
                    300,
                    200,
                    parent,
                    None,
                    None,
                    None,
                )
            }
            .unwrap()
        };
        let hidden = WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
        let said = "Your Documents folder is in OneDrive. ".repeat(11);
        assert!(said.len() > 400);
        let mut made = Made(Vec::new());
        let dialog = make(
            DIALOG_CLASS,
            "Rubylight test box",
            hidden,
            WS_POPUP | WS_VISIBLE,
            None,
        );
        made.0.push(dialog);
        let child = WS_CHILD | WS_VISIBLE;
        made.0
            .push(make("Static", "", Default::default(), child, Some(dialog)));
        made.0.push(make(
            "Static",
            &said,
            Default::default(),
            child,
            Some(dialog),
        ));
        made.0.push(make(
            "Static",
            "not a box",
            hidden,
            WS_POPUP | WS_VISIBLE,
            None,
        ));
        assert_eq!(
            new(&mut desktop),
            [MessageBox {
                window: dialog.0 as usize as u64,
                caption: "Rubylight test box".into(),
                text: said.clone(),
            }]
        );
        // With no process, only the boxes Windows shows: not this one.
        assert!(
            desktop
                .message_boxes(&BTreeSet::new())
                .unwrap()
                .iter()
                .all(|b| b.window != dialog.0 as usize as u64)
        );
        drop(made);
        assert_eq!(new(&mut desktop), []);

        // A form shown with ShowDialog: the enabled form owned by the window
        // it disabled, which is closed with what it says. ShowDialog
        // disables the thread's other windows too: a form open beside it
        // and the modal form below a nested one are left alone, as are a
        // native window over the disabled one (a tooltip, a message box)
        // and a form whose owner is enabled.
        unsafe extern "system" fn plain(
            window: HWND,
            message: u32,
            w: WPARAM,
            l: LPARAM,
        ) -> LRESULT {
            // SAFETY: the message as Windows passed it.
            unsafe { DefWindowProcW(window, message, w, l) }
        }
        let class = "WindowsForms10.Window.8.app.rubylight-test";
        let name = windows::core::HSTRING::from(class);
        let registered = WNDCLASSW {
            lpfnWndProc: Some(plain),
            lpszClassName: windows::core::PCWSTR(name.as_ptr()),
            ..Default::default()
        };
        // SAFETY: the class name outlives the call; a second registration fails harmlessly.
        unsafe { RegisterClassW(&registered) };
        let disable = |window: HWND| {
            // SAFETY: a window this thread made.
            let _ =
                unsafe { windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow(window, false) };
        };
        let mut made = Made(Vec::new());
        let popup = WS_POPUP | WS_VISIBLE;
        let owner = make(class, "Nucleus Co-op", hidden, popup, None);
        made.0.push(owner);
        let form = make(class, "", hidden, popup, Some(owner));
        made.0.push(form);
        made.0.push(make(
            "Static",
            "The profile can't be loaded.",
            Default::default(),
            WS_CHILD | WS_VISIBLE,
            Some(form),
        ));
        let beside = make(class, "beside", hidden, popup, Some(owner));
        made.0.push(beside);
        made.0
            .push(make("Static", "tooltip", hidden, popup, Some(owner)));
        made.0
            .push(make(DIALOG_CLASS, "box", hidden, popup, Some(owner)));
        let free = make(class, "free", hidden, popup, None);
        made.0.push(free);
        made.0.push(make(class, "owned", hidden, popup, Some(free)));
        assert!(desktop.close_forms(&own).unwrap().is_empty());
        disable(owner);
        disable(beside);
        assert_eq!(
            desktop.close_forms(&own).unwrap(),
            [MessageBox {
                window: form.0 as usize as u64,
                caption: String::new(),
                text: "The profile can't be loaded.".into(),
            }]
        );
        // A form shown over that one: only it is.
        let nested = make(class, "nested", hidden, popup, Some(form));
        made.0.push(nested);
        disable(form);
        let closed: Vec<u64> = desktop
            .close_forms(&own)
            .unwrap()
            .iter()
            .map(|form| form.window)
            .collect();
        assert_eq!(closed, [nested.0 as usize as u64]);
    }
}
