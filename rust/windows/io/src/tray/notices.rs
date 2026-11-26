//! Balloon notifications and held-notice dispatch for the Windows tray icon.
use anyhow::{Result, bail};
use butterpollo_session::tray::{NoticeAction, Notices, Post, QueuedNotice};
use std::{mem::size_of, sync::Mutex, time::Instant};
use windows::Win32::{
    Foundation::HWND,
    UI::Shell::{NIF_INFO, NIIF_INFO, NIM_MODIFY, NOTIFYICONDATAW, Shell_NotifyIconW},
};

/// The tray icon's window while it is shown.
pub(super) static SHOWN: std::sync::Mutex<Option<isize>> = std::sync::Mutex::new(None);
pub(super) static NOTICES: Mutex<Notices> = Mutex::new(Notices::new());
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

/// When [`release_overdue_notices`] next has something to release.
pub fn notices_due() -> Option<Instant> {
    NOTICES.lock().unwrap().due()
}

/// Show a notification from the tray icon, or say why it could not be shown.
pub fn try_notify(title: &str, text: &str) -> Result<()> {
    post_notice(&notice(title, text, None))
}

/// What happened, then what to do: the stream stays open until the user ends it.
pub(super) fn launch_error_notice(app: &str, code: u32) -> butterpollo_session::tray::Notice {
    (
        "Launch failed",
        format!(
            "{app} closed right after it started (exit code {}). Click to end the stream.",
            code as i32
        ),
    )
}

/// Keep the detached stream open until the user chooses to end it.
pub fn launch_error(app: &str, code: u32) {
    super::launch_failed(app);
    let (title, text) = launch_error_notice(app, code);
    if let Err(error) = post_notice(&notice(title, &text, Some(NoticeAction::QuitApp))) {
        tracing::warn!(%error, "no tray notification for a launch error");
    }
}

/// Put up notices that waited for a launch error's notification to go, on a
/// thread of their own: callers include the window procedure and the host's loop.
pub(super) fn show_held(held: Vec<QueuedNotice>) {
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
