//! Native Windows task dialogs. TaskDialogIndirect lives in Common Controls
//! 6, which a process only gets through a manifest: setup activates one at
//! run time and loads the function then, so it starts on any system.
use std::sync::{Arc, Mutex};
use windows::{
    Win32::{
        Foundation::{FreeLibrary, HMODULE, HWND, LPARAM, S_FALSE, S_OK, WPARAM},
        System::{
            ApplicationInstallationAndServicing::{ACTCTXW, ActivateActCtx, CreateActCtxW},
            LibraryLoader::{
                GetModuleHandleW, GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
            },
        },
        UI::{
            Controls::*,
            WindowsAndMessaging::{
                IDCANCEL, IDCLOSE, IDOK, MB_ICONERROR, MB_OK, MessageBoxW, SendMessageW,
            },
        },
    },
    core::{BOOL, HRESULT, HSTRING, PCWSTR, w},
};

type TaskDialogIndirect =
    unsafe extern "system" fn(*const TASKDIALOGCONFIG, *mut i32, *mut i32, *mut BOOL) -> HRESULT;

const ISOLATIONAWARE_MANIFEST_RESOURCE_ID: PCWSTR = PCWSTR(std::ptr::without_provenance(2));
const ACTCTX_FLAG_RESOURCE_NAME_VALID: u32 = 0x0000_0008;
const ACTCTX_FLAG_HMODULE_VALID: u32 = 0x0000_0080;

