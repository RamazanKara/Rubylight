//! Diverts the process's stderr into `logfile.rs`.
//!
//! Release builds on Windows have no console, so the `eprintln!("nightjar: ...")` diagnostics (and
//! whatever native libraries write to stderr) would be lost. [`start`] makes a pipe the process's
//! stderr. Two threads serve it:
//!
//! - "log" reads the pipe and passes the bytes unchanged to the real stderr if there is one
//!   (a console gets them as UTF-16), then hands a copy to the writer with `try_send`. It never
//!   waits for the disk.
//! - "log-file" stamps the lines and writes them to the file.
//!
//! Callers keep using `eprintln!`: a write goes into the pipe, as it would have gone to the
//! console, and takes no lock and does no file work on the calling thread.
//!
//! [`shutdown`] puts the real stderr back and waits (up to a second) for both threads to drain
//! what is in the pipe and flush the file. It runs on every exit path: the end of `main`,
//! `process::exit`, and the panic hook (release builds abort on panic).
//!
//! A failure to set this up is printed as one line and the client runs on without the file.

use std::io::Write;
use std::panic::PanicHookInfo;
use std::sync::mpsc::{channel, sync_channel, Receiver};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::logfile::{self, Backlog, Chunk, LogFile};

/// How long [`shutdown`] waits for the logger threads.
const DRAIN: Duration = Duration::from_millis(1000);
/// Chunks that may wait in the channel; what is held back beyond that is bounded in bytes
/// ([`logfile::BACKLOG_BYTES`]).
const CHANNEL: usize = 256;

/// The names of the logger threads: a panic on one of them must not wait for the other.
const READER: &str = "log";
const WRITER: &str = "log-file";

struct Active {
    redirect: sys::Redirect,
    reader: JoinHandle<()>,
    writer: JoinHandle<()>,
    done: Receiver<()>,
}

static ACTIVE: Mutex<Option<Active>> = Mutex::new(None);

/// A line of the logger's own to the user's stderr (and so into the log); never panics.
fn say(text: &str) {
    let _ = writeln!(std::io::stderr(), "nightjar: {text}");
}

/// Start logging to the file. Call once, before anything writes to stderr that matters.
pub fn start() {
    // First: with fd 2 closed (a service, a launcher) the pipe or the file would take its number.
    sys::prepare();
    let log = match logfile::logs_dir().and_then(|dir| LogFile::open(&dir, logfile::CAP)) {
        Ok(log) => log,
        Err(e) => return say(&format!("no log file: {e}")),
    };
    let (redirect, reader, mut forward) = match sys::install() {
        Ok(parts) => parts,
        Err(e) => return say(&format!("no log file: could not redirect stderr: {e}")),
    };
    let (files, chunks) = sync_channel::<Chunk>(CHANNEL);
    let (finished, done) = channel();
    let backlog = Arc::new(Backlog::new(logfile::BACKLOG_BYTES));
    let writer = std::thread::Builder::new().name(WRITER.into()).spawn({
        let backlog = backlog.clone();
        move || {
            logfile::pump_writer(chunks, &backlog, log, say);
            let _ = finished.send(());
        }
    });
    let writer = match writer {
        Ok(writer) => writer,
        Err(e) => {
            redirect.restore();
            return say(&format!("no log file: could not start the file thread: {e}"));
        }
    };
    let reader = std::thread::Builder::new().name(READER.into()).spawn(move || {
        logfile::pump_reader(|buffer| reader.read(buffer), |bytes| forward.write(bytes), files, &backlog);
        // Still registered: nobody asked for this, the pipe failed. Give stderr back before this
        // thread goes, or the next `eprintln!` would fail on a pipe nobody reads and panic.
        if let Some(active) = take_active() {
            active.redirect.restore();
            say("log pipe failed, logging to the file has stopped");
        }
    });
    let reader = match reader {
        Ok(reader) => reader,
        Err(e) => {
            // The closure, and with it the channel's sender, went with the failed spawn: the
            // writer ends by itself.
            redirect.restore();
            return say(&format!("no log file: could not start the logger thread: {e}"));
        }
    };
    *ACTIVE.lock().unwrap_or_else(|e| e.into_inner()) = Some(Active { redirect, reader, writer, done });
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| panic_hook(&*previous, info)));
    say(&format!("{} {} log started, pid {}", env!("CARGO_PKG_VERSION"), std::env::consts::OS, std::process::id()));
}

