//! Native notification-area UI, owned by its Windows message thread.
use anyhow::{Context, Result, bail};
use butterpollo_core::tray::{Icon, NoticeAction, Notices, Post, QueuedNotice};
use std::{
    mem::size_of,
    path::PathBuf,
    sync::{Mutex, mpsc},
    thread,
    time::Instant,
};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::Gdi::{
            BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, DIB_RGB_COLORS,
            DeleteObject, GetDC, GetDIBits, GetObjectW, ReleaseDC,
        },
        System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
        UI::{Shell::*, WindowsAndMessaging::*},
    },
    core::{PCWSTR, w},
};
/// Retries adding the icon while the taskbar does not accept it yet.
const RETRY_TIMER: usize = 1;
/// The running app changed; see [`show_app`].
const APP_CHANGED: u32 = WM_APP + 2;
#[derive(Clone, Copy)]
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
    Restart,
    Quit,
}
struct State {
    sender: mpsc::Sender<Action>,
    icon: NOTIFYICONDATAW,
    /// Idle, streaming and paused.
    icons: [HICON; 3],
    port: u16,
    taskbar: u32,
    hide_controls: bool,
}
impl State {
    /// Show the running app's state in the icon and its tooltip.
    fn show(&mut self) {
        let app = APP.lock().unwrap();
        self.icon.hIcon = self.icons[match app.icon {
            Icon::Idle => 0,
            Icon::Streaming => 1,
            Icon::Paused => 2,
        }];
        let tip = match &app.summary {
            Some(summary) => format!("Rubylight — {summary}"),
            None => format!("Rubylight — web port {}", self.port),
        };
        let tip: Vec<u16> = tip.encode_utf16().take(127).collect();
        self.icon.szTip = [0; 128];
        self.icon.szTip[..tip.len()].copy_from_slice(&tip);
    }
}
/// The running app as the tray shows it.
struct Shown {
    icon: Icon,
    app: Option<String>,
    summary: Option<String>,
}
static APP: Mutex<Shown> = Mutex::new(Shown {
    icon: Icon::Idle,
    app: None,
    summary: None,
});
/// Show the running app (None when none runs) in the tray: the icon's
/// state, the tooltip and the menu's item that quits it.
pub fn show_app(icon: Icon, app: Option<&str>, summary: Option<&str>) {
    let mut shown = APP.lock().unwrap();
    let changed = shown.app.as_deref() != app;
    *shown = Shown {
        icon,
        app: app.map(str::to_owned),
        summary: summary.map(str::to_owned),
    };
    drop(shown);
    if changed {
        let held = NOTICES.lock().unwrap().app_changed();
        show_held(held);
    }
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
/// `base` with the state's dot in its corner; None for Idle or an icon
/// without alpha, which then shows as is.
unsafe fn badged(base: HICON, state: Icon) -> Option<HICON> {
    // SAFETY: The caller retains base on the tray thread. GetIconInfo returns owned
    // bitmaps; checked dimensions bound the 32-bit pixel and word-aligned mask buffers.
    // GDI copies their data synchronously, and each bitmap/DC is deleted/released once.
    unsafe {
        let mut info = ICONINFO::default();
        GetIconInfo(base, &mut info).ok()?;
        let made = (|| {
            if info.hbmColor.is_invalid() {
                return None;
            }
            let mut bitmap = BITMAP::default();
            if GetObjectW(
                info.hbmColor.into(),
                size_of::<BITMAP>() as i32,
                Some((&mut bitmap as *mut BITMAP).cast()),
            ) == 0
            {
                return None;
            }
            let (width, height) = (bitmap.bmWidth, bitmap.bmHeight);
            if width <= 0 || height <= 0 || width > 256 || height > 256 {
                return None;
            }
            let mut header = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    // Top-down rows, as CreateBitmap takes them.
                    biHeight: -height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut pixels = vec![0u8; width as usize * height as usize * 4];
            let screen = GetDC(None);
            let lines = GetDIBits(
                screen,
                info.hbmColor,
                0,
                height as u32,
                Some(pixels.as_mut_ptr().cast()),
                &mut header,
                DIB_RGB_COLORS,
            );
            ReleaseDC(None, screen);
            if lines != height
                || !butterpollo_core::tray::badge(
                    &mut pixels,
                    width as usize,
                    height as usize,
                    state,
                )
            {
                return None;
            }
            let color = CreateBitmap(width, height, 1, 32, Some(pixels.as_ptr().cast()));
            // An all-zero mask: the colour image's alpha decides.
            let mask_bytes = vec![0u8; (width as usize).div_ceil(16) * 2 * height as usize];
            let mask = CreateBitmap(width, height, 1, 1, Some(mask_bytes.as_ptr().cast()));
            let icon = (!color.is_invalid() && !mask.is_invalid())
                .then(|| {
                    CreateIconIndirect(&ICONINFO {
                        fIcon: TRUE,
                        xHotspot: 0,
                        yHotspot: 0,
                        hbmMask: mask,
                        hbmColor: color,
                    })
                    .ok()
                })
                .flatten();
            let _ = DeleteObject(color.into());
            let _ = DeleteObject(mask.into());
            icon
        })();
        let _ = DeleteObject(info.hbmColor.into());
        let _ = DeleteObject(info.hbmMask.into());
        made
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
                s.show();
                if Shell_NotifyIconW(NIM_ADD, &s.icon).as_bool() {
                    let _ = KillTimer(Some(hwnd), RETRY_TIMER);
                } else if message == taskbar {
                    SetTimer(Some(hwnd), RETRY_TIMER, 3000, None);
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
                        let (action, held) = NOTICES.lock().unwrap().clicked();
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
                            let _ = AppendMenuW(menu, MF_STRING, 1, w!("Open Rubylight"));
                            let _ = AppendMenuW(menu, MF_STRING, 7, w!("Pair a device"));
                            let _ = AppendMenuW(menu, MF_STRING, 6, w!("Check for updates"));
                            if !hide_controls {
                                let _ = AppendMenuW(menu, MF_STRING, 2, w!("Disconnect clients"));
                                let app = APP.lock().unwrap().app.clone();
                                let label: Vec<u16> = match &app {
                                    // An ampersand would underline the next letter.
                                    Some(app) => format!("Quit {}", app.replace('&', "&&")),
                                    None => "No app running".into(),
                                }
                                .encode_utf16()
                                .chain(Some(0))
                                .collect();
                                let flags = if app.is_some() {
                                    MF_STRING
                                } else {
                                    MF_STRING | MF_GRAYED
                                };
                                let _ = AppendMenuW(menu, flags, 5, PCWSTR(label.as_ptr()));
                                let _ = AppendMenuW(menu, MF_STRING, 3, w!("Restart"));
                                let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
                                let _ = AppendMenuW(menu, MF_STRING, 4, w!("Quit"));
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
                            let action = match id {
                                1 => Some(Action::Open),
                                2 => Some(Action::StopSessions),
                                3 => Some(Action::Restart),
                                4 => Some(Action::Quit),
                                5 => Some(Action::QuitApp),
                                6 => Some(Action::CheckUpdates),
                                7 => Some(Action::PairDevice),
                                _ => None,
                            };
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
                    // The executable's own icon (resource 1, see host/build.rs),
                    // else the icon file, else the generic application icon.
                    let (width, height) =
                        (GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON));
                    let path: Vec<u16> = icon
                        .to_string_lossy()
                        .encode_utf16()
                        .chain(Some(0))
                        .collect();
                    let custom = LoadImageW(
                        Some(HINSTANCE(module.0)),
                        PCWSTR(1 as _),
                        IMAGE_ICON,
                        width,
                        height,
                        LR_DEFAULTCOLOR,
                    )
                    .or_else(|_| {
                        LoadImageW(
                            None,
                            PCWSTR(path.as_ptr()),
                            IMAGE_ICON,
                            width,
                            height,
                            LR_LOADFROMFILE,
                        )
                    })
                    .ok();
                    let icon = if let Some(handle) = custom {
                        HICON(handle.0)
                    } else {
                        LoadIconW(None, IDI_APPLICATION)?
                    };
                    let icons = [
                        icon,
                        badged(icon, Icon::Streaming).unwrap_or(icon),
                        badged(icon, Icon::Paused).unwrap_or(icon),
                    ];
                    let mut state = Box::new(State {
                        sender,
                        icons,
                        port,
                        icon: NOTIFYICONDATAW {
                            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
                            uID: 1,
                            uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
                            uCallbackMessage: WM_APP + 1,
                            hIcon: icon,
                            ..Default::default()
                        },
                        taskbar: RegisterWindowMessageW(w!("TaskbarCreated")),
                        hide_controls,
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
                    if !Shell_NotifyIconW(NIM_ADD, &state.icon).as_bool() {
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
                    for badge in &icons[1..] {
                        if *badge != icon {
                            let _ = DestroyIcon(*badge);
                        }
                    }
                    if custom.is_some() {
                        let _ = DestroyIcon(icon);
                    }
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
/// The tray icon's window while it is shown.
static SHOWN: std::sync::Mutex<Option<isize>> = std::sync::Mutex::new(None);
static NOTICES: Mutex<Notices> = Mutex::new(Notices::new());
/// Keeps notices in the order they reach Windows. Held across the call to Windows,
/// so never taken on the tray's window thread: a callback dispatched during that
/// call would wait for itself. [`NOTICES`] is only ever held briefly.
static SHOWING: Mutex<()> = Mutex::new(());
fn notice(title: &str, text: &str, action: Option<NoticeAction>) -> QueuedNotice {
    QueuedNotice {
        title: title.to_owned(),
        text: text.to_owned(),
        action,
        request: None,
    }
}
/// Show a notification from the tray icon, if there is one.
pub fn notify(title: &str, text: &str) {
    let _ = post_notice(&notice(title, text, None));
}
/// Show pairing request `request`'s notification; clicking it opens the console at the PIN.
pub fn notify_pairing(request: &str, title: &str, text: &str) {
    let notice = QueuedNotice {
        request: Some(request.to_owned()),
        ..notice(title, text, Some(NoticeAction::EnterPin))
    };
    if let Err(error) = post_notice(&notice) {
        tracing::debug!(%error, "no tray notification for a pairing request");
    }
}
/// Pairing request `request` ended, so a notification still waiting about it goes.
pub fn pairing_request_ended(request: &str) {
    NOTICES.lock().unwrap().request_ended(request);
}
/// Called from the host's loop: a launch error nobody answered lets others go up.
pub fn release_overdue_notices() {
    let held = NOTICES.lock().unwrap().tick(Instant::now());
    show_held(held);
}
/// Show a notification from the tray icon, or say why it could not be shown.
pub fn try_notify(title: &str, text: &str) -> Result<()> {
    post_notice(&notice(title, text, None))
}
fn launch_error_notice(app: &str, code: u32) -> butterpollo_core::tray::Notice {
    (
        "Launch Error",
        format!(
            "Application {app} exited too fast with code {}. Click here to terminate the stream.",
            code as i32
        ),
    )
}
/// Keep the detached stream open until the user chooses to end it.
pub fn launch_error(app: &str, code: u32) {
    let (title, text) = launch_error_notice(app, code);
    if let Err(error) = post_notice(&notice(title, &text, Some(NoticeAction::QuitApp))) {
        tracing::warn!(%error, "no tray notification for a launch error");
    }
}
/// Put up notices that waited for a launch error's notification to go, on a
/// thread of their own: callers include the window procedure and the host's loop.
fn show_held(held: Vec<QueuedNotice>) {
    if held.is_empty() {
        return;
    }
    let shown = std::thread::Builder::new()
        .name("tray-notices".into())
        .spawn(move || {
            for notice in &held {
                if let Err(error) = post_notice(notice) {
                    tracing::warn!(%error, title = %notice.title, "no tray notification for a held notice");
                }
            }
        });
    if let Err(error) = shown {
        tracing::warn!(%error, "no thread for held tray notifications");
    }
}
fn post_notice(notice: &QueuedNotice) -> Result<()> {
    let Some(window) = *SHOWN.lock().unwrap() else {
        bail!("the tray icon is not shown");
    };
    let showing = SHOWING.lock().unwrap();
    let post = NOTICES.lock().unwrap().post(notice, Instant::now());
    if post == Post::Held {
        tracing::info!(title = %notice.title, "tray notification waits for the launch error's");
        return Ok(());
    }
    let cleared = post != Post::ClearThenShow || show_notification(window, "", "");
    if cleared && show_notification(window, &notice.title, &notice.text) {
        return Ok(());
    }
    let held = NOTICES.lock().unwrap().not_shown();
    drop(showing);
    show_held(held);
    bail!("Windows did not show the notification")
}
/// Empty text takes down what is up.
fn show_notification(window: isize, title: &str, text: &str) -> bool {
    let mut icon = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: HWND(window as *mut _),
        uID: 1,
        uFlags: NIF_INFO,
        dwInfoFlags: NIIF_INFO,
        ..Default::default()
    };
    let copy = |target: &mut [u16], value: &str| {
        let units: Vec<u16> = value.encode_utf16().take(target.len() - 1).collect();
        target[..units.len()].copy_from_slice(&units);
    };
    copy(&mut icon.szInfoTitle, title);
    copy(&mut icon.szInfo, text);
    // SAFETY: icon is initialized with its exact size and terminated text buffers,
    // and Shell_NotifyIconW consumes it synchronously; hWnd is only an opaque identifier.
    unsafe { Shell_NotifyIconW(NIM_MODIFY, &icon) }.as_bool()
}

/// The console's Devices page with a waiting device's PIN field focused.
pub const PAGE_ENTER_PIN: &str = "/devices?pair=pin";
/// The console's Devices page at the QR code that pairs a device.
pub const PAGE_PAIR_QR: &str = "/devices?pair=qr";

fn console_url(port: u16, page: &str) -> String {
    format!("https://localhost:{port}{page}")
}

pub fn open_web(port: u16) -> Result<()> {
    open_web_page(port, "/")
}

/// Open a page of the console in the signed-in user's browser. `page` is one
/// of the `PAGE_` paths or `/`.
pub fn open_web_page(port: u16, page: &str) -> Result<()> {
    if !matches!(page, "/" | PAGE_ENTER_PIN | PAGE_PAIR_QR) {
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
mod tests {
    use super::*;

    #[test]
    fn the_pairing_pages_lead_to_the_devices_page() {
        assert_eq!(console_url(47990, "/"), "https://localhost:47990/");
        assert_eq!(
            console_url(47990, PAGE_ENTER_PIN),
            "https://localhost:47990/devices?pair=pin"
        );
        assert_eq!(
            console_url(47990, PAGE_PAIR_QR),
            "https://localhost:47990/devices?pair=qr"
        );
        assert!(open_web_page(47990, "//evil.example/").is_err());
    }

    #[test]
    fn launch_error_uses_vibepollos_wording_and_signed_exit_code() {
        assert_eq!(
            launch_error_notice("Game", 1),
            (
                "Launch Error",
                "Application Game exited too fast with code 1. Click here to terminate the stream."
                    .into()
            )
        );
        assert!(
            launch_error_notice("Game", 0xc0000005)
                .1
                .contains("code -1073741819.")
        );
    }
}
