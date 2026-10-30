//! Windows crash dumps, ported from Moonlight V+ PC (qiin2333/moonlight-qt, `app/main.cpp`): a
//! crash the process does not handle (an access violation in a driver, the decoder or SDL) writes
//! a minidump to the logs folder and appends what happened, the dump's path and the faulting
//! module to `nightjar.log` before Windows ends the process. The newest dumps are kept, older ones
//! removed at start. Rust panics are not crashes here: the panic hook logs them (logpipe.rs).
//!
//! The filter runs on the crashed thread, which may have no stack left (a stack overflow) and may
//! hold the heap or loader lock. So it only hands the exception to a thread started for this at
//! launch and waits, at most [`WAIT_MS`], for it to write the dump and the log lines; that thread
//! allocates nothing either, building every path and line in buffers prepared at start or on its
//! own stack. A crash on a second thread meanwhile waits for the same dump instead of ending the
//! process mid-write. A filter installed before ours still runs after it.
//!
//! The dump holds the threads' stacks, modules and thread details, not the memory they point to:
//! logs get shared, and pointed-to memory would carry pairing keys, tokens and clipboard text.

use std::fmt::Write as _;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, GENERIC_WRITE, HANDLE, HMODULE};
use windows::Win32::Storage::FileSystem::{CreateFileW, WriteFile, FlushFileBuffers, CREATE_ALWAYS, FILE_APPEND_DATA, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_WRITE_THROUGH, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_ALWAYS, FILE_SHARE_NONE};
use windows::Win32::System::Diagnostics::Debug::{MiniDumpIgnoreInaccessibleMemory, MiniDumpNormal, MiniDumpWithThreadInfo, MiniDumpWithUnloadedModules, MiniDumpWriteDump, SetUnhandledExceptionFilter, EXCEPTION_POINTERS, LPTOP_LEVEL_EXCEPTION_FILTER, MINIDUMP_EXCEPTION_INFORMATION, MINIDUMP_TYPE};
use windows::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT};
use windows::Win32::System::Threading::{CreateEventW, GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId, SetEvent, Sleep, WaitForSingleObject, INFINITE};

const PREFIX: &str = "nightjar-crash-";
/// Dumps kept, counting the one a crash in this run would write.
const KEEP: usize = 3;
const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
/// How long a crashed thread waits for the dump before Windows ends the process anyway.
const WAIT_MS: u32 = 30_000;
/// Stacks, modules and threads; no memory reached through pointers.
const KIND: MINIDUMP_TYPE = MINIDUMP_TYPE(MiniDumpNormal.0 | MiniDumpWithThreadInfo.0 | MiniDumpWithUnloadedModules.0 | MiniDumpIgnoreInaccessibleMemory.0);

struct Paths {
    /// The logs folder, without a terminating NUL.
    dir: Vec<u16>,
    /// `nightjar.log` in it, NUL-terminated.
    log: Vec<u16>,
}

static PATHS: OnceLock<Paths> = OnceLock::new();
static PREVIOUS: OnceLock<LPTOP_LEVEL_EXCEPTION_FILTER> = OnceLock::new();
static HIT: AtomicBool = AtomicBool::new(false);
/// The writer's events, as raw handle values: asked to write (auto reset), and done (manual reset,
/// so every crashed thread waiting sees it).
static REQUEST: AtomicUsize = AtomicUsize::new(0);
static DONE: AtomicUsize = AtomicUsize::new(0);
static CRASH: AtomicPtr<EXCEPTION_POINTERS> = AtomicPtr::new(std::ptr::null_mut());
static CRASH_THREAD: AtomicU32 = AtomicU32::new(0);

