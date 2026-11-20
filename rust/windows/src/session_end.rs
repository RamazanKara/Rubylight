//! The end of the Windows session (sign-out or shutdown), heard by a hidden window.
//!
//! Windows announces it with WM_QUERYENDSESSION and WM_ENDSESSION, sent only to top-level
//! windows: a process that loads user32 gets no console control event for it, and
//! message-only windows (HWND_MESSAGE) get neither message. After WM_ENDSESSION returns,
//! Windows may end the process at any moment, so whatever must happen first happens before
//! the window procedure returns, within the few seconds Windows waits for it.
use anyhow::{Result, bail};
use std::{cell::RefCell, sync::mpsc, thread};
use windows::{
    Win32::{
        Foundation::*,
        System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
        UI::WindowsAndMessaging::*,
    },
    core::w,
};

/// How Windows ends the session, from WM_ENDSESSION's lParam, for the log.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ending {
    /// The user signs out; otherwise the PC shuts down or restarts.
    pub logoff: bool,
    /// Windows ends the session whatever the programs answer.
    pub critical: bool,
    /// Restart Manager asks the program itself to close.
    pub close_app: bool,
}
impl Ending {
    fn from_lparam(lparam: u32) -> Self {
        Self {
            logoff: lparam & ENDSESSION_LOGOFF != 0,
            critical: lparam & ENDSESSION_CRITICAL != 0,
            close_app: lparam & ENDSESSION_CLOSEAPP != 0,
        }
    }
}

/// What the window does with a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reaction {
    /// WM_QUERYENDSESSION: the end is allowed. The host never vetoes or delays it.
    Allow,
    /// WM_ENDSESSION with wParam TRUE: the session ends; the handler runs before the reply.
    End(Ending),
    /// WM_ENDSESSION with wParam FALSE: another program cancelled the end; nothing happens.
    Cancelled,
    /// Any other message goes to DefWindowProcW.
    Default,
}

/// The decision for one message, apart from any window.
pub fn reaction(message: u32, wparam: usize, lparam: isize) -> Reaction {
    match message {
        WM_QUERYENDSESSION => Reaction::Allow,
        WM_ENDSESSION if wparam != 0 => Reaction::End(Ending::from_lparam(lparam as u32)),
        WM_ENDSESSION => Reaction::Cancelled,
        _ => Reaction::Default,
    }
}

type Handler = Box<dyn FnOnce(Ending) + Send>;

thread_local! {
    /// The handler of the window on this thread; taken by the first WM_ENDSESSION, so it runs
    /// at most once.
    static ON_END: RefCell<Option<Handler>> = const { RefCell::new(None) };
}

const CLASS: windows::core::PCWSTR = w!("ButterpolloSessionEnd");

