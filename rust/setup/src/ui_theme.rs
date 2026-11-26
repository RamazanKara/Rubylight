//! Setup's 7.0 look: whether Windows is in dark app mode, a best-effort dark
//! title bar, Per-Monitor-V2 DPI awareness and the brand
//! icon task dialogs show. Everything here is best effort: a failure leaves
//! Windows' default look and never stops setup.
use windows::{
    Win32::{
        Foundation::{HMODULE, HWND},
        Graphics::Dwm::{DWMWINDOWATTRIBUTE, DwmSetWindowAttribute},
        System::{
            LibraryLoader::{GetModuleHandleW, GetProcAddress},
            Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW},
        },
    },
    core::{BOOL, PCWSTR, s, w},
};

/// Whether apps are dark: `AppsUseLightTheme` is 0. Light when the value is
/// missing, as on Windows 10 before 1809.
pub fn dark_mode() -> bool {
    let mut value = 1u32;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: the key and value names are static NUL-terminated strings, and `value`
    // and `size` are valid for the DWORD the call writes.
    let read = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut value as *mut u32).cast()),
            Some(&mut size),
        )
    };
    read.is_ok() && value == 0
}

/// `DWMWA_USE_IMMERSIVE_DARK_MODE` since Windows 10 20H1, and its number on
/// 1809 to 1909.
const DARK_TITLE_BAR: [DWMWINDOWATTRIBUTE; 2] = [DWMWINDOWATTRIBUTE(20), DWMWINDOWATTRIBUTE(19)];

/// Give `window` a dark title bar when `dark`; ignored where Windows can't.
pub fn title_bar(window: HWND, dark: bool) {
    if !dark {
        return;
    }
    let on = BOOL::from(true);
    for attribute in DARK_TITLE_BAR {
        // SAFETY: `on` is a BOOL that outlives the synchronous call; a window that is
        // gone or an attribute Windows does not know only fails.
        let set = unsafe {
            DwmSetWindowAttribute(
                window,
                attribute,
                (&on as *const BOOL).cast(),
                size_of::<BOOL>() as u32,
            )
        };
        if set.is_ok() {
            return;
        }
    }
}

/// Per-Monitor-V2 DPI awareness for this process, before any window: dialogs then
/// render sharp on every monitor. Windows 10 before 1703 lacks it and stays as it was.
pub fn per_monitor_dpi() -> bool {
    // DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2.
    const PER_MONITOR_AWARE_V2: isize = -4;
    type SetContext = unsafe extern "system" fn(isize) -> BOOL;
    // SAFETY: user32 is loaded in every GUI process; the address, when present, is
    // SetProcessDpiAwarenessContext, whose signature `SetContext` matches.
    unsafe {
        let Ok(user32) = GetModuleHandleW(w!("user32.dll")) else {
            return false;
        };
        let Some(function) = GetProcAddress(user32, s!("SetProcessDpiAwarenessContext")) else {
            return false;
        };
        let set = std::mem::transmute::<unsafe extern "system" fn() -> isize, SetContext>(function);
        set(PER_MONITOR_AWARE_V2).as_bool()
    }
}

/// This executable, whose icon resource 1 is the Rubylight mark (see build.rs).
pub fn module() -> Option<HMODULE> {
    // SAFETY: the handle of the running executable needs no release.
    unsafe { GetModuleHandleW(PCWSTR::null()) }.ok()
}

/// The resource id of the brand icon, as a task dialog's main icon.
pub const BRAND_ICON: PCWSTR = PCWSTR(std::ptr::without_provenance(1));

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_queries_never_fail_setup() {
        // Whatever this PC says, reading it neither panics nor blocks.
        let _ = dark_mode();
        title_bar(HWND::default(), true);
        title_bar(HWND::default(), false);
        assert!(module().is_some());
    }
}