/// TaskDialogIndirect from Common Controls 6, loaded once.
fn task_dialog() -> Option<TaskDialogIndirect> {
    static FUNCTION: std::sync::OnceLock<Option<usize>> = std::sync::OnceLock::new();
    // SAFETY: the embedded manifest resource lives in the current module and `library` stays loaded while the address is in use.
    let address = FUNCTION.get_or_init(|| unsafe {
        let module = GetModuleHandleW(None).ok()?;
        let context = CreateActCtxW(&ACTCTXW {
            cbSize: size_of::<ACTCTXW>() as u32,
            dwFlags: ACTCTX_FLAG_HMODULE_VALID | ACTCTX_FLAG_RESOURCE_NAME_VALID,
            lpResourceName: ISOLATIONAWARE_MANIFEST_RESOURCE_ID,
            hModule: module,
            ..Default::default()
        })
        .ok()?;
        let mut cookie = 0;
        ActivateActCtx(Some(context), &mut cookie).ok()?;
        let library: HMODULE =
            LoadLibraryExW(w!("comctl32.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32).ok()?;
        match GetProcAddress(library, windows::core::s!("TaskDialogIndirect")) {
            Some(function) => Some(function as usize),
            None => {
                let _ = FreeLibrary(library);
                None
            }
        }
    });
    // SAFETY: the address is TaskDialogIndirect from comctl32 6, whose signature `TaskDialogIndirect` matches.
    address.map(|a| unsafe { std::mem::transmute::<usize, TaskDialogIndirect>(a) })
}

pub fn message_box(title: &str, text: &str) {
    let title = HSTRING::from(title);
    let text = HSTRING::from(text);
    // SAFETY: both strings are HSTRINGs that outlive the call.
    unsafe {
        MessageBoxW(None, &text, &title, MB_OK | MB_ICONERROR);
    }
}

pub struct Choice {
    pub accepted: bool,
    /// The second button was chosen instead.
    pub other: bool,
    pub checked: bool,
}
/// A question with a confirming button, an optional second button, Cancel
/// and an optional checkbox.
pub fn ask(
    title: &str,
    heading: &str,
    text: &str,
    confirm: &str,
    other: Option<&str>,
    checkbox: Option<(&str, bool)>,
) -> Choice {
    let Some(dialog) = task_dialog() else {
        // Without Common Controls 6, a plain question.
        use windows::Win32::UI::WindowsAndMessaging::{IDYES, MB_ICONQUESTION, MB_YESNO};
        let body = HSTRING::from(format!("{heading}\n\n{text}"));
        let caption = HSTRING::from(title);
        // SAFETY: both strings are HSTRINGs that outlive the call.
        let answer = unsafe { MessageBoxW(None, &body, &caption, MB_YESNO | MB_ICONQUESTION) };
        return Choice {
            accepted: answer == IDYES,
            other: false,
            checked: checkbox.is_some_and(|(_, checked)| checked),
        };
    };
    let title = HSTRING::from(title);
    let heading = HSTRING::from(heading);
    let text = HSTRING::from(text);
    let confirm = HSTRING::from(confirm);
    let other = other.map(HSTRING::from);
    let label = checkbox.map(|(label, _)| HSTRING::from(label));
    let mut buttons = vec![TASKDIALOG_BUTTON {
        nButtonID: IDOK.0,
        pszButtonText: PCWSTR(confirm.as_ptr()),
    }];
    if let Some(other) = &other {
        buttons.push(TASKDIALOG_BUTTON {
            nButtonID: OTHER_BUTTON,
            pszButtonText: PCWSTR(other.as_ptr()),
        });
    }
    let mut flags = TDF_ALLOW_DIALOG_CANCELLATION | TDF_POSITION_RELATIVE_TO_WINDOW;
    if checkbox.is_some_and(|(_, checked)| checked) {
        flags |= TDF_VERIFICATION_FLAG_CHECKED;
    }
    let config = TASKDIALOGCONFIG {
        cbSize: size_of::<TASKDIALOGCONFIG>() as u32,
        dwFlags: flags,
        dwCommonButtons: TDCBF_CANCEL_BUTTON,
        pszWindowTitle: PCWSTR(title.as_ptr()),
        // Setup runs elevated already and none of its questions warns of a risk:
        // the Rubylight mark, not the shield.
        hInstance: brand_instance(),
        Anonymous1: TASKDIALOGCONFIG_0 {
            pszMainIcon: brand_icon(),
        },
        pfCallback: Some(themed_callback),
        pszMainInstruction: PCWSTR(heading.as_ptr()),
        pszContent: PCWSTR(text.as_ptr()),
        cButtons: buttons.len() as u32,
        pButtons: buttons.as_ptr(),
        nDefaultButton: IDOK.0,
        pszVerificationText: label
            .as_ref()
            .map_or(PCWSTR::null(), |l| PCWSTR(l.as_ptr())),
        ..Default::default()
    };
    let mut button = 0;
    let mut checked = BOOL(0);
    // SAFETY: `config` and every string and button it points at outlive the modal call; the out-pointers are valid or null.
    let result = unsafe { dialog(&config, &mut button, std::ptr::null_mut(), &mut checked) };
    Choice {
        accepted: result.is_ok() && button == IDOK.0,
        other: result.is_ok() && button == OTHER_BUTTON,
        checked: checked.as_bool(),
    }
}
const OTHER_BUTTON: i32 = 101;

/// This executable, whose icon resource is the main icon of the dialogs that show the mark.
fn brand_instance() -> windows::Win32::Foundation::HINSTANCE {
    crate::ui_theme::module()
        .map(|module| windows::Win32::Foundation::HINSTANCE(module.0))
        .unwrap_or_default()
}

/// The Rubylight mark, or Windows' information icon if the executable has none.
fn brand_icon() -> PCWSTR {
    if brand_instance().is_invalid() {
        TD_INFORMATION_ICON
    } else {
        crate::ui_theme::BRAND_ICON
    }
}

/// Dark title bar once the dialog exists, when Windows' apps are dark.
unsafe extern "system" fn themed_callback(
    hwnd: HWND,
    message: TASKDIALOG_NOTIFICATIONS,
    _: WPARAM,
    _: LPARAM,
    _: isize,
) -> HRESULT {
    if message == TDN_CREATED {
        #[cfg(test)]
        if let Some((dark, created)) = *DUMP_HOOK.lock().unwrap() {
            crate::ui_theme::title_bar(hwnd, dark);
            created(hwnd.0 as isize);
            return S_OK;
        }
        crate::ui_theme::title_bar(hwnd, crate::ui_theme::dark_mode());
    }
    S_OK
}
/// For the dialog dump: the title bar to show, and what to call once a dialog exists.
#[cfg(test)]
pub(crate) static DUMP_HOOK: Mutex<Option<DumpHook>> = Mutex::new(None);
/// Dark title bar or not, and the call that gets the dialog's window.
#[cfg(test)]
pub(crate) type DumpHook = (bool, fn(isize));

/// The Windows folder picker, opened at `start` or its nearest existing
/// parent. None when cancelled.
pub fn pick_folder(title: &str, start: &std::path::Path) -> Option<std::path::PathBuf> {
    use windows::Win32::System::Com::*;
    use windows::Win32::UI::Shell::*;
    // SAFETY: COM is initialised on this thread before the dialog is used and released after it is dropped; the path string the dialog returns is freed with CoTaskMemFree once copied.
    unsafe {
        let initialized = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
        let picked = (|| -> windows::core::Result<std::path::PathBuf> {
            let dialog: IFileOpenDialog =
                CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
            dialog.SetOptions(
                dialog.GetOptions()? | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST,
            )?;
            dialog.SetTitle(&HSTRING::from(title))?;
            if let Some(existing) = start.ancestors().find(|folder| folder.is_dir()) {
                let item: windows::core::Result<IShellItem> =
                    SHCreateItemFromParsingName(&HSTRING::from(existing.as_os_str()), None);
                if let Ok(item) = item {
                    let _ = dialog.SetFolder(&item);
                }
            }
            dialog.Show(None)?;
            let name = dialog.GetResult()?.GetDisplayName(SIGDN_FILESYSPATH)?;
            let path = std::path::PathBuf::from(name.to_string().unwrap_or_default());
            CoTaskMemFree(Some(name.0 as *const _));
            Ok(path)
        })();
        if initialized {
            CoUninitialize();
        }
        picked.ok().filter(|path| !path.as_os_str().is_empty())
    }
}

/// Shared state between the work and the progress dialog.
pub struct Progress {
    text: Mutex<String>,
    done: std::sync::atomic::AtomicBool,
}
impl Progress {
    pub fn set(&self, text: &str) {
        crate::log::line(format!("== {text}"));
        *self.text.lock().unwrap() = text.to_owned();
    }
}
unsafe extern "system" fn progress_callback(
    hwnd: HWND,
    message: TASKDIALOG_NOTIFICATIONS,
    _: WPARAM,
    _: LPARAM,
    data: isize,
) -> HRESULT {
    // SAFETY: `data` is the `lpCallbackData` set in `progress`, which keeps that state alive until the dialog closes.
    let state = unsafe { &*(data as *const (Arc<Progress>, Mutex<String>)) };
    // SAFETY: `hwnd` is the dialog that called back, valid for the duration of the notification.
    unsafe {
        match message {
            TDN_CREATED => {
                crate::ui_theme::title_bar(hwnd, crate::ui_theme::dark_mode());
                SendMessageW(
                    hwnd,
                    TDM_SET_PROGRESS_BAR_MARQUEE.0 as u32,
                    Some(WPARAM(1)),
                    Some(LPARAM(30)),
                );
            }
            TDN_TIMER => {
                let text = state.0.text.lock().unwrap().clone();
                let mut shown = state.1.lock().unwrap();
                if *shown != text {
                    let wide = HSTRING::from(text.as_str());
                    SendMessageW(
                        hwnd,
                        TDM_SET_ELEMENT_TEXT.0 as u32,
                        Some(WPARAM(TDE_CONTENT.0 as usize)),
                        Some(LPARAM(wide.as_ptr() as isize)),
                    );
                    *shown = text;
                }
                if state.0.done.load(std::sync::atomic::Ordering::Acquire) {
                    SendMessageW(
                        hwnd,
                        TDM_CLICK_BUTTON.0 as u32,
                        Some(WPARAM(IDCANCEL.0 as usize)),
                        Some(LPARAM(0)),
                    );
                }
            }
            // Setup cannot be interrupted halfway; the dialog closes itself.
            TDN_BUTTON_CLICKED if !state.0.done.load(std::sync::atomic::Ordering::Acquire) => {
                return S_FALSE;
            }
            _ => {}
        }
    }
    S_OK
}
/// Run `work` on another thread while a progress dialog shows its steps.
pub fn progress<T: Send + 'static>(
    title: &str,
    heading: &str,
    quiet: bool,
    work: impl FnOnce(Arc<Progress>) -> T + Send + 'static,
) -> T {
    let state = Arc::new(Progress {
        text: Mutex::new("Preparing…".into()),
        done: std::sync::atomic::AtomicBool::new(false),
    });
    let worker = {
        let state = state.clone();
        std::thread::spawn(move || {
            // Set on the way out, also when `work` panics: the dialog closes only
            // on this flag, and the panic is reported by the join below.
            struct Done(Arc<Progress>);
            impl Drop for Done {
                fn drop(&mut self) {
                    self.0
                        .done
                        .store(true, std::sync::atomic::Ordering::Release);
                }
            }
            let done = Done(state);
            work(done.0.clone())
        })
    };
    if !quiet && let Some(dialog) = task_dialog() {
        let title = HSTRING::from(title);
        let heading = HSTRING::from(heading);
        let initial = HSTRING::from("Preparing…");
        let callback_state: Box<(Arc<Progress>, Mutex<String>)> =
            Box::new((state.clone(), Mutex::new(String::new())));
        let config = TASKDIALOGCONFIG {
            cbSize: size_of::<TASKDIALOGCONFIG>() as u32,
            dwFlags: TDF_SHOW_MARQUEE_PROGRESS_BAR | TDF_CALLBACK_TIMER,
            dwCommonButtons: TDCBF_CANCEL_BUTTON,
            pszWindowTitle: PCWSTR(title.as_ptr()),
            pszMainInstruction: PCWSTR(heading.as_ptr()),
            pszContent: PCWSTR(initial.as_ptr()),
            pfCallback: Some(progress_callback),
            lpCallbackData: &*callback_state as *const _ as isize,
            ..Default::default()
        };
        // SAFETY: `config` and the state it points at outlive the modal call; the out-pointers are null.
        unsafe {
            let _ = dialog(
                &config,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
        }
        drop(callback_state);
    }
    worker.join().expect("setup work panicked")
}
/// The button that closes the final message.
pub const DONE: &str = "Done";
/// The final message. With `action`, offers that button and reports
/// whether it was chosen.
pub fn finished(
    title: &str,
    heading: &str,
    text: &str,
    success: bool,
    action: Option<&str>,
) -> bool {
    let Some(dialog) = task_dialog() else {
        message_box(title, &format!("{heading}\n\n{text}"));
        return false;
    };
    let title = HSTRING::from(title);
    let heading = HSTRING::from(heading);
    let text = HSTRING::from(text);
    let action = action.map(HSTRING::from);
    let done = HSTRING::from(DONE);
    // The action first, as before: with no default set, the first button is the default.
    let buttons: Vec<TASKDIALOG_BUTTON> = action
        .iter()
        .map(|label| TASKDIALOG_BUTTON {
            nButtonID: 100,
            pszButtonText: PCWSTR(label.as_ptr()),
        })
        .chain([TASKDIALOG_BUTTON {
            nButtonID: IDCLOSE.0,
            pszButtonText: PCWSTR(done.as_ptr()),
        }])
        .collect();
    let config = TASKDIALOGCONFIG {
        cbSize: size_of::<TASKDIALOGCONFIG>() as u32,
        dwFlags: TDF_ALLOW_DIALOG_CANCELLATION,
        pszWindowTitle: PCWSTR(title.as_ptr()),
        // A failure keeps Windows' error icon; success shows the Rubylight mark.
        hInstance: if success {
            brand_instance()
        } else {
            Default::default()
        },
        Anonymous1: TASKDIALOGCONFIG_0 {
            pszMainIcon: if success { brand_icon() } else { TD_ERROR_ICON },
        },
        pfCallback: Some(themed_callback),
        pszMainInstruction: PCWSTR(heading.as_ptr()),
        pszContent: PCWSTR(text.as_ptr()),
        cButtons: buttons.len() as u32,
        pButtons: buttons.as_ptr(),
        ..Default::default()
    };
    let mut button = 0;
    // SAFETY: `config` and every string and button it points at outlive the modal call.
    unsafe {
        let _ = dialog(
            &config,
            &mut button,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );
    }
    button == 100
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_dialog_loads_from_embedded_manifest_without_temp_file() {
        let stale = std::env::temp_dir().join("butterpollo-setup-controls.manifest");
        let _ = std::fs::remove_file(&stale);
        assert!(task_dialog().is_some());
        assert!(!stale.exists());
    }
}

#[cfg(test)]
#[path = "ui_dump_tests.rs"]
mod dump_tests;
