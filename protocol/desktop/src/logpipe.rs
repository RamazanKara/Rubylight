//! Diverts the process's stderr into `logfile.rs`.
//!
//! Release builds on Windows have no console, so the `eprintln!("nightjar: ...")` diagnostics (and
//! whatever native libraries write to stderr) would be lost. [`start`] makes a pipe the process's
//! stderr; a logger thread reads it, passes the bytes unchanged to the real stderr if there is one,
//! and writes timestamped lines to the log file. Callers keep using `eprintln!`: a write goes into the
//! pipe, as it would have gone to the console, and takes no lock and does no file work on the
//! calling thread.
//!
//! [`shutdown`] puts the real stderr back, which closes the pipe's write end, and waits (up to
//! a second) for the logger thread to drain it and flush the file. It runs on every exit path: the
//! end of `main`, `process::exit`, and the panic hook (release builds abort on panic).
//!
//! A failure to set this up is printed as one line and the client runs on without the file.

use std::sync::mpsc::{channel, Receiver};
use std::sync::Mutex;
use std::thread::JoinHandle;
use std::time::Duration;

use crate::logfile::{self, LogFile};

/// How long [`shutdown`] waits for the logger thread.
const DRAIN: Duration = Duration::from_millis(1000);

struct Active {
    redirect: sys::Redirect,
    thread: JoinHandle<()>,
    done: Receiver<()>,
}

static ACTIVE: Mutex<Option<Active>> = Mutex::new(None);

/// Start logging to the file. Call once, before anything writes to stderr that matters.
pub fn start() {
    let log = match logfile::logs_dir().and_then(|dir| LogFile::open(&dir, logfile::CAP)) {
        Ok(log) => log,
        Err(e) => return eprintln!("nightjar: no log file: {e}"),
    };
    let (redirect, reader, forward) = match sys::install() {
        Ok(parts) => parts,
        Err(e) => return eprintln!("nightjar: no log file: could not redirect stderr: {e}"),
    };
    let (finished, done) = channel();
    let thread = std::thread::Builder::new().name("log".into()).spawn(move || {
        logfile::pump(|buffer| reader.read(buffer), |bytes| forward.write(bytes), log);
        let _ = finished.send(());
    });
    match thread {
        Ok(thread) => {
            let mut active = ACTIVE.lock().unwrap_or_else(|e| e.into_inner());
            *active = Some(Active { redirect, thread, done });
        }
        Err(e) => {
            // The reader and writer closures went with the failed spawn; undo the redirect.
            redirect.restore();
            return eprintln!("nightjar: no log file: could not start the logger thread: {e}");
        }
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        previous(info);
        shutdown();
    }));
    eprintln!("nightjar: {} {} log started, pid {}", env!("CARGO_PKG_VERSION"), std::env::consts::OS, std::process::id());
}

/// Put the real stderr back and flush the log. Safe to call again and from any thread.
pub fn shutdown() {
    let active = match ACTIVE.try_lock() {
        Ok(mut guard) => guard.take(),
        Err(std::sync::TryLockError::Poisoned(e)) => e.into_inner().take(),
        // Another thread is already doing it.
        Err(std::sync::TryLockError::WouldBlock) => None,
    };
    let Some(active) = active else { return };
    active.redirect.restore();
    if active.done.recv_timeout(DRAIN).is_ok() {
        let _ = active.thread.join();
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
    /// The real stderr, as the logger thread forwards to it (-1: none).
    pub struct Forward(libc::c_int);

    pub fn install() -> io::Result<(Redirect, Reader, Forward)> {
        let mut fds = [0 as libc::c_int; 2];
        // SAFETY: plain syscalls on descriptors made here.
        unsafe {
            if libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) != 0 {
                return Err(io::Error::last_os_error());
            }
            let [read, write] = fds;
            // -1 when stderr was closed. Above fd 2 so it can never be the pipe's own.
            let saved = libc::fcntl(2, libc::F_DUPFD_CLOEXEC, 3);
            let forward = if saved >= 0 { libc::fcntl(saved, libc::F_DUPFD_CLOEXEC, 3) } else { -1 };
            if libc::dup2(write, 2) < 0 {
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
        /// Put the saved stderr back on fd 2; that drops the pipe's last write end.
        pub fn restore(self) {
            // SAFETY: descriptors made in `install`.
            unsafe {
                if self.saved >= 0 {
                    libc::dup2(self.saved, 2);
                    libc::close(self.saved);
                } else {
                    // Stderr was closed when we started: close it again.
                    libc::close(2);
                }
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
        pub fn write(&self, mut bytes: &[u8]) {
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
}

#[cfg(windows)]
mod sys {
    use std::io;

    use windows::Win32::Foundation::{CloseHandle, DuplicateHandle, DUPLICATE_SAME_ACCESS, HANDLE, INVALID_HANDLE_VALUE};
    use windows::Win32::Storage::FileSystem::{ReadFile, WriteFile};
    use windows::Win32::System::Console::{GetStdHandle, SetStdHandle, STD_ERROR_HANDLE};
    use windows::Win32::System::Pipes::CreatePipe;
    use windows::Win32::System::Threading::GetCurrentProcess;

    // The C runtime's own stderr (fd 2), which SDL, FFmpeg and other native code write through.
    // `_dup2` onto fd 0-2 also updates the process's standard handle.
    extern "C" {
        fn _open_osfhandle(handle: isize, flags: i32) -> i32;
        fn _dup2(from: i32, to: i32) -> i32;
        fn _close(fd: i32) -> i32;
    }
    const O_BINARY: i32 = 0x8000;
    const O_NOINHERIT: i32 = 0x80;

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
        /// The pipe's write end that is the standard handle.
        pipe: Raw,
    }

    pub struct Reader(Raw);
    pub struct Forward(Option<Raw>);

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
            let fd = _open_osfhandle(copy.0 as isize, O_BINARY | O_NOINHERIT);
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
            Ok((Redirect { saved, pipe: Raw(write) }, Reader(Raw(read)), Forward(saved)))
        }
    }

    impl Redirect {
        /// Put the saved stderr back (for the C runtime and for Rust) and close the pipe's write
        /// ends, which ends the logger thread's read.
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
                let _ = CloseHandle(self.pipe.0);
            }
            // `saved` stays open: the logger thread may still be writing to it. It ends with the
            // process.
        }
    }

    impl Reader {
        pub fn read(&self, buffer: &mut [u8]) -> usize {
            let mut count = 0u32;
            // SAFETY: the buffer is valid for its length.
            let ok = unsafe { ReadFile(self.0 .0, Some(buffer), Some(&mut count), None) };
            if ok.is_err() { 0 } else { count as usize }
        }
    }

    impl Drop for Reader {
        fn drop(&mut self) {
            // SAFETY: closing our own handle.
            let _ = unsafe { CloseHandle(self.0 .0) };
        }
    }

    impl Forward {
        pub fn write(&self, mut bytes: &[u8]) {
            let Some(handle) = self.0 else { return };
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
}
