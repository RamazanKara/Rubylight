//! The driver's log. Every line lands in a bounded ring the host drains over
//! `IOCTL_DRAIN_LOG` into its own log, because WUDFHost runs in session 0 under LocalService
//! where no debugger view reaches. With `RLVD_DEBUG_LOG` set (or in a debug build) lines are
//! also sent to the debugger and appended to `rlvd-driver.log` in WUDFHost's own temp folder.
//!
//! From punktfunk's pf-vdisplay `log.rs` and pf-umdf-util `log.rs` by Enrico Bühler (unom),
//! MIT OR Apache-2.0.

use std::collections::VecDeque;
use std::fs::File;
use std::sync::{Mutex, OnceLock};

use rubylight_vdisplay_proto::control;

/// Lines the host has not drained. Bounded, so a per-frame failure loop costs a fixed amount:
/// the oldest line goes and `dropped` counts it.
const RING_LINES: usize = 256;
/// Longest line kept, so one record always fits the host's drain buffer.
const MAX_LINE: usize = 512;

struct Ring {
    lines: VecDeque<(u8, String)>,
    dropped: u64,
}

fn ring() -> &'static Mutex<Ring> {
    static R: OnceLock<Mutex<Ring>> = OnceLock::new();
    R.get_or_init(|| {
        Mutex::new(Ring {
            lines: VecDeque::new(),
            dropped: 0,
        })
    })
}

fn push(level: u8, line: &str) {
    let end = (0..=MAX_LINE.min(line.len()))
        .rev()
        .find(|&i| line.is_char_boundary(i))
        .unwrap_or(0);
    let Ok(mut r) = ring().lock() else { return };
    if r.lines.len() >= RING_LINES {
        r.lines.pop_front();
        r.dropped += 1;
    }
    r.lines.push_back((level, line[..end].to_string()));
}

/// Answer `IOCTL_DRAIN_LOG`: whole records, oldest first, up to `cap` bytes. What does not
/// fit waits for the next call, except a record larger than `cap` alone, which is dropped.
pub fn drain(cap: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let Ok(mut r) = ring().lock() else { return out };
    if r.dropped > 0 {
        let n = std::mem::take(&mut r.dropped);
        let line = format!("[rlvd] log ring overflowed; {n} lines dropped");
        control::write_log_record(&mut out, control::LOG_WARN, &line);
    }
    while let Some((level, line)) = r.lines.pop_front() {
        let start = out.len();
        control::write_log_record(&mut out, level, &line);
        if out.len() > cap {
            out.truncate(start);
            if start == 0 {
                r.dropped += 1;
            } else {
                r.lines.push_front((level, line));
            }
            break;
        }
    }
    out
}

/// A driver knob: the process environment first, then the machine environment in the
/// registry, where `setx /M` writes. WUDFHost inherits a boot-time environment, so the
/// registry read makes a device restart enough for a new value.
pub(crate) fn knob(name: &str) -> Option<String> {
    std::env::var(name).ok().or_else(|| machine_env(name))
}

fn machine_env(name: &str) -> Option<String> {
    use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
    use windows::core::{HSTRING, PCWSTR};
    const KEY: &str = r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment";
    let (subkey, value) = (HSTRING::from(KEY), HSTRING::from(name));
    let mut buf = [0u16; 256];
    let mut size = std::mem::size_of_val(&buf) as u32;
    // SAFETY: both names are NUL-terminated HSTRING buffers alive for the call; `buf` and
    // `size` are a matched out-buffer and its byte length; RRF_RT_REG_SZ rejects other types.
    let rc = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if rc.is_err() {
        return None;
    }
    let chars = (size as usize / 2).min(buf.len());
    Some(
        String::from_utf16_lossy(&buf[..chars])
            .trim_end_matches('\0')
            .to_string(),
    )
}

fn file_log_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| cfg!(debug_assertions) || knob("RLVD_DEBUG_LOG").is_some())
}

/// The bring-up file, opened once. WUDFHost's own temp folder, never a public one a
/// non-admin could pre-create.
fn log_file() -> Option<&'static Mutex<File>> {
    static FILE: OnceLock<Option<Mutex<File>>> = OnceLock::new();
    FILE.get_or_init(|| {
        File::options()
            .create(true)
            .append(true)
            .open(std::env::temp_dir().join("rlvd-driver.log"))
            .ok()
            .map(Mutex::new)
    })
    .as_ref()
}

/// `HH:MM:SS.mmm` UTC, so a driver line lines up with the host's UTC log.
fn utc_hms_millis() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs() % 86_400;
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60,
        now.subsec_millis()
    )
}

/// One line at `INFO`.
pub fn log(s: &str) {
    log_at(control::LOG_INFO, s);
}

/// Queue one line for the host and, when enabled, tee it to the debugger and the file.
pub(crate) fn log_at(level: u8, s: &str) {
    push(level, s);
    if !file_log_enabled() {
        return;
    }
    if let Ok(c) = std::ffi::CString::new(s) {
        // SAFETY: `c` is a valid NUL-terminated string for the duration of the call.
        unsafe { wdk_sys::windows::OutputDebugStringA(c.as_ptr().cast()) };
    }
    if let Some(m) = log_file()
        && let Ok(mut f) = m.lock()
    {
        use std::io::Write;
        let _ = writeln!(f, "{} {s}", utc_hms_millis());
        let _ = f.flush();
    }
}

// Always formats: the ring is the host's only view of this process, and it bounds what a
// failure loop can hold.
macro_rules! dbglog {
    ($($a:tt)*) => { $crate::log::log(&::std::format!($($a)*)) };
}
