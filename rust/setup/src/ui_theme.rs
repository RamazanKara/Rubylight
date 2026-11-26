//! Setup's 7.0 look: the colours it draws with (mirroring
//! `rust/web/src/styles/tokens.css`), whether Windows is in dark app mode, a
//! best-effort dark title bar, Per-Monitor-V2 DPI awareness and the brand
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

/// The 7.0 surface colours setup draws with, as RGB.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub bg: [u8; 3],
    pub panel: [u8; 3],
    pub ink: [u8; 3],
    pub muted: [u8; 3],
    pub line: [u8; 3],
    pub accent: [u8; 3],
    pub brand: [u8; 3],
}

/// Alabaster Mineral, the light theme.
pub const LIGHT: Palette = Palette {
    bg: [0xf3, 0xf4, 0xf7],
    panel: [0xff, 0xff, 0xff],
    ink: [0x11, 0x13, 0x18],
    muted: [0x59, 0x61, 0x70],
    line: [0xd8, 0xdc, 0xe4],
    accent: [0xc8, 0x19, 0x42],
    brand: [0xc4, 0x12, 0x42],
};

/// Obsidian Studio, the dark theme.
pub const DARK: Palette = Palette {
    bg: [0x09, 0x0a, 0x0d],
    panel: [0x11, 0x13, 0x19],
    ink: [0xf5, 0xf7, 0xfa],
    muted: [0x9a, 0xa3, 0xb5],
    line: [0x23, 0x28, 0x36],
    accent: [0xce, 0x1e, 0x49],
    brand: [0xc4, 0x12, 0x42],
};

/// The palette for Windows' app mode.
pub fn palette(dark: bool) -> &'static Palette {
    if dark { &DARK } else { &LIGHT }
}

/// `rgb` as a GDI COLORREF (0x00BBGGRR).
pub fn colorref(rgb: [u8; 3]) -> u32 {
    u32::from(rgb[0]) | u32::from(rgb[1]) << 8 | u32::from(rgb[2]) << 16
}

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

    /// The first value of `--name` in `block`.
    fn token(block: &str, name: &str) -> [u8; 3] {
        let start = block.find(&format!("{name}:")).expect(name) + name.len() + 1;
        let value = block[start..].split(';').next().unwrap().trim();
        let hex = value.strip_prefix('#').expect(value);
        assert_eq!(hex.len(), 6, "{name}: {value}");
        [0, 2, 4].map(|at| u8::from_str_radix(&hex[at..at + 2], 16).unwrap())
    }

    fn from_tokens(block: &str) -> Palette {
        Palette {
            bg: token(block, "--bg"),
            panel: token(block, "--panel"),
            ink: token(block, "--ink"),
            muted: token(block, "--muted"),
            line: token(block, "--line"),
            accent: token(block, "--accent"),
            brand: token(block, "--brand"),
        }
    }

    #[test]
    fn the_palettes_are_the_70_tokens() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../web/src/styles/tokens.css");
        let css = std::fs::read_to_string(&path).expect("rust/web/src/styles/tokens.css");
        let dark_at = css
            .find(":root[data-theme='dark']")
            .expect("the dark block");
        let system_at = css
            .find("@media (prefers-color-scheme: dark)")
            .expect("system dark");
        assert_eq!(from_tokens(&css[..dark_at]), LIGHT);
        assert_eq!(from_tokens(&css[dark_at..system_at]), DARK);
        assert_eq!(from_tokens(&css[system_at..]), DARK);
        assert_eq!(palette(false), &LIGHT);
        assert_eq!(palette(true), &DARK);
    }

    #[test]
    fn colours_become_colorrefs() {
        assert_eq!(colorref([0xc4, 0x12, 0x42]), 0x0042_12c4);
        assert_eq!(colorref(DARK.bg), 0x000d_0a09);
    }

    #[test]
    fn theme_queries_never_fail_setup() {
        // Whatever this PC says, reading it neither panics nor blocks.
        let _ = dark_mode();
        title_bar(HWND::default(), true);
        title_bar(HWND::default(), false);
        assert!(module().is_some());
    }
}