unsafe extern "system" fn window(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match reaction(message, wparam.0, lparam.0) {
        // TRUE: never block the end of the session.
        Reaction::Allow => LRESULT(1),
        Reaction::End(ending) => {
            // Taken out first, so the handler holds no borrow while it runs.
            let handler = ON_END.with(|on_end| on_end.borrow_mut().take());
            if let Some(handler) = handler {
                handler(ending);
            }
            LRESULT(0)
        }
        Reaction::Cancelled => LRESULT(0),
        // SAFETY: The arguments are the ones Windows passed to this window procedure.
        Reaction::Default => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

/// A hidden top-level window on a thread of its own that calls a handler when the Windows
/// session ends. Dropping it closes the window.
pub struct SessionEndWatch {
    thread: u32,
    worker: Option<thread::JoinHandle<()>>,
}
impl SessionEndWatch {
    /// Calls `on_end` once, on the window's thread, when Windows sends WM_ENDSESSION for an
    /// ending session. Windows waits for the window's reply, so the session ends only after
    /// `on_end` returns: it must return within a few seconds.
    pub fn start(on_end: impl FnOnce(Ending) + Send + 'static) -> Result<Self> {
        let (ready, started) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("session-end".into())
            .spawn(move || {
                ON_END.with(|slot| *slot.borrow_mut() = Some(Box::new(on_end)));
                // SAFETY: This thread registers the class, creates, dispatches and destroys the
                // window; `message` is a live local for every call.
                let result = unsafe {
                    (|| -> Result<HWND> {
                        let module = GetModuleHandleW(None)?;
                        let class = WNDCLASSW {
                            lpfnWndProc: Some(window),
                            hInstance: HINSTANCE(module.0),
                            lpszClassName: CLASS,
                            ..Default::default()
                        };
                        if RegisterClassW(&class) == 0
                            && GetLastError() != ERROR_CLASS_ALREADY_EXISTS
                        {
                            bail!("cannot register the session-end window class");
                        }
                        // Top-level (no parent) and without WS_VISIBLE: hidden, yet sent the
                        // end-session messages.
                        Ok(CreateWindowExW(
                            WINDOW_EX_STYLE(0),
                            CLASS,
                            w!("Butterpollo session end"),
                            WINDOW_STYLE(0),
                            0,
                            0,
                            0,
                            0,
                            None,
                            None,
                            Some(HINSTANCE(module.0)),
                            None,
                        )?)
                    })()
                };
                let hwnd = match result {
                    Ok(hwnd) => hwnd,
                    Err(error) => {
                        let _ = ready.send(Err(error));
                        return;
                    }
                };
                // SAFETY: As above; the window belongs to this thread.
                unsafe {
                    let _ = ready.send(Ok(GetCurrentThreadId()));
                    let mut message = MSG::default();
                    while GetMessageW(&mut message, None, 0, 0).0 > 0 {
                        let _ = TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }
                    let _ = DestroyWindow(hwnd);
                }
            })?;
        match started.recv() {
            Ok(Ok(thread)) => Ok(Self {
                thread,
                worker: Some(worker),
            }),
            Ok(Err(error)) => {
                let _ = worker.join();
                Err(error)
            }
            Err(_) => {
                let _ = worker.join();
                bail!("the session-end window thread stopped")
            }
        }
    }
}
impl Drop for SessionEndWatch {
    fn drop(&mut self) {
        // SAFETY: PostThreadMessageW only takes the worker's thread id and plain values; a stale
        // id fails.
        unsafe {
            let _ = PostThreadMessageW(self.thread, WM_QUIT, WPARAM(0), LPARAM(0));
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_end_of_the_session_is_never_vetoed() {
        for lparam in [
            0,
            ENDSESSION_LOGOFF as isize,
            ENDSESSION_CRITICAL as isize,
            ENDSESSION_CLOSEAPP as isize,
        ] {
            assert_eq!(reaction(WM_QUERYENDSESSION, 0, lparam), Reaction::Allow);
        }
    }

    #[test]
    fn only_an_end_that_goes_ahead_runs_the_handler() {
        assert_eq!(
            reaction(WM_ENDSESSION, 1, 0),
            Reaction::End(Ending::default())
        );
        assert_eq!(
            reaction(WM_ENDSESSION, 1, ENDSESSION_LOGOFF as isize),
            Reaction::End(Ending {
                logoff: true,
                ..Ending::default()
            })
        );
        assert_eq!(
            reaction(
                WM_ENDSESSION,
                1,
                (ENDSESSION_CRITICAL | ENDSESSION_CLOSEAPP) as isize
            ),
            Reaction::End(Ending {
                critical: true,
                close_app: true,
                ..Ending::default()
            })
        );
        // Another program refused: the session goes on.
        assert_eq!(
            reaction(WM_ENDSESSION, 0, ENDSESSION_LOGOFF as isize),
            Reaction::Cancelled
        );
        assert_eq!(reaction(WM_CLOSE, 1, 0), Reaction::Default);
    }

    #[test]
    fn the_window_starts_and_closes() {
        let watch = SessionEndWatch::start(|_| {}).expect("session-end window");
        drop(watch);
    }
}
