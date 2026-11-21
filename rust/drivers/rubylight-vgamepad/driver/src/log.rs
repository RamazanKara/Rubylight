//! DebugView logging for lifecycle diagnostics (avoid calling on hot 1 kHz input paths).

use windows::Win32::System::Diagnostics::Debug::OutputDebugStringW;
use windows::core::HSTRING;

pub fn write(message: &str) {
    let text = HSTRING::from(message);
    // SAFETY: `text` is a valid null-terminated UTF-16 string living across the call.
    unsafe {
        OutputDebugStringW(&text);
    }
}

macro_rules! dbglog {
    ($($arg:tt)*) => {{
        let line = format!("[rlvg] {}\n", format_args!($($arg)*));
        $crate::log::write(&line);
    }};
}
