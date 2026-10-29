//! The PC's real desktop for a Nucleus session: processes, top-level
//! windows, monitors, moving windows without activating them, and the click
//! that loads the host's profile in Nucleus Co-op's window.
use super::instances::{Desktop, MessageBox};
use anyhow::{Result, bail, ensure};
use butterpollo_core::nucleus::window::{Rect, Window};
use butterpollo_core::steam::Process;
use std::collections::{BTreeMap, BTreeSet};
use std::os::windows::io::OwnedHandle;
use windows::Win32::{
    Foundation::*, Graphics::Dwm::*, Graphics::Gdi::*, System::Threading::*,
    UI::WindowsAndMessaging::*,
};
use windows::core::BOOL;

/// EnumWindows stops after this many windows, as the background keeper does.
const WINDOW_LIMIT: usize = 4096;
/// `MK_LBUTTON`: the left button is down.
const LEFT_BUTTON: usize = 0x0001;

/// The desktop of the signed-in user. Dropping it gives every process it
/// unthrottled its power settings back.
#[derive(Default)]
pub struct Win32 {
    unthrottled: BTreeMap<u32, (OwnedHandle, PROCESS_POWER_THROTTLING_STATE)>,
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
            scan.found.push(Window {
                id: window.0 as usize as u64,
                pid,
                rect: rect_of(rect),
                // A minimised window shows nothing to capture; a cloaked one
                // (another virtual desktop, a suspended app) neither.
                visible: IsWindowVisible(window).as_bool()
                    && !IsIconic(window).as_bool()
                    && cloaked == 0,
                owned: !GetWindow(window, GW_OWNER).unwrap_or_default().is_invalid(),
                tool: ex & WS_EX_TOOLWINDOW.0 != 0,
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
            if IsZoomed(window).as_bool() {
                // Maximising shows and activates a window; restore it without activation.
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
    fn click(&mut self, label: u64) -> Result<()> {
        let label = hwnd(label);
        let mut client = RECT::default();
        // SAFETY: plain out-pointers to live locals; a closed window fails the calls, and posting
        // never waits for Nucleus' message loop.
        unsafe {
            GetClientRect(label, &mut client)?;
            let (x, y) = click_point(client.right - client.left, client.bottom - client.top);
            let mut point = POINT { x, y };
            ensure!(
                ClientToScreen(label, &mut point).as_bool(),
                "Nucleus Co-op's profile entry closed"
            );
            // WinForms reports a click only when the press was its own and
            // the point is still its window when the button is released.
            ensure!(
                WindowFromPoint(point) == label,
                "another window covers Nucleus Co-op's profile list on the PC"
            );
            let position = LPARAM(((y as u16 as u32) << 16 | x as u16 as u32) as isize);
            PostMessageW(Some(label), WM_LBUTTONDOWN, WPARAM(LEFT_BUTTON), position)?;
            PostMessageW(Some(label), WM_LBUTTONUP, WPARAM(0), position)?;
        }
        Ok(())
    }
    fn message_boxes(&mut self, processes: &BTreeSet<u32>) -> Result<Vec<MessageBox>> {
        if processes.is_empty() {
            return Ok(Vec::new());
        }
        crate::input::follow_input_desktop();
        // A box shown with MB_SERVICE_NOTIFICATION is raised as a hard error
        // (NtRaiseHardError), which csrss.exe shows, not the process that
        // asked; MB_DEFAULT_DESKTOP_ONLY, which waits for the default desktop,
        // takes the same way. Nucleus Co-op 2.4.2 uses WinForms'
        // DefaultDesktopOnly for a Settings.ini it can't read and for its
        // End-key reminder. Either owner is listed. EnumWindows lists only
        // this thread's desktop, the console's, so a csrss.exe owning a
        // window there is this session's.
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
                caption: text_of(dialog).unwrap_or_default(),
                text: text.join(" "),
            });
        }
        Ok(found)
    }
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
                caption: "Rubylight test box".into(),
                text: said.clone(),
            }]
        );
        drop(made);
        assert_eq!(new(&mut desktop), []);
    }
}