/// Writes dumps for unhandled crashes to `dir` (the logs folder) from now on, and says so in the
/// log. A folder that cannot be made, or no writer thread, is logged and leaves dumps off.
pub fn install(dir: &Path) {
    if let Err(e) = std::fs::create_dir_all(dir) {
        eprintln!("nightjar: crash dumps off: {}: {e}", dir.display());
        return;
    }
    for (path, error) in prune(dir, KEEP - 1) {
        eprintln!("nightjar: could not remove old crash dump {}: {error}", path.display());
    }
    let wide = |p: &Path| p.as_os_str().encode_wide().collect::<Vec<u16>>();
    if PATHS.set(Paths { dir: wide(dir), log: wide(&dir.join(crate::logfile::CURRENT)).into_iter().chain([0]).collect() }).is_err() {
        return;
    }
    // SAFETY: unnamed events, kept open for the life of the process.
    let events = unsafe { CreateEventW(None, false, false, PCWSTR::null()).and_then(|r| Ok((r, CreateEventW(None, true, false, PCWSTR::null())?))) };
    let (request, done) = match events {
        Ok(events) => events,
        Err(e) => { eprintln!("nightjar: crash dumps off: no event: {e}"); return; }
    };
    REQUEST.store(request.0 as usize, Ordering::SeqCst);
    DONE.store(done.0 as usize, Ordering::SeqCst);
    let request = request.0 as usize;
    let spawned = std::thread::Builder::new().name("crash-dump".into()).spawn(move || {
        // SAFETY: the event made above, never closed.
        unsafe { WaitForSingleObject(HANDLE(request as *mut _), INFINITE) };
        write_crash();
    });
    if let Err(e) = spawned {
        eprintln!("nightjar: crash dumps off: no writer thread: {e}");
        return;
    }
    // SAFETY: the handler is a plain function that lives as long as the process.
    let _ = PREVIOUS.set(unsafe { SetUnhandledExceptionFilter(Some(handler)) });
    eprintln!("nightjar: crash dumps go to {}", dir.display());
}

/// Removes all but the newest `keep` dumps in `dir`; returns the ones it could not remove.
fn prune(dir: &Path, keep: usize) -> Vec<(PathBuf, std::io::Error)> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut dumps: Vec<(SystemTime, PathBuf)> = entries.filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().starts_with(PREFIX) && e.path().extension().is_some_and(|x| x == "dmp"))
        .map(|e| (e.metadata().and_then(|m| m.modified()).unwrap_or(UNIX_EPOCH), e.path()))
        .collect();
    dumps.sort_by_key(|d| std::cmp::Reverse(d.0));
    dumps.into_iter().skip(keep).filter_map(|(_, path)| std::fs::remove_file(&path).err().map(|e| (path, e))).collect()
}

/// A line built without allocating; text past its end is cut.
struct Line {
    bytes: [u8; 1024],
    len: usize,
}

impl Line {
    fn new() -> Self {
        Self { bytes: [0; 1024], len: 0 }
    }

    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }

    fn wide(&mut self, text: &[u16]) {
        for c in char::decode_utf16(text.iter().copied()) {
            let _ = self.write_char(c.unwrap_or(char::REPLACEMENT_CHARACTER));
        }
    }
}

impl std::fmt::Write for Line {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        let n = s.len().min(self.bytes.len() - self.len);
        self.bytes[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}

fn code_name(code: u32) -> &'static str {
    match code {
        0xC000_0005 => "EXCEPTION_ACCESS_VIOLATION",
        0xC000_008C => "EXCEPTION_ARRAY_BOUNDS_EXCEEDED",
        0x8000_0003 => "EXCEPTION_BREAKPOINT",
        0x8000_0002 => "EXCEPTION_DATATYPE_MISALIGNMENT",
        0xC000_008E => "EXCEPTION_FLT_DIVIDE_BY_ZERO",
        0xC000_0090 => "EXCEPTION_FLT_INVALID_OPERATION",
        0xC000_001D => "EXCEPTION_ILLEGAL_INSTRUCTION",
        0xC000_0006 => "EXCEPTION_IN_PAGE_ERROR",
        0xC000_0094 => "EXCEPTION_INT_DIVIDE_BY_ZERO",
        0xC000_0096 => "EXCEPTION_PRIV_INSTRUCTION",
        0xC000_00FD => "EXCEPTION_STACK_OVERFLOW",
        0xE06D_7363 => "MSVC_CPP_EXCEPTION",
        _ => "UNKNOWN_EXCEPTION",
    }
}