fn panic_hook(previous: &(dyn Fn(&PanicHookInfo<'_>) + Sync + Send), info: &PanicHookInfo<'_>) {
    let thread = std::thread::current();
    if matches!(thread.name(), Some(READER | WRITER)) {
        // The logger itself failed: put stderr back first so the message reaches the user, and
        // do not wait for threads, one of which is this one.
        if let Some(active) = take_active() {
            active.redirect.restore();
        }
        previous(info);
    } else {
        previous(info);
        shutdown();
    }
}

fn take_active() -> Option<Active> {
    match ACTIVE.try_lock() {
        Ok(mut guard) => guard.take(),
        Err(std::sync::TryLockError::Poisoned(e)) => e.into_inner().take(),
        // Another thread is already doing it.
        Err(std::sync::TryLockError::WouldBlock) => None,
    }
}

/// Put the real stderr back and flush the log. Safe to call again and from any thread.
pub fn shutdown() {
    let Some(active) = take_active() else { return };
    active.redirect.restore();
    // Finished, or the writer is gone (it panicked); otherwise give up after DRAIN.
    if active.done.recv_timeout(DRAIN).is_ok() {
        let _ = active.reader.join();
        let _ = active.writer.join();
    }
}

/// Calls [`shutdown`] when `main` returns.
pub struct Guard;

impl Drop for Guard {
    fn drop(&mut self) {
        shutdown();
    }
}

#[cfg(unix)]
mod sys {
    use std::io;

    /// What `restore` needs: the original stderr, saved.
    pub struct Redirect {
        saved: libc::c_int,
    }

    /// The pipe's read end, owned by the logger thread.
    pub struct Reader(libc::c_int);
    /// The real stderr, as the logger thread forwards to it.
    pub struct Forward(libc::c_int);

    /// Make sure `fd` is open: if it is closed, `/dev/null` takes it, so nothing opened later can.
    /// Returns whether it had to open it.
    pub fn ensure_open(fd: libc::c_int) -> io::Result<bool> {
        // SAFETY: plain syscalls on descriptors checked or made here.
        unsafe {
            if libc::fcntl(fd, libc::F_GETFD) >= 0 {
                return Ok(false);
            }
            let null = libc::open(c"/dev/null".as_ptr(), libc::O_RDWR);
            if null < 0 {
                return Err(io::Error::last_os_error());
            }
            if null != fd {
                let moved = libc::dup2(null, fd);
                let error = io::Error::last_os_error();
                libc::close(null);
                if moved < 0 {
                    return Err(error);
                }
            }
            Ok(true)
        }
    }

    /// Start with fds 0, 1 and 2 open. Without them a descriptor made later (the log file, the
    /// pipe) could get number 0, 1 or 2 and be read, written or closed as a standard stream, by
    /// us or by anyone else.
    pub fn prepare() {
        for fd in 0..3 {
            let _ = ensure_open(fd);
        }
    }

    /// A close-on-exec copy of `fd` numbered 3 or more, which is closed. Stdin, stdout and stderr
    /// stay free for whatever the process makes of them.
    fn move_up(fd: libc::c_int) -> io::Result<libc::c_int> {
        if fd >= 3 {
            return Ok(fd);
        }
        // SAFETY: duplicating and closing a descriptor made by the caller.
        unsafe {
            let high = libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3);
            let error = io::Error::last_os_error();
            libc::close(fd);
            if high < 0 { Err(error) } else { Ok(high) }
        }
    }

    pub fn install() -> io::Result<(Redirect, Reader, Forward)> {
        let mut fds = [0 as libc::c_int; 2];
        // SAFETY: plain syscalls on descriptors made here.
        unsafe {
            if libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) != 0 {
                return Err(io::Error::last_os_error());
            }
            let [read, write] = fds;
            let read = move_up(read);
            let write = move_up(write);
            let (read, write) = match (read, write) {
                (Ok(read), Ok(write)) => (read, write),
                (read, write) => {
                    for fd in [read, write].into_iter().flatten() {
                        libc::close(fd);
                    }
                    return Err(io::Error::last_os_error());
                }
            };
            // Above fd 2 so they can never be the pipe's own.
            let saved = libc::fcntl(2, libc::F_DUPFD_CLOEXEC, 3);
            let forward = if saved >= 0 { libc::fcntl(saved, libc::F_DUPFD_CLOEXEC, 3) } else { -1 };
            if saved < 0 || forward < 0 || libc::dup2(write, 2) < 0 {
                let error = io::Error::last_os_error();
                for fd in [read, write, saved, forward] {
                    if fd >= 0 {
                        libc::close(fd);
                    }
                }
                return Err(error);
            }
            libc::close(write);
            Ok((Redirect { saved }, Reader(read), Forward(forward)))
        }
    }

    impl Redirect {
        /// Put the saved stderr back on fd 2; that drops the pipe's last write end, which ends
        /// the logger thread's read.
        pub fn restore(self) {
            // SAFETY: descriptors made in `install`.
            unsafe {
                libc::dup2(self.saved, 2);
                libc::close(self.saved);
            }
        }
    }

    impl Reader {
        /// Bytes read, 0 at the end of the pipe or on an error.
        pub fn read(&self, buffer: &mut [u8]) -> usize {
            loop {
                // SAFETY: the buffer is valid for its length.
                let n = unsafe { libc::read(self.0, buffer.as_mut_ptr().cast(), buffer.len()) };
                if n >= 0 {
                    return n as usize;
                }
                if io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
                    return 0;
                }
            }
        }
    }

    impl Drop for Reader {
        fn drop(&mut self) {
            // SAFETY: closing our own descriptor.
            unsafe { libc::close(self.0) };
        }
    }

    impl Forward {
        pub fn write(&mut self, mut bytes: &[u8]) {
            while self.0 >= 0 && !bytes.is_empty() {
                // SAFETY: the slice is valid for its length.
                let n = unsafe { libc::write(self.0, bytes.as_ptr().cast(), bytes.len()) };
                if n < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                if n <= 0 {
                    return;
                }
                bytes = &bytes[n as usize..];
            }
        }
    }

    impl Drop for Forward {
        fn drop(&mut self) {
            if self.0 >= 0 {
                // SAFETY: closing our own descriptor.
                unsafe { libc::close(self.0) };
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn a_closed_descriptor_is_taken_by_dev_null_and_an_open_one_is_left() {
            // A number far above what a test process uses, so no other test thread gets it.
            let fd = 500;
            // SAFETY: probing and closing a descriptor this test made.
            unsafe {
                if libc::fcntl(fd, libc::F_GETFD) >= 0 {
                    return;
                }
                assert!(ensure_open(fd).unwrap());
                assert!(libc::fcntl(fd, libc::F_GETFD) >= 0);
                assert!(!ensure_open(fd).unwrap());
                // It is /dev/null: writing succeeds and goes nowhere.
                assert_eq!(libc::write(fd, b"x".as_ptr().cast(), 1), 1);
                libc::close(fd);
            }
        }

        /// Run `body` in a forked child (the standard descriptors cannot be closed in the test
        /// process) and return its exit code; 0 is success. The child only uses plain syscalls
        /// and exits without unwinding.
        fn in_child(body: fn() -> i32) -> i32 {
            // SAFETY: the child runs `body` and `_exit`s; the parent waits for it.
            unsafe {
                let pid = libc::fork();
                assert!(pid >= 0);
                if pid == 0 {
                    libc::_exit(body());
                }
                let mut status = 0;
                assert_eq!(libc::waitpid(pid, &mut status, 0), pid);
                if libc::WIFEXITED(status) { libc::WEXITSTATUS(status) } else { 100 + libc::WTERMSIG(status) }
            }
        }

        /// `install` the way `start` does, then write to stderr and read it back from the pipe.
        fn install_and_loop_back() -> i32 {
            let Ok((redirect, reader, forward)) = install() else { return 1 };
            // Both ends of the pipe and the saved stderr must stay clear of 0, 1 and 2.
            if reader.0 < 3 || forward.0 < 3 {
                return 2;
            }
            // SAFETY: fd 2 is the pipe's write end now.
            if unsafe { libc::write(2, b"ok".as_ptr().cast(), 2) } != 2 {
                return 3;
            }
            let mut buffer = [0u8; 2];
            if reader.read(&mut buffer) != 2 || &buffer != b"ok" {
                return 4;
            }
            redirect.restore();
            0
        }

        #[test]
        fn pipe_ends_are_moved_off_the_standard_descriptors() {
            // A launcher that closed stdin and stdout: pipe2 hands out 0 and 1 first, and only
            // `move_up` keeps the pipe off them (no `prepare` here).
            assert_eq!(
                in_child(|| {
                    // SAFETY: closing this child's own descriptors.
                    unsafe {
                        libc::close(0);
                        libc::close(1);
                    }
                    install_and_loop_back()
                }),
                0
            );
        }

        #[test]
        fn prepare_opens_whichever_standard_descriptors_are_closed() {
            assert_eq!(
                in_child(|| {
                    // SAFETY: closing and probing this child's own descriptors.
                    unsafe {
                        libc::close(0);
                        libc::close(1);
                        libc::close(2);
                        prepare();
                        for fd in 0..3 {
                            if libc::fcntl(fd, libc::F_GETFD) < 0 {
                                return 10 + fd;
                            }
                        }
                        // What start() opens next, the log file, lands above them.
                        let file = libc::open(c"/dev/null".as_ptr(), libc::O_RDONLY);
                        if file < 3 {
                            return 20;
                        }
                    }
                    install_and_loop_back()
                }),
                0
            );
        }
    }
}

#[cfg(windows)]
mod sys {
    use std::io;
    use std::sync::atomic::{AtomicBool, Ordering};

    use windows::Win32::Foundation::{CloseHandle, DuplicateHandle, DUPLICATE_SAME_ACCESS, HANDLE, INVALID_HANDLE_VALUE};
    use windows::Win32::Storage::FileSystem::{ReadFile, WriteFile};
    use windows::Win32::System::Console::{GetConsoleMode, GetStdHandle, SetStdHandle, WriteConsoleW, CONSOLE_MODE, STD_ERROR_HANDLE};
    use windows::Win32::System::Pipes::{CreatePipe, PeekNamedPipe};
    use windows::Win32::System::Threading::GetCurrentProcess;

    use crate::logfile::Utf8Carry;

    // The C runtime's own stderr (fd 2), which SDL, FFmpeg and other native code write through.
    // `_dup2` onto fd 0-2 also updates the process's standard handle.
    extern "C" {
        fn _open_osfhandle(handle: isize, flags: i32) -> i32;
        fn _dup2(from: i32, to: i32) -> i32;
        fn _close(fd: i32) -> i32;
    }
    // Text mode, like the descriptor the runtime starts with: `\n` goes out as `\r\n`.
    const O_TEXT: i32 = 0x4000;
    const O_NOINHERIT: i32 = 0x80;

    /// Set by `restore`: the pipe's write end is never closed (see there), so the reader ends
    /// when it has read what is in the pipe.
    static STOP: AtomicBool = AtomicBool::new(false);
    /// `restore` wrote the one wake-up byte ([`SENTINEL`]) that the reader must not log.
    static SENTINEL_SENT: AtomicBool = AtomicBool::new(false);
    /// Written by `restore` to wake a reader that is waiting in `ReadFile` on an empty pipe.
    const SENTINEL: u8 = 0;

    /// A handle that may move to the logger thread; the code below uses each one from one place.
    #[derive(Clone, Copy)]
    struct Raw(HANDLE);
    // SAFETY: a kernel handle is just a number; the owners below serialise their use.
    unsafe impl Send for Raw {}
    unsafe impl Sync for Raw {}

    pub struct Redirect {
        /// A duplicate of the original stderr handle, if there was one (a GUI-subsystem process
        /// started from Explorer has none).
        saved: Option<Raw>,
        /// The pipe's write end, kept open for the life of the process (see `restore`).
        pipe: Raw,
    }

    /// The pipe's read end. Never closed either: a write that was already under way when the
    /// reader left would fail with a broken pipe, and `eprintln!` panics on that.
    pub struct Reader(Raw);

    /// The real stderr, as the logger thread forwards to it.
    pub struct Forward {
        handle: Option<Raw>,
        /// A console wants UTF-16 (`WriteFile` would show UTF-8 as mojibake); a file or pipe wants
        /// the bytes as they are.
        console: bool,
        carry: Utf8Carry,
    }

    pub fn prepare() {}

    fn duplicate(handle: HANDLE) -> Option<HANDLE> {
        let mut copy = HANDLE::default();
        // SAFETY: duplicating a handle of this process into this process.
        unsafe { DuplicateHandle(GetCurrentProcess(), handle, GetCurrentProcess(), &mut copy, 0, false, DUPLICATE_SAME_ACCESS) }.ok()?;
        Some(copy)
    }

    /// Make the C runtime's fd 2 write to `handle` (a copy is used; `handle` stays ours).
    /// Returns false if the runtime refused.
    fn set_crt_stderr(handle: HANDLE) -> bool {
        let Some(copy) = duplicate(handle) else { return false };
        // SAFETY: on success the runtime owns `copy` through the descriptor.
        unsafe {
            let fd = _open_osfhandle(copy.0 as isize, O_TEXT | O_NOINHERIT);
            if fd < 0 {
                let _ = CloseHandle(copy);
                return false;
            }
            let moved = _dup2(fd, 2) == 0;
            _close(fd);
            moved
        }
    }

    pub fn install() -> io::Result<(Redirect, Reader, Forward)> {
        STOP.store(false, Ordering::SeqCst);
        SENTINEL_SENT.store(false, Ordering::SeqCst);
        let (mut read, mut write) = (HANDLE::default(), HANDLE::default());
        // SAFETY: handles made and used here.
        unsafe {
            CreatePipe(&mut read, &mut write, None, 64 * 1024).map_err(io::Error::other)?;
            let original = GetStdHandle(STD_ERROR_HANDLE).ok().filter(|h| !h.is_invalid() && *h != INVALID_HANDLE_VALUE && !h.0.is_null());
            // Our own copy: the C runtime closes the handle it had on fd 2 when it is redirected.
            let saved = original.and_then(duplicate).map(Raw);
            if let Err(e) = SetStdHandle(STD_ERROR_HANDLE, write) {
                let _ = CloseHandle(read);
                let _ = CloseHandle(write);
                if let Some(saved) = saved {
                    let _ = CloseHandle(saved.0);
                }
                return Err(io::Error::other(e));
            }
            set_crt_stderr(write);
            // The C runtime changed the standard handle to its copy of the pipe; keep ours.
            let _ = SetStdHandle(STD_ERROR_HANDLE, write);
            let console = saved.is_some_and(|saved| {
                let mut mode = CONSOLE_MODE(0);
                GetConsoleMode(saved.0, &mut mode).is_ok()
            });
            Ok((Redirect { saved, pipe: Raw(write) }, Reader(Raw(read)), Forward { handle: saved, console, carry: Utf8Carry::default() }))
        }
    }

    impl Redirect {
        /// Put the saved stderr back (for the C runtime and for Rust) and tell the reader to
        /// finish. The pipe's write handle is deliberately not closed: another thread may have
        /// fetched the number a moment ago, and closing it would let the system give the same
        /// number to something else, which that thread would then write log text into. One
        /// handle until the process ends; the reader stops by `STOP` instead of end-of-pipe.
        pub fn restore(self) {
            // SAFETY: handles and descriptors made in `install`.
            unsafe {
                match self.saved {
                    Some(saved) => {
                        if !set_crt_stderr(saved.0) {
                            _close(2);
                        }
                        let _ = SetStdHandle(STD_ERROR_HANDLE, saved.0);
                    }
                    None => {
                        _close(2);
                        let _ = SetStdHandle(STD_ERROR_HANDLE, HANDLE::default());
                    }
                }
            }
            STOP.store(true, Ordering::SeqCst);
            // A reader waiting on an empty pipe needs a byte to wake it. It can only block if the
            // pipe is full, and then the reader is not waiting, so that never holds up the
            // caller: it is written from a thread of its own, which is left behind if it does.
            SENTINEL_SENT.store(true, Ordering::SeqCst);
            let pipe = self.pipe;
            let _ = std::thread::Builder::new().name("log-wake".into()).spawn(move || {
                // The whole `Raw`, which is `Send`, not just its handle field.
                let pipe = pipe;
                let mut count = 0u32;
                // SAFETY: the handle is never closed.
                if unsafe { WriteFile(pipe.0, Some(&[SENTINEL]), Some(&mut count), None) }.is_err() {
                    SENTINEL_SENT.store(false, Ordering::SeqCst);
                }
            });
            // `saved` stays open too: the logger thread may still be writing to it. It ends with
            // the process.
        }
    }

    /// `count` bytes were read: leave out the wake-up byte if it is the last of them.
    fn without_sentinel(buffer: &[u8], count: usize) -> usize {
        if count > 0 && STOP.load(Ordering::SeqCst) && buffer[count - 1] == SENTINEL && SENTINEL_SENT.swap(false, Ordering::SeqCst) {
            count - 1
        } else {
            count
        }
    }

    impl Reader {
        /// Bytes read; 0 on an error or once `STOP` is set and the pipe is empty. An empty write
        /// from native code is not the end: the write end stays open, so nothing but an error
        /// ends the read while the pipe is in use.
        pub fn read(&self, buffer: &mut [u8]) -> usize {
            loop {
                if STOP.load(Ordering::SeqCst) {
                    let count = self.read_available(buffer);
                    if count == 0 {
                        return 0;
                    }
                    let count = without_sentinel(buffer, count);
                    if count > 0 {
                        return count;
                    }
                    continue;
                }
                let mut count = 0u32;
                // SAFETY: the buffer is valid for its length.
                let ok = unsafe { ReadFile(self.0 .0, Some(buffer), Some(&mut count), None) };
                match ok {
                    Ok(()) => {
                        let count = without_sentinel(buffer, count as usize);
                        if count > 0 {
                            return count;
                        }
                    }
                    Err(_) => return 0,
                }
            }
        }

        /// What is in the pipe now, without waiting for more.
        fn read_available(&self, buffer: &mut [u8]) -> usize {
            let mut available = 0u32;
            // SAFETY: valid out pointer; the pipe handle is ours.
            if unsafe { PeekNamedPipe(self.0 .0, None, 0, None, Some(&mut available), None) }.is_err() || available == 0 {
                return 0;
            }
            let want = buffer.len().min(available as usize);
            let mut count = 0u32;
            // SAFETY: the buffer is valid for `want` bytes, and that much is waiting.
            let ok = unsafe { ReadFile(self.0 .0, Some(&mut buffer[..want]), Some(&mut count), None) };
            if ok.is_err() { 0 } else { count as usize }
        }
    }

    impl Forward {
        pub fn write(&mut self, bytes: &[u8]) {
            let Some(handle) = self.handle else { return };
            if self.console {
                let text = self.carry.push(bytes);
                let wide: Vec<u16> = text.encode_utf16().collect();
                let mut rest = &wide[..];
                while !rest.is_empty() {
                    let mut written = 0u32;
                    // SAFETY: the slice is valid for its length.
                    let ok = unsafe { WriteConsoleW(handle.0, rest, Some(&mut written), None) };
                    if ok.is_err() || written == 0 {
                        return;
                    }
                    rest = &rest[written as usize..];
                }
                return;
            }
            let mut bytes = bytes;
            while !bytes.is_empty() {
                let mut count = 0u32;
                // SAFETY: the slice is valid for its length.
                let ok = unsafe { WriteFile(handle.0, Some(bytes), Some(&mut count), None) };
                if ok.is_err() || count == 0 {
                    return;
                }
                bytes = &bytes[count as usize..];
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::sync::mpsc::channel;
        use std::sync::Mutex;
        use std::time::Duration;

        /// The tests share `STOP` and `SENTINEL_SENT`.
        static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

        fn pipe() -> (Reader, HANDLE) {
            let (mut read, mut write) = (HANDLE::default(), HANDLE::default());
            // SAFETY: handles made here; like the real ones they are never closed.
            unsafe { CreatePipe(&mut read, &mut write, None, 4096).unwrap() };
            STOP.store(false, Ordering::SeqCst);
            SENTINEL_SENT.store(false, Ordering::SeqCst);
            (Reader(Raw(read)), write)
        }

        fn put(write: HANDLE, bytes: &[u8]) {
            let mut count = 0u32;
            // SAFETY: a valid pipe handle and slice.
            unsafe { WriteFile(write, Some(bytes), Some(&mut count), None).unwrap() };
        }

        #[test]
        fn once_stopped_the_reader_drains_the_pipe_and_ends_without_waiting() {
            let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
            let (reader, write) = pipe();
            put(write, b"left in the pipe");
            // The write end stays open, as at shutdown.
            STOP.store(true, Ordering::SeqCst);
            let mut buffer = [0u8; 8];
            let mut got = Vec::new();
            loop {
                let n = reader.read(&mut buffer);
                if n == 0 {
                    break;
                }
                got.extend_from_slice(&buffer[..n]);
            }
            STOP.store(false, Ordering::SeqCst);
            assert_eq!(got, b"left in the pipe");
        }

        #[test]
        fn the_wake_up_byte_ends_a_waiting_reader_and_is_not_logged() {
            let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
            let (reader, write) = pipe();
            let (done, finished) = channel();
            std::thread::spawn(move || {
                let mut buffer = [0u8; 64];
                let mut got = Vec::new();
                loop {
                    let n = reader.read(&mut buffer);
                    if n == 0 {
                        break;
                    }
                    got.extend_from_slice(&buffer[..n]);
                }
                let _ = done.send(got);
            });
            put(write, b"line\n");
            std::thread::sleep(Duration::from_millis(100));
            // What `restore` does, without touching this process's stderr.
            STOP.store(true, Ordering::SeqCst);
            SENTINEL_SENT.store(true, Ordering::SeqCst);
            put(write, &[SENTINEL]);
            let got = finished.recv_timeout(Duration::from_secs(5)).expect("the reader did not end");
            STOP.store(false, Ordering::SeqCst);
            assert_eq!(got, b"line\n");
        }

        #[test]
        fn an_empty_write_does_not_end_the_reader() {
            let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
            let (reader, write) = pipe();
            let (done, finished) = channel();
            std::thread::spawn(move || {
                let mut buffer = [0u8; 64];
                let n = reader.read(&mut buffer);
                let _ = done.send(buffer[..n].to_vec());
            });
            std::thread::sleep(Duration::from_millis(100));
            let mut count = 0u32;
            // SAFETY: a valid pipe handle; a zero-length write, as native code sometimes makes.
            unsafe { WriteFile(write, Some(&[]), Some(&mut count), None).unwrap() };
            std::thread::sleep(Duration::from_millis(100));
            put(write, b"x");
            assert_eq!(finished.recv_timeout(Duration::from_secs(5)).expect("the reader ended"), b"x");
        }
    }
}
