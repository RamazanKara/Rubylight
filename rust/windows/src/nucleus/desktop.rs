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

/// UI Automation on this thread, for reading WinForms control names.
struct Automation {
    automation: windows::Win32::UI::Accessibility::IUIAutomation,
    // Declared after `automation`, so the object is released before COM is.
    _com: Com,
}

/// This thread's COM, uninitialised on drop when it was ours to start.
struct Com(bool);

impl Drop for Com {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: only a successful CoInitializeEx on this thread sets the flag.
            unsafe { windows::Win32::System::Com::CoUninitialize() }
        }
    }
}

impl Automation {
    fn new() -> Option<Self> {
        use windows::Win32::System::Com::{
            CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
        };
        use windows::Win32::UI::Accessibility::CUIAutomation;
        // SAFETY: CoInitializeEx takes no pointers here. A thread already in an apartment
        // (RPC_E_CHANGED_MODE) keeps it and the guard leaves it alone; a successful call is
        // balanced once by the guard.
        unsafe {
            let com = Com(CoInitializeEx(None, COINIT_MULTITHREADED).is_ok());
            let automation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
            Some(Self {
                automation,
                _com: com,
            })
        }
    }
}

/// What a WinForms window of another process is called in its program: its
/// UI Automation id, which Windows reads from the control's `Name`.
fn control_name(automation: &Automation, window: HWND) -> Option<String> {
    // SAFETY: a closed window fails the call, and the id is an owned BSTR.
    let id = unsafe {
        automation
            .automation
            .ElementFromHandle(window)
            .and_then(|element| element.CurrentAutomationId())
            .ok()?
    };
    let id = id.to_string();
    (!id.is_empty()).then_some(id)
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

/// Whether a press of `button` reaches it: Nucleus keeps Play on its form hidden until a
/// profile is ready to play (`Generic_Functions.EnablePlay`), and a press of a hidden or
/// disabled button does nothing, yet is only ever made once.
fn pressable(button: HWND) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled;
    // SAFETY: plain queries of a window handle; one that closed meanwhile answers false.
    unsafe { IsWindowVisible(button).as_bool() && IsWindowEnabled(button).as_bool() }
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
        let Some(automation) = Automation::new() else {
            bail!("UI Automation is unavailable, so Nucleus Co-op's buttons can't be found");
        };
        for window in top_level(Some(processes))? {
            if !window.visible {
                continue;
            }
            for child in children(hwnd(window.id)) {
                let button = class_of(child).is_some_and(|class| {
                    class.starts_with(FORM_CLASS) && class.contains(".BUTTON.")
                });
                if button
                    && pressable(child)
                    && control_name(&automation, child).as_deref() == Some(name)
                {
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
mod tests;