/// The first log line: the exception, where, on which thread, and for an access violation what
/// the access was.
fn describe(line: &mut Line, code: u32, address: usize, info: &[usize], thread: u32) {
    let _ = write!(line, "nightjar: crash: unhandled exception 0x{code:08X} ({}) at 0x{address:X}, thread {thread}", code_name(code));
    if matches!(code, 0xC000_0005 | 0xC000_0006) && info.len() >= 2 {
        let operation = match info[0] { 0 => "read", 1 => "write", 8 => "execute", _ => "unknown" };
        let _ = write!(line, ", {operation} of 0x{:X}", info[1]);
    }
    let _ = write!(line, ", Nightjar {}\r\n", crate::VERSION);
}

/// Appends to `nightjar.log` past the log thread, which may not run again; stderr if that fails.
fn append(log: &[u16], line: &Line) {
    // SAFETY: a NUL-terminated path; the handle is closed here.
    unsafe {
        let file = CreateFileW(PCWSTR(log.as_ptr()), FILE_APPEND_DATA.0, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, None, OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL | FILE_FLAG_WRITE_THROUGH, None);
        match file {
            Ok(file) => {
                let _ = WriteFile(file, Some(line.as_bytes()), None, None);
                let _ = FlushFileBuffers(file);
                let _ = CloseHandle(file);
            }
            Err(_) => {
                let _ = std::io::Write::write_all(&mut std::io::stderr(), line.as_bytes());
            }
        }
    }
}

/// The filter, on the crashed thread: as little as can be, no stack to speak of.
unsafe extern "system" fn handler(pointers: *const EXCEPTION_POINTERS) -> i32 {
    // SAFETY: plain Win32 calls on the events made at install.
    unsafe {
        // The first unhandled exception is dumped; a crash on another thread meanwhile waits for it.
        if !HIT.swap(true, Ordering::SeqCst) {
            CRASH.store(pointers.cast_mut(), Ordering::SeqCst);
            CRASH_THREAD.store(GetCurrentThreadId(), Ordering::SeqCst);
            let _ = SetEvent(HANDLE(REQUEST.load(Ordering::SeqCst) as *mut _));
        }
        WaitForSingleObject(HANDLE(DONE.load(Ordering::SeqCst) as *mut _), WAIT_MS);
        match PREVIOUS.get().copied().flatten() {
            Some(previous) => previous(pointers),
            None => EXCEPTION_CONTINUE_SEARCH,
        }
    }
}

/// On the writer thread: the log lines and the dump of the crash the filter handed over.
fn write_crash() {
    let pointers = CRASH.load(Ordering::SeqCst).cast_const();
    let thread = CRASH_THREAD.load(Ordering::SeqCst);
    if let Some(paths) = PATHS.get() { write_dump(paths, pointers, thread); }
    // SAFETY: the event made at install, never closed.
    let _ = unsafe { SetEvent(HANDLE(DONE.load(Ordering::SeqCst) as *mut _)) };
}

