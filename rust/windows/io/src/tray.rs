//! Native notification-area UI, owned by its Windows message thread.
use anyhow::{Context, Result, bail};
use butterpollo_session::tray::{Icon, NoticeAction};
use std::{
    mem::size_of,
    path::PathBuf,
    sync::{Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::*,
        System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
        UI::{Shell::*, WindowsAndMessaging::*},
    },
    core::{PCWSTR, w},
};

mod icon;
mod notices;
pub mod status;

use icon::Icons;
#[cfg(test)]
use notices::launch_error_notice;
use notices::{NOTICES, SHOWN, show_held};
pub use notices::{
    launch_error, notices_due, notify, notify_pairing, pairing_request_ended,
    release_overdue_notices, try_notify,
};
use status::{Item, Status};

/// Retries adding the icon while the taskbar does not accept it yet.
const RETRY_TIMER: usize = 1;
/// The running app changed; see [`show_app`].
const APP_CHANGED: u32 = WM_APP + 2;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Open,
    /// Open the console where a waiting device's PIN goes.
    EnterPin,
    /// Open the console where the QR code that pairs a device is.
    PairDevice,
    StopSessions,
    /// Quit the running app, as the console's close button does.
    QuitApp,
    /// Look for a newer release and say what was found.
    CheckUpdates,
    /// Save the settings, without secrets, to the user's Documents folder.
    BackupSettings,
    /// Open the console where a settings backup is restored.
    RestoreSettings,
    Restart,
    Quit,
}
struct State {
    sender: mpsc::Sender<Action>,
    icon: NOTIFYICONDATAW,
    /// The brand icon and one badged copy per status, at the current small-icon size.
    icons: Icons,
    /// The icon file used when the executable carries no icon.
    icon_file: PathBuf,
    port: u16,
    taskbar: u32,
    hide_controls: bool,
    added: AddLog,
}
/// How long the taskbar may refuse the icon before the log says so again.
const ADD_REFUSED_LONG: Duration = Duration::from_secs(60);
/// What the log says of the attempts to add the icon: Windows refusing it, once, and again once
/// it keeps refusing, and the icon going in.
#[derive(Default)]
struct AddLog {
    refused_since: Option<Instant>,
    retry_since: Option<Instant>,
    warned_long: bool,
    warned_with_taskbar: bool,
    added_once: bool,
}
#[derive(Debug, PartialEq, Eq)]
enum AddReport {
    Refused,
    StillRefused,
    /// Added, the first time or after Windows refused it for this long.
    Added(Option<Duration>),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AddOutcome {
    Added,
    Retry,
    GiveUp,
}
impl AddLog {
    fn is_added(&self) -> bool {
        self.added_once && self.refused_since.is_none()
    }
    fn note_still_refused(&mut self, taskbar: bool) {
        if taskbar {
            self.warned_with_taskbar = true;
        }
    }
    /// Allow a fresh retry window once Explorer broadcasts `TaskbarCreated`.
    fn on_taskbar_created(&mut self) {
        self.retry_since = None;
        if self.warned_long && !self.warned_with_taskbar {
            self.refused_since = None;
            self.warned_long = false;
        }
    }
    fn gave_up(&self, now: Instant) -> bool {
        self.retry_since
            .is_some_and(|since| now.saturating_duration_since(since) >= ADD_REFUSED_LONG)
    }
    fn attempt(&mut self, added: bool, now: Instant) -> Option<AddReport> {
        if added {
            self.retry_since = None;
            let refused = self.refused_since.take().map(|since| now - since);
            self.warned_long = false;
            self.warned_with_taskbar = false;
            let first = !std::mem::replace(&mut self.added_once, true);
            return (first || refused.is_some()).then_some(AddReport::Added(refused));
        }
        self.retry_since.get_or_insert(now);
        match self.refused_since {
            None => {
                self.refused_since = Some(now);
                Some(AddReport::Refused)
            }
            Some(since) if !self.warned_long && now - since >= ADD_REFUSED_LONG => {
                self.warned_long = true;
                Some(AddReport::StillRefused)
            }
            Some(_) => None,
        }
    }
}
/// Add the icon, and log what [`AddLog`] says of the attempt.
unsafe fn add_icon(state: &mut State, on_taskbar: bool) -> AddOutcome {
    // Explorer can broadcast `TaskbarCreated` just after our 3 s timer already
    // added the icon; `NIM_ADD` on an already-registered icon fails with
    // `0x80004005`, so check `NIM_MODIFY` first when we believe it is present.
    if on_taskbar && state.added.is_added() {
        // SAFETY: the caller passes the live State of the tray window, whose icon data is complete.
        if unsafe { Shell_NotifyIconW(NIM_MODIFY, &state.icon).as_bool() } {
            return AddOutcome::Added;
        }
    }
    // SAFETY: the caller passes the live State of the tray window, whose icon data is complete.
    let (added, error) = unsafe {
        let added = Shell_NotifyIconW(NIM_ADD, &state.icon).as_bool();
        (added, (!added).then(|| GetLastError()))
    };
    let report = state.added.attempt(added, Instant::now());
    let session = crate::process::current_session().ok();
    // Explorer's taskbar on this desktop: without it nothing can show an icon.
    // SAFETY: both strings are static and NUL-terminated.
    let taskbar = unsafe { FindWindowW(w!("Shell_TrayWnd"), PCWSTR::null()) }.is_ok();
    match report {
        // Initial refusals (e.g. 0x80004005 before the taskbar is ready) are retried
        // every 3 s, so keep them at DEBUG until retries give up.
        Some(AddReport::Refused) if !taskbar => tracing::debug!(
            error = ?error,
            session_id = session,
            "no taskbar yet for the tray icon; adding it when the taskbar starts"
        ),
        Some(AddReport::Refused) => tracing::debug!(
            error = ?error,
            session_id = session,
            taskbar,
            "Windows refused the tray icon; adding it again every 3 s and when the taskbar starts"
        ),
        Some(AddReport::StillRefused) if !taskbar => {
            state.added.note_still_refused(false);
            tracing::info!(
                error = ?error,
                session_id = session,
                "still no taskbar for the tray icon after a minute; stopping 3 s retries until the taskbar starts"
            );
        }
        Some(AddReport::StillRefused) => {
            state.added.note_still_refused(true);
            tracing::warn!(
                error = ?error,
                session_id = session,
                taskbar,
                "Windows still refuses the tray icon after a minute although the taskbar is there; stopping 3 s retries until the taskbar restarts"
            );
        }
        Some(AddReport::Added(refused)) => tracing::info!(
            session_id = session,
            refused_s = refused.map(|refused| refused.as_secs()),
            "tray icon added"
        ),
        None => {}
    }
    if added {
        AddOutcome::Added
    } else if state.added.gave_up(Instant::now()) {
        AddOutcome::GiveUp
    } else {
        AddOutcome::Retry
    }
}
impl State {
    /// Show the running app's state in the icon and its tooltip.
    fn show(&mut self) {
        let app = APP.lock().unwrap();
        self.icon.hIcon = self.icons.get(app.status());
        let tip = match &app.summary {
            Some(summary) => format!("Rubylight — {summary}"),
            None => format!("Rubylight — web port {}", self.port),
        };
        let tip: Vec<u16> = tip.encode_utf16().take(127).collect();
        self.icon.szTip = [0; 128];
        self.icon.szTip[..tip.len()].copy_from_slice(&tip);
    }
    /// Load the icons again at the size Windows uses now, as after Explorer restarts
    /// (possibly at another DPI), and show the current status with them.
    fn reload_icons(&mut self) {
        match Icons::load(&self.icon_file) {
            // The old set goes once the new one is in the icon data.
            Ok(icons) => drop(std::mem::replace(&mut self.icons, icons)),
            Err(error) => tracing::debug!(%error, "kept the tray icons"),
        }
        self.show();
    }
}
/// The running app as the tray shows it.
struct Shown {
    icon: Icon,
    app: Option<String>,
    summary: Option<String>,
    /// The app whose launch failed, while it is still the running app.
    failed: Option<String>,
}
impl Shown {
    fn status(&self) -> Status {
        let failed = self.failed.is_some() && self.failed == self.app;
        Status::from_icon(self.icon, failed)
    }
}
static APP: Mutex<Shown> = Mutex::new(Shown {
    icon: Icon::Idle,
    app: None,
    summary: None,
    failed: None,
});
/// Show the running app (None when none runs) in the tray: the icon's
/// state, the tooltip and the menu's item that quits it.
pub fn show_app(icon: Icon, app: Option<&str>, summary: Option<&str>) {
    let mut shown = APP.lock().unwrap();
    let changed = shown.app.as_deref() != app;
    let failed = if changed { None } else { shown.failed.take() };
    *shown = Shown {
        icon,
        app: app.map(str::to_owned),
        summary: summary.map(str::to_owned),
        failed,
    };
    drop(shown);
    if changed {
        let held = NOTICES.lock().unwrap().app_changed();
        show_held(held);
    }
    repaint();
}
/// App `app` closed right after it started: the tray shows the error while it stays
/// the running app.
fn launch_failed(app: &str) {
    APP.lock().unwrap().failed = Some(app.to_owned());
    repaint();
}
/// Have the tray's thread show the current status.
fn repaint() {
    if let Some(window) = *SHOWN.lock().unwrap() {
        // SAFETY: The tray publishes its HWND under SHOWN; this asynchronous message
        // contains only integer values and borrows no Rust data after the call returns.
        unsafe {
            let _ = PostMessageW(
                Some(HWND(window as *mut _)),
                APP_CHANGED,
                WPARAM(0),
                LPARAM(0),
            );
        }
    }
}
unsafe extern "system" fn window(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: Windows supplies CREATESTRUCTW for WM_NCCREATE. The tray thread
    // keeps its boxed State alive until message dispatch ends and clears
    // GWLP_USERDATA before destroying the window. State borrows stay within
    // handlers and do not cross the reentrant popup-menu calls.
    unsafe {
        if message == WM_NCCREATE {
            let create = &*(lparam.0 as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        }
        let state = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
        if !state.is_null() {
            let taskbar = (*state).taskbar;
            // Explorer announces a new taskbar (at sign-in, or after it
            // restarts); until the icon is in, retry every few seconds.
            if message == taskbar || (message == WM_TIMER && wparam.0 == RETRY_TIMER) {
                let s = &mut *state;
                if message == taskbar {
                    // A new taskbar may come with another DPI: badge the icon afresh.
                    s.reload_icons();
                    s.added.on_taskbar_created();
                } else {
                    s.show();
                }
                match add_icon(s, message == taskbar) {
                    AddOutcome::Added | AddOutcome::GiveUp => {
                        let _ = KillTimer(Some(hwnd), RETRY_TIMER);
                    }
                    AddOutcome::Retry if message == taskbar => {
                        SetTimer(Some(hwnd), RETRY_TIMER, 3000, None);
                    }
                    AddOutcome::Retry => {}
                }
                return LRESULT(0);
            }
            if message == APP_CHANGED {
                let s = &mut *state;
                s.show();
                let _ = Shell_NotifyIconW(NIM_MODIFY, &s.icon);
                return LRESULT(0);
            }
            if message == WM_APP + 1 {
                match (lparam.0 & 0xffff) as u32 {
                    NIN_BALLOONUSERCLICK => {
                        let (action, held) = NOTICES.lock().unwrap().clicked(Instant::now());
                        match action {
                            Some(NoticeAction::EnterPin) => {
                                let _ = (*state).sender.send(Action::EnterPin);
                            }
                            Some(NoticeAction::QuitApp) => {
                                let _ = (*state).sender.send(Action::QuitApp);
                            }
                            None => {}
                        }
                        show_held(held);
                    }
                    NIN_BALLOONTIMEOUT => {
                        let held = NOTICES.lock().unwrap().timed_out(Instant::now());
                        show_held(held);
                    }
                    WM_LBUTTONDBLCLK => {
                        let _ = (*state).sender.send(Action::Open);
                    }
                    WM_RBUTTONUP | WM_CONTEXTMENU => {
                        let hide_controls = (*state).hide_controls;
                        if let Ok(menu) = CreatePopupMenu() {
                            let items = {
                                let app = APP.lock().unwrap();
                                status::menu(app.status(), app.app.as_deref(), hide_controls)
                            };
                            // Command ids are the item's position plus one; 0 is no choice.
                            for (at, item) in items.iter().enumerate() {
                                let (flags, label) = match item {
                                    Item::Separator => {
                                        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
                                        continue;
                                    }
                                    Item::Header(label) => (MF_STRING | MF_GRAYED, label),
                                    Item::Entry { label, enabled, .. } => (
                                        if *enabled {
                                            MF_STRING
                                        } else {
                                            MF_STRING | MF_GRAYED
                                        },
                                        label,
                                    ),
                                };
                                let label: Vec<u16> = status::menu_label(label)
                                    .encode_utf16()
                                    .chain(Some(0))
                                    .collect();
                                let _ = AppendMenuW(menu, flags, at + 1, PCWSTR(label.as_ptr()));
                            }
                            let mut point = POINT::default();
                            let _ = GetCursorPos(&mut point);
                            // These calls can re-enter `window`; keep only copied
                            // values and the raw state pointer across them.
                            let _ = SetForegroundWindow(hwnd);
                            let id = TrackPopupMenu(
                                menu,
                                TPM_RETURNCMD | TPM_NONOTIFY,
                                point.x,
                                point.y,
                                None,
                                hwnd,
                                None,
                            )
                            .0;
                            let action = usize::try_from(id)
                                .ok()
                                .and_then(|id| items.get(id.checked_sub(1)?))
                                .and_then(|item| match item {
                                    Item::Entry {
                                        action,
                                        enabled: true,
                                        ..
                                    } => Some(*action),
                                    _ => None,
                                });
                            if let Some(action) = action {
                                let s = &*state;
                                let _ = s.sender.send(action);
                            }
                            let _ = DestroyMenu(menu);
                            let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
                        }
                    }
                    _ => {}
                }
                return LRESULT(0);
            }
        }
        DefWindowProcW(hwnd, message, wparam, lparam)
    }
}
pub struct Tray {
    id: u32,
    worker: Option<thread::JoinHandle<()>>,
}
impl Tray {
    pub fn new(icon: PathBuf, port: u16) -> Result<(Self, mpsc::Receiver<Action>)> {
        Self::new_options(icon, port, false)
    }
    pub fn new_options(
        icon: PathBuf,
        port: u16,
        hide_controls: bool,
    ) -> Result<(Self, mpsc::Receiver<Action>)> {
        let (sender, receiver) = mpsc::channel();
        let (ready, started) = mpsc::sync_channel(1);
        let worker = thread::Builder::new().name("tray".into()).spawn(move || {
            let result = (|| -> Result<()> {
                // SAFETY: This thread creates, dispatches and destroys the window;
                // sized structures and terminated strings remain live through calls.
                // The boxed State outlives GWLP_USERDATA, owned icons are destroyed
                // once after the window, and the shared fallback icon is never freed.
                unsafe {
                    let module = GetModuleHandleW(None)?;
                    let class = WNDCLASSW {
                        lpfnWndProc: Some(window),
                        hInstance: HINSTANCE(module.0),
                        lpszClassName: w!("ButterpolloRustTray"),
                        ..Default::default()
                    };
                    if RegisterClassW(&class) == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
                        bail!("cannot register tray window");
                    }
                    let icons = Icons::load(&icon)?;
                    let mut state = Box::new(State {
                        sender,
                        icon: NOTIFYICONDATAW {
                            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
                            uID: 1,
                            uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
                            uCallbackMessage: WM_APP + 1,
                            hIcon: icons.get(Status::Idle),
                            ..Default::default()
                        },
                        icons,
                        icon_file: icon,
                        port,
                        taskbar: RegisterWindowMessageW(w!("TaskbarCreated")),
                        hide_controls,
                        added: AddLog::default(),
                    });
                    state.show();
                    let hwnd = CreateWindowExW(
                        WINDOW_EX_STYLE(0),
                        w!("ButterpolloRustTray"),
                        w!("Butterpollo Rust"),
                        WINDOW_STYLE(0),
                        0,
                        0,
                        0,
                        0,
                        None,
                        None,
                        Some(HINSTANCE(module.0)),
                        Some((&mut *state as *mut State).cast()),
                    )?;
                    state.icon.hWnd = hwnd;
                    // The service starts the host elevated, and Windows keeps
                    // Explorer's messages from an elevated window unless they
                    // are allowed: the TaskbarCreated broadcast after sign-in
                    // never arrived, so an icon that the not-yet-ready taskbar
                    // refused at boot never appeared.
                    for message in [state.taskbar, WM_APP + 1] {
                        let _ = ChangeWindowMessageFilterEx(hwnd, message, MSGFLT_ALLOW, None);
                    }
                    if add_icon(&mut state, false) == AddOutcome::Retry {
                        SetTimer(Some(hwnd), RETRY_TIMER, 3000, None);
                    }
                    *SHOWN.lock().unwrap() = Some(hwnd.0 as isize);
                    let _ = ready.send(Ok(GetCurrentThreadId()));
                    let mut message = MSG::default();
                    loop {
                        let status = GetMessageW(&mut message, None, 0, 0).0;
                        if status <= 0 {
                            break;
                        }
                        let _ = TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }
                    *SHOWN.lock().unwrap() = None;
                    let _ = Shell_NotifyIconW(NIM_DELETE, &state.icon);
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                    let _ = DestroyWindow(hwnd);
                    // The icons go with the state, after the window.
                    drop(state);
                    Ok(())
                }
            })();
            if let Err(e) = result {
                let _ = ready.send(Err(e.to_string()));
            }
        })?;
        let id = started
            .recv_timeout(std::time::Duration::from_secs(10))
            .context("tray startup timed out")?
            .map_err(|e| anyhow::anyhow!(e))?;
        Ok((
            Self {
                id,
                worker: Some(worker),
            },
            receiver,
        ))
    }
}
impl Drop for Tray {
    fn drop(&mut self) {
        // SAFETY: id identifies the worker's message queue; WM_QUIT carries no borrowed
        // pointers, and the worker is joined before its owner finishes dropping.
        unsafe {
            let _ = PostThreadMessageW(self.id, WM_QUIT, WPARAM(0), LPARAM(0));
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// The console's Devices page with a waiting device's PIN field focused.
pub const PAGE_ENTER_PIN: &str = "/devices?pair=pin";
/// The console's Devices page at the QR code that pairs a device.
pub const PAGE_PAIR_QR: &str = "/devices?pair=qr";
/// The console's Troubleshoot page at the settings backup.
pub const PAGE_BACKUP: &str = "/troubleshoot#backup";

fn console_url(port: u16, page: &str) -> String {
    format!("https://localhost:{port}{page}")
}

pub fn open_web(port: u16) -> Result<()> {
    open_web_page(port, "/")
}

/// Open a page of the console in the signed-in user's browser. `page` is one
/// of the `PAGE_` paths or `/`.
pub fn open_web_page(port: u16, page: &str) -> Result<()> {
    if !matches!(page, "/" | PAGE_ENTER_PIN | PAGE_PAIR_QR | PAGE_BACKUP) {
        bail!("not a console page: {page}");
    }
    if crate::process::is_system() {
        return crate::process::Process::spawn_detached(
            &std::env::current_exe()?,
            &[
                "--open-web".into(),
                port.to_string().into(),
                "--open-web-page".into(),
                page.into(),
            ],
            crate::process::Target::User { elevated: false },
        );
    }
    let url: Vec<u16> = format!("{}\0", console_url(port, page))
        .encode_utf16()
        .collect();
    // SAFETY: The verb and URL are terminated UTF-16 strings that remain live for
    // ShellExecuteW; the unused optional pointer arguments are null.
    let result = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(url.as_ptr()),
            None,
            None,
            SW_SHOWNORMAL,
        )
    };
    if result.0 as usize <= 32 {
        bail!("Windows could not open the web interface");
    }
    Ok(())
}

#[cfg(test)]
mod tests;