fn write_dump(paths: &Paths, pointers: *const EXCEPTION_POINTERS, thread: u32) {
    // SAFETY: the crashed thread waits in the filter, so the exception it passed stays valid; each
    // pointer may be null.
    let record = unsafe { pointers.as_ref().and_then(|p| p.ExceptionRecord.as_ref()) };
    let (code, address) = record.map_or((0, 0), |r| (r.ExceptionCode.0 as u32, r.ExceptionAddress as usize));
    let info = record.map_or(&[][..], |r| &r.ExceptionInformation[..(r.NumberParameters as usize).min(15)]);
    // SAFETY: plain Win32 calls with buffers on this stack.
    unsafe {
        let mut line = Line::new();
        describe(&mut line, code, address, info, thread);
        append(&paths.log, &line);

        let mut name = [0u16; 1024];
        let mut text = Line::new();
        let seconds = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let _ = write!(text, "\\{PREFIX}{seconds}.dmp");
        let suffix: [u16; 64] = {
            let mut wide = [0u16; 64];
            for (slot, unit) in wide.iter_mut().zip(std::str::from_utf8(text.as_bytes()).unwrap_or("").encode_utf16()) { *slot = unit; }
            wide
        };
        let mut at = 0;
        for unit in paths.dir.iter().chain(suffix.iter().take_while(|u| **u != 0)) {
            if at + 1 >= name.len() { break; }
            name[at] = *unit;
            at += 1;
        }
        let path = &name[..at];

        let mut line = Line::new();
        match CreateFileW(PCWSTR(name.as_ptr()), GENERIC_WRITE.0, FILE_SHARE_NONE, None, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, None) {
            Ok(file) => {
                let exception = MINIDUMP_EXCEPTION_INFORMATION { ThreadId: thread, ExceptionPointers: pointers.cast_mut(), ClientPointers: false.into() };
                match MiniDumpWriteDump(GetCurrentProcess(), GetCurrentProcessId(), file, KIND, Some(&exception), None, None) {
                    Ok(()) => { let _ = write!(line, "nightjar: crash dump written to "); line.wide(path); }
                    Err(e) => { let _ = write!(line, "nightjar: crash dump failed: error 0x{:08X}, ", e.code().0); line.wide(path); }
                }
                let _ = CloseHandle(file);
            }
            Err(e) => { let _ = write!(line, "nightjar: crash dump not opened: error 0x{:08X}, ", e.code().0); line.wide(path); }
        }
        let _ = write!(line, "\r\n");
        append(&paths.log, &line);

        // The module after the dump: these calls may need the loader lock the crashed thread holds.
        let mut module = HMODULE::default();
        let mut file = [0u16; 260];
        let mut line = Line::new();
        if address != 0 && GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, PCWSTR(address as *const u16), &mut module).is_ok() {
            let n = GetModuleFileNameW(Some(module), &mut file) as usize;
            let full = &file[..n.min(file.len())];
            let base = full.iter().rposition(|u| *u == u16::from(b'\\')).map_or(full, |i| &full[i + 1..]);
            let _ = write!(line, "nightjar: crash: in ");
            line.wide(base);
            let _ = write!(line, " at offset 0x{:X}\r\n", address - module.0 as usize);
        } else {
            let _ = write!(line, "nightjar: crash: in an unknown module\r\n");
        }
        append(&paths.log, &line);
        // The log thread may still be writing what came before.
        Sleep(500);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_crash_line_names_the_exception_and_the_access() {
        let mut line = Line::new();
        describe(&mut line, 0xC000_0005, 0x7FF6_1234, &[1, 0x10], 42);
        let text = std::str::from_utf8(line.as_bytes()).unwrap();
        assert!(text.starts_with("nightjar: crash: unhandled exception 0xC0000005 (EXCEPTION_ACCESS_VIOLATION) at 0x7FF61234, thread 42, write of 0x10, Nightjar "), "{text}");
        assert!(text.ends_with("\r\n"));
        let mut line = Line::new();
        describe(&mut line, 0xC000_0094, 0x10, &[], 1);
        assert!(std::str::from_utf8(line.as_bytes()).unwrap().contains("(EXCEPTION_INT_DIVIDE_BY_ZERO) at 0x10, thread 1, Nightjar"));
        let mut long = Line::new();
        for _ in 0..200 { let _ = write!(long, "0123456789"); }
        assert_eq!(long.as_bytes().len(), 1024, "cut, not grown");
    }

    #[test]
    fn old_dumps_are_pruned_to_the_newest() {
        let dir = std::env::temp_dir().join(format!("nightjar-prune-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..5u64 {
            let path = dir.join(format!("{PREFIX}{i}.dmp"));
            std::fs::write(&path, b"x").unwrap();
            let file = std::fs::File::options().write(true).open(&path).unwrap();
            file.set_modified(UNIX_EPOCH + std::time::Duration::from_secs(1_000 + i)).unwrap();
        }
        std::fs::write(dir.join("nightjar.log"), b"kept").unwrap();
        assert!(prune(&dir, 2).is_empty());
        let mut left: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left, [format!("{PREFIX}3.dmp"), format!("{PREFIX}4.dmp"), "nightjar.log".to_owned()]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The exit code of the filter installed before ours in [`crash_child`].
    const CHAINED: u32 = 0x4E4A;

    unsafe extern "system" fn earlier_filter(_: *const EXCEPTION_POINTERS) -> i32 {
        // SAFETY: ends this child process.
        let _ = unsafe { windows::Win32::System::Threading::TerminateProcess(GetCurrentProcess(), CHAINED) };
        EXCEPTION_CONTINUE_SEARCH
    }

    fn overflow(depth: u64) -> u64 {
        if depth == u64::MAX { return 0; }
        let frame = std::hint::black_box([depth as u8; 4096]);
        overflow(depth + 1) + u64::from(frame[0])
    }

    /// Run by the tests below in a child process: crashes for real.
    #[test]
    fn crash_child() {
        let Some(dir) = std::env::var_os("NIGHTJAR_CRASH_CHILD_DIR") else { return };
        let overflows = std::env::var_os("NIGHTJAR_CRASH_CHILD_OVERFLOW").is_some();
        // No Windows Error Reporting dialog for the crash this test makes on purpose.
        // SAFETY: changes this child process's error mode and filter only.
        unsafe {
            windows::Win32::System::Diagnostics::Debug::SetErrorMode(windows::Win32::System::Diagnostics::Debug::SEM_NOGPFAULTERRORBOX);
            if !overflows { SetUnhandledExceptionFilter(Some(earlier_filter)); }
        }
        install(Path::new(&dir));
        if overflows {
            std::hint::black_box(overflow(0));
        }
        // SAFETY: none; an access violation is the point.
        unsafe { std::ptr::write_volatile(8 as *mut u8, 1) };
    }

    /// Runs [`crash_child`]; its exit code, the dumps it wrote and its log.
    fn crash(name: &str, overflow: bool) -> (Option<u32>, Vec<PathBuf>, String, PathBuf) {
        let dir = std::env::temp_dir().join(format!("nightjar-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut child = std::process::Command::new(std::env::current_exe().unwrap());
        child.args(["--exact", "crash_dump::tests::crash_child", "--nocapture", "--test-threads=1"])
            .env("NIGHTJAR_CRASH_CHILD_DIR", &dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        if overflow { child.env("NIGHTJAR_CRASH_CHILD_OVERFLOW", "1"); }
        let status = child.status().unwrap();
        let dumps: Vec<PathBuf> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().path()).filter(|p| p.extension().is_some_and(|x| x == "dmp")).collect();
        for dump in &dumps { assert_eq!(&std::fs::read(dump).unwrap()[..4], b"MDMP", "a minidump"); }
        let log = std::fs::read_to_string(dir.join(crate::logfile::CURRENT)).unwrap_or_default();
        (status.code().map(|c| c as u32), dumps, log, dir)
    }

    #[test]
    fn a_crash_writes_a_dump_says_where_and_runs_the_earlier_filter() {
        let (code, dumps, log, dir) = crash("crash", false);
        assert_eq!(code, Some(CHAINED), "the filter installed before ours ran after it");
        assert_eq!(dumps.len(), 1, "{dumps:?}");
        assert!(log.contains("(EXCEPTION_ACCESS_VIOLATION)") && log.contains(", write of 0x8,"), "{log}");
        assert!(log.contains(&format!("crash dump written to {}", dumps[0].display())), "{log}");
        assert!(log.contains("nightjar: crash: in "), "{log}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_stack_overflow_gets_its_dump_too() {
        let (code, dumps, log, dir) = crash("overflow", true);
        assert_eq!(code, Some(0xC000_00FD), "the child overflowed its stack");
        assert_eq!(dumps.len(), 1, "{dumps:?} {log}");
        assert!(log.contains("(EXCEPTION_STACK_OVERFLOW)") && log.contains("crash dump written to "), "{log}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_dump_leaves_out_memory_behind_pointers() {
        use windows::Win32::System::Diagnostics::Debug::{MiniDumpWithFullMemory, MiniDumpWithIndirectlyReferencedMemory, MiniDumpWithPrivateReadWriteMemory};
        for kind in [MiniDumpWithIndirectlyReferencedMemory, MiniDumpWithFullMemory, MiniDumpWithPrivateReadWriteMemory] {
            assert_eq!(KIND.0 & kind.0, 0, "{kind:?}");
        }
    }
}
