//! Hands a pairing link to the Nightjar that is already open.
//!
//! The system starts a new Nightjar for every link it opens. That one offers the link here first
//! and exits when an open window took it; with no window open it becomes the window itself. A
//! start without a link never asks, and a second window simply has no channel: more than one can
//! be open, as ever.
//!
//! The channel carries one link and a newline, answered by [`DELIVERED`]. It is local to this
//! user: a named pipe open to this user alone, refusing remote clients and with an owner the sender
//! checks, on Windows; a Unix socket in the private `XDG_RUNTIME_DIR` elsewhere. Every step has a
//! time limit, so neither side can hang the other. What arrives is only ever a link for
//! the window to ask about, and it is never written to the log.

use std::io::{self, Read, Write};
use std::sync::{Arc, Mutex};

use rubylight_protocol::pairing_uri::MAX_LEN;

use crate::hosts::Waker;

const DELIVERED: u8 = b'k';

/// The link waiting for the window: the newest one, so a page opening links in a loop cannot pile
/// up dialogs behind each other.
#[derive(Default)]
pub struct Inbox(Mutex<Option<String>>);

impl Inbox {
    pub fn put(&self, link: String) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(link);
    }

    pub fn take(&self) -> Option<String> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).take()
    }
}

/// Gives the link to the open Nightjar. False when none took it: this process opens the window.
pub fn send(link: &str) -> bool {
    os::address().is_some_and(|address| os::send_to(&address, link))
}

/// Starts taking links for this window. An error is no failure of the window: it means links
/// start their own window, because another one has the channel or there is none.
pub fn listen(inbox: Arc<Inbox>, wake: Waker) -> Result<(), String> {
    let address = os::address().ok_or("this system offers no private channel")?;
    os::listen_at(&address, inbox, wake)
}

/// What goes over the channel. A link the reader would refuse for its size or its line breaks
/// goes as an empty line, which the window refuses the same way.
fn wire(link: &str) -> Vec<u8> {
    let link = link.trim();
    let plain = link.len() <= MAX_LEN && !link.contains(['\r', '\n']);
    let mut out = Vec::with_capacity(link.len() + 1);
    if plain {
        out.extend_from_slice(link.as_bytes());
    }
    out.push(b'\n');
    out
}

/// The sender's side of one connection.
fn hand_over(conn: &mut (impl Read + Write), link: &str) -> bool {
    let mut answer = [0];
    conn.write_all(&wire(link)).is_ok() && conn.flush().is_ok() && conn.read_exact(&mut answer).is_ok() && answer[0] == DELIVERED
}

/// Reads one line, at most a link's length. None when the connection ended without one (a start
/// checking for this window); an empty text when what came cannot be a link.
fn receive(conn: &mut impl Read) -> Option<String> {
    let mut line = Vec::new();
    let mut chunk = [0; 1024];
    loop {
        let count = match conn.read(&mut chunk) {
            Ok(0) | Err(_) => return None,
            Ok(count) => count,
        };
        if let Some(end) = chunk[..count].iter().position(|&b| b == b'\n') {
            line.extend_from_slice(&chunk[..end]);
            let fits = line.len() <= MAX_LEN;
            return Some(String::from_utf8(line).ok().filter(|_| fits).unwrap_or_default());
        }
        line.extend_from_slice(&chunk[..count]);
        if line.len() > MAX_LEN {
            return Some(String::new());
        }
    }
}

/// The window's side of one connection.
fn serve(conn: &mut (impl Read + Write), inbox: &Inbox, wake: &Waker) {
    if let Some(link) = receive(conn) {
        inbox.put(link);
        wake();
        // A sender that is gone already opened its own window; nothing to tell it.
        let _ = conn.write_all(&[DELIVERED]).and_then(|()| conn.flush());
    }
}

/// `$XDG_RUNTIME_DIR/nightjar-link`. A Flatpak's own one is new for every start: it shares only
/// `app/<id>` in there between them.
#[cfg(any(unix, test))]
fn socket_path(runtime_dir: Option<std::ffi::OsString>, flatpak_id: Option<std::ffi::OsString>) -> Option<std::path::PathBuf> {
    let mut path = std::path::PathBuf::from(runtime_dir.filter(|dir| !dir.is_empty())?);
    if let Some(id) = flatpak_id.filter(|id| !id.is_empty()) {
        path.push("app");
        path.push(id);
    }
    path.push("nightjar-link");
    Some(path)
}

fn thread(run: impl FnOnce() + Send + 'static) -> Result<(), String> {
    std::thread::Builder::new().name("link-channel".into()).spawn(run).map(|_| ()).map_err(|e| e.to_string())
}

#[cfg(unix)]
mod os {
    use super::*;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    /// A sender that does not finish its one line in this time is dropped.
    const PATIENCE: Duration = Duration::from_secs(2);

    pub fn address() -> Option<PathBuf> {
        socket_path(std::env::var_os("XDG_RUNTIME_DIR"), std::env::var_os("FLATPAK_ID"))
    }

    pub fn send_to(path: &Path, link: &str) -> bool {
        let Ok(mut conn) = UnixStream::connect(path) else { return false };
        conn.set_read_timeout(Some(PATIENCE)).is_ok() && conn.set_write_timeout(Some(PATIENCE)).is_ok() && hand_over(&mut conn, link)
    }

    pub fn listen_at(path: &Path, inbox: Arc<Inbox>, wake: Waker) -> Result<(), String> {
        let listener = match UnixListener::bind(path) {
            Ok(listener) => listener,
            Err(e) if e.kind() == io::ErrorKind::AddrInUse => {
                if UnixStream::connect(path).is_ok() {
                    return Err("another window has the channel".into());
                }
                // Left by a Nightjar that ended.
                std::fs::remove_file(path).map_err(|e| e.to_string())?;
                UnixListener::bind(path).map_err(|e| e.to_string())?
            }
            Err(e) => return Err(e.to_string()),
        };
        thread(move || loop {
            match listener.accept() {
                Ok((mut conn, _)) => {
                    if conn.set_read_timeout(Some(PATIENCE)).is_ok() && conn.set_write_timeout(Some(PATIENCE)).is_ok() {
                        serve(&mut conn, &inbox, &wake);
                    }
                }
                Err(e) => {
                    eprintln!("nightjar: pairing links cannot reach this window: {e}");
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
        })
    }
}

#[cfg(windows)]
mod os {
    use super::*;
    use std::time::Duration;
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Foundation::{CloseHandle, LocalFree, ERROR_IO_PENDING, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, GENERIC_READ, GENERIC_WRITE, HANDLE, HLOCAL, WAIT_OBJECT_0};
    use windows::Win32::Security::Authorization::{ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo, SDDL_REVISION_1, SE_KERNEL_OBJECT};
    use windows::Win32::Security::{EqualSid, GetTokenInformation, TokenUser, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER};
    use windows::Win32::Storage::FileSystem::{CreateFileW, ReadFile, WriteFile, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_NONE, OPEN_EXISTING, PIPE_ACCESS_DUPLEX, SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT};
    use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
    use windows::Win32::System::Pipes::{ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, WaitNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT};
    use windows::Win32::System::Threading::{CreateEventW, GetCurrentProcess, OpenProcessToken, WaitForSingleObject, INFINITE};

    /// How long a busy pipe (the window is reading another start's link) is waited for.
    const BUSY_PATIENCE: Duration = Duration::from_millis(500);
    /// How long one read or write may take: a suspended window cannot hang a new start, which
    /// then opens its own window, and a stuck start cannot hold the window's channel.
    const PATIENCE: Duration = Duration::from_secs(2);

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain([0]).collect()
    }

    /// This process's user. In u64s for the alignment of the `TOKEN_USER` inside.
    struct User(Vec<u64>);

    impl User {
        fn current() -> Option<Self> {
            // SAFETY: the token handle is this function's and closed in it; the buffer has the
            // size the first call asks for.
            unsafe {
                let mut token = HANDLE::default();
                OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).ok()?;
                let mut size = 0;
                let _ = GetTokenInformation(token, TokenUser, None, 0, &mut size);
                let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
                let read = GetTokenInformation(token, TokenUser, Some(buffer.as_mut_ptr().cast()), size, &mut size);
                let _ = CloseHandle(token);
                read.ok()?;
                Some(Self(buffer))
            }
        }

        fn sid(&self) -> PSID {
            // SAFETY: the buffer holds a TOKEN_USER, written by GetTokenInformation.
            unsafe { (*self.0.as_ptr().cast::<TOKEN_USER>()).User.Sid }
        }

        fn text(&self) -> Option<String> {
            let mut text = PWSTR::null();
            // SAFETY: a valid SID; the string is copied and freed here as the call asks.
            unsafe {
                ConvertSidToStringSidW(self.sid(), &mut text).ok()?;
                let copy = text.to_string().ok();
                let _ = LocalFree(Some(HLOCAL(text.0.cast())));
                copy
            }
        }

        /// Whether this user created the pipe. Pipe names are shared between users: another one
        /// could have made ours first to read the links.
        fn owns(&self, pipe: HANDLE) -> bool {
            let mut owner = PSID::default();
            let mut descriptor = PSECURITY_DESCRIPTOR::default();
            // SAFETY: a handle opened with GENERIC_READ, which has READ_CONTROL; the owner points
            // into the descriptor, freed after the comparison.
            unsafe {
                if GetSecurityInfo(pipe, SE_KERNEL_OBJECT, OWNER_SECURITY_INFORMATION, Some(&mut owner), None, None, None, Some(&mut descriptor)).is_err() {
                    return false;
                }
                let same = EqualSid(owner, self.sid()).is_ok();
                let _ = LocalFree(Some(HLOCAL(descriptor.0)));
                same
            }
        }
    }

    /// Each user has their own name.
    pub fn address() -> Option<String> {
        Some(format!(r"\\.\pipe\nightjar-link-{}", User::current()?.text()?))
    }

    /// A handle this code closes.
    struct Owned(HANDLE);
    // SAFETY: a kernel handle; only one thread uses it at a time.
    unsafe impl Send for Owned {}

    impl Drop for Owned {
        fn drop(&mut self) {
            // SAFETY: the handle is this value's own.
            let _ = unsafe { CloseHandle(self.0) };
        }
    }

    /// One end of the pipe, opened for overlapped I/O so that every step can give up.
    struct Conn {
        pipe: Owned,
        done: Owned,
    }

    impl Conn {
        fn new(pipe: Owned) -> io::Result<Self> {
            // SAFETY: an unnamed manual-reset event, as overlapped I/O asks.
            let done = unsafe { CreateEventW(None, true, false, PCWSTR::null()) }?;
            Ok(Self { pipe, done: Owned(done) })
        }

        fn overlapped(&self) -> OVERLAPPED {
            OVERLAPPED { hEvent: self.done.0, ..Default::default() }
        }

        /// Waits for what `started` began, up to `patience` milliseconds, and says how many bytes
        /// it moved. It returns only once the system is done with `overlapped` and the buffer.
        fn finish(&self, started: windows::core::Result<()>, overlapped: &OVERLAPPED, patience: u32) -> io::Result<u32> {
            let mut moved = 0;
            if let Err(e) = started {
                if e.code() != ERROR_IO_PENDING.to_hresult() {
                    return Err(e.into());
                }
                // SAFETY: both handles are this value's own, and `overlapped` stays alive and
                // unmoved until the wait below has seen the end of the operation.
                unsafe {
                    if WaitForSingleObject(self.done.0, patience) != WAIT_OBJECT_0 {
                        let _ = CancelIoEx(self.pipe.0, Some(overlapped));
                        let _ = GetOverlappedResult(self.pipe.0, overlapped, &mut moved, true);
                        return Err(io::ErrorKind::TimedOut.into());
                    }
                }
            }
            // SAFETY: as above; the operation has ended.
            unsafe { GetOverlappedResult(self.pipe.0, overlapped, &mut moved, false) }?;
            Ok(moved)
        }

        /// Waits for a sender, for as long as it takes.
        fn accept(&self) -> io::Result<()> {
            let mut overlapped = self.overlapped();
            // SAFETY: a valid pipe; `finish` keeps `overlapped` alive until it is done.
            let started = unsafe { ConnectNamedPipe(self.pipe.0, Some(&raw mut overlapped)) };
            if matches!(&started, Err(e) if e.code() == ERROR_PIPE_CONNECTED.to_hresult()) {
                return Ok(());
            }
            self.finish(started, &overlapped, INFINITE).map(|_| ())
        }
    }

    impl Read for Conn {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let mut overlapped = self.overlapped();
            // SAFETY: a valid pipe; `finish` keeps the buffer and `overlapped` in use until it is done.
            let started = unsafe { ReadFile(self.pipe.0, Some(&mut *buffer), None, Some(&raw mut overlapped)) };
            self.finish(started, &overlapped, PATIENCE.as_millis() as u32).map(|moved| moved as usize)
        }
    }

    impl Write for Conn {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            let mut overlapped = self.overlapped();
            // SAFETY: as in `read`.
            let started = unsafe { WriteFile(self.pipe.0, Some(buffer), None, Some(&raw mut overlapped)) };
            self.finish(started, &overlapped, PATIENCE.as_millis() as u32).map(|moved| moved as usize)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn open(name: &[u16]) -> Option<Owned> {
        let flags = FILE_FLAGS_AND_ATTRIBUTES(FILE_FLAG_OVERLAPPED.0 | SECURITY_SQOS_PRESENT.0 | SECURITY_IDENTIFICATION.0);
        for _ in 0..2 {
            // SAFETY: a NUL-terminated name; the handle goes to the caller. The flags keep the
            // pipe's creator from acting as this user.
            let opened = unsafe { CreateFileW(PCWSTR(name.as_ptr()), (GENERIC_READ | GENERIC_WRITE).0, FILE_SHARE_NONE, None, OPEN_EXISTING, flags, None) };
            match opened {
                Ok(pipe) => return Some(Owned(pipe)),
                // SAFETY: a NUL-terminated name.
                Err(e) if e.code() == ERROR_PIPE_BUSY.to_hresult() => {
                    let _ = unsafe { WaitNamedPipeW(PCWSTR(name.as_ptr()), BUSY_PATIENCE.as_millis() as u32) };
                }
                Err(_) => return None,
            }
        }
        None
    }

    pub fn send_to(name: &str, link: &str) -> bool {
        let Some(pipe) = open(&wide(name)) else { return false };
        User::current().is_some_and(|user| user.owns(pipe.0)) && Conn::new(pipe).is_ok_and(|mut conn| hand_over(&mut conn, link))
    }

    /// The pipe, open to this user alone: under the default rules any local account can open it
    /// for reading, and with one instance that alone would keep the links from this window. The
    /// user owns it even in an elevated window, whose objects the Administrators group owns
    /// otherwise, so a sender that is not elevated still recognizes it.
    fn create(name: &str) -> Result<Owned, String> {
        let name = wide(name);
        let sid = User::current().and_then(|user| user.text()).ok_or("this user has no SID")?;
        let rules = wide(&format!("O:{sid}D:P(A;;GA;;;{sid})"));
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        let buffer = MAX_LEN as u32 + 1;
        // SAFETY: NUL-terminated strings and plain flags; the descriptor is freed after the call
        // that reads it. One instance, reused for every link, so the name is never free for
        // someone else while this window is open; the first-instance flag makes this fail when
        // someone has it now.
        let pipe = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(PCWSTR(rules.as_ptr()), SDDL_REVISION_1, &mut descriptor, None).map_err(|e| e.to_string())?;
            let attributes = SECURITY_ATTRIBUTES {
                nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: descriptor.0,
                bInheritHandle: false.into(),
            };
            let pipe = CreateNamedPipeW(
                PCWSTR(name.as_ptr()),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE | FILE_FLAG_OVERLAPPED,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                buffer,
                buffer,
                0,
                Some(&attributes),
            );
            let made = if pipe.is_invalid() { Err(io::Error::last_os_error().to_string()) } else { Ok(Owned(pipe)) };
            let _ = LocalFree(Some(HLOCAL(descriptor.0)));
            made
        }?;
        Ok(pipe)
    }

    pub fn listen_at(name: &str, inbox: Arc<Inbox>, wake: Waker) -> Result<(), String> {
        run(create(name)?, inbox, wake)
    }

    fn run(pipe: Owned, inbox: Arc<Inbox>, wake: Waker) -> Result<(), String> {
        let mut conn = Conn::new(pipe).map_err(|e| e.to_string())?;
        thread(move || loop {
            if let Err(e) = conn.accept() {
                eprintln!("nightjar: pairing links cannot reach this window: {e}");
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
            serve(&mut conn, &inbox, &wake);
            // SAFETY: the pipe is this loop's own.
            let _ = unsafe { DisconnectNamedPipe(conn.pipe.0) };
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::time::Instant;
        use windows::Win32::Security::Authorization::ConvertSecurityDescriptorToStringSecurityDescriptorW;
        use windows::Win32::Security::DACL_SECURITY_INFORMATION;

        fn address(tag: &str) -> String {
            format!(r"\\.\pipe\nightjar-link-os-test-{}-{tag}", std::process::id())
        }

        /// Who may do what with the pipe, as the system holds it.
        fn rules_of(pipe: HANDLE) -> String {
            let mut descriptor = PSECURITY_DESCRIPTOR::default();
            let wanted = OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
            let mut text = PWSTR::null();
            // SAFETY: a valid pipe; the strings are freed once copied.
            unsafe {
                assert!(GetSecurityInfo(pipe, SE_KERNEL_OBJECT, wanted, None, None, None, None, Some(&mut descriptor)).is_ok());
                ConvertSecurityDescriptorToStringSecurityDescriptorW(descriptor, SDDL_REVISION_1, wanted, &mut text, None).unwrap();
                let rules = text.to_string().unwrap();
                let _ = LocalFree(Some(HLOCAL(text.0.cast())));
                let _ = LocalFree(Some(HLOCAL(descriptor.0)));
                rules
            }
        }

        #[test]
        fn the_pipe_is_this_users_alone() {
            let name = address("rules");
            let pipe = create(&name).unwrap();
            let sid = User::current().unwrap().text().unwrap();
            assert_eq!(rules_of(pipe.0), format!("O:{sid}D:P(A;;FA;;;{sid})"));
        }

        #[test]
        fn a_window_that_does_not_answer_cannot_hang_a_start() {
            let name = address("silent");
            // Made, and then never served: what a suspended window looks like from outside.
            let _pipe = create(&name).unwrap();
            let began = Instant::now();
            assert!(!send_to(&name, "rubylight://pair?v=1"), "this start opens its own window");
            assert!(began.elapsed() < PATIENCE * 3, "{:?}", began.elapsed());
        }

        #[test]
        fn a_start_that_says_nothing_cannot_hold_the_channel() {
            let name = address("mute");
            let inbox = Arc::new(Inbox::default());
            let wake: Waker = Arc::new(|| {});
            listen_at(&name, inbox.clone(), wake).unwrap();
            let _mute = open(&wide(&name)).unwrap();
            let began = Instant::now();
            let mut sent = false;
            while !sent && began.elapsed() < PATIENCE * 3 {
                sent = send_to(&name, "rubylight://pair?v=1");
            }
            assert!(sent, "the next start gets through once the silent one is dropped");
            assert_eq!(inbox.take().as_deref(), Some("rubylight://pair?v=1"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A connection that reads from one buffer and writes to another.
    struct Fake {
        input: Cursor<Vec<u8>>,
        output: Vec<u8>,
    }

    impl Fake {
        fn new(input: &[u8]) -> Self {
            Self { input: Cursor::new(input.to_vec()), output: Vec::new() }
        }
    }

    impl Read for Fake {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            self.input.read(buffer)
        }
    }

    impl Write for Fake {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.output.write(buffer)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn counter() -> (Arc<AtomicUsize>, Waker) {
        let count = Arc::new(AtomicUsize::new(0));
        let inner = count.clone();
        (count, Arc::new(move || {
            inner.fetch_add(1, Ordering::SeqCst);
        }))
    }

    #[test]
    fn a_line_reaches_the_inbox_and_the_sender_hears_of_it() {
        let inbox = Inbox::default();
        let (woken, wake) = counter();
        let mut conn = Fake::new(b"rubylight://pair?v=1\n");
        serve(&mut conn, &inbox, &wake);
        assert_eq!(inbox.take().as_deref(), Some("rubylight://pair?v=1"));
        assert_eq!(inbox.take(), None);
        assert_eq!(conn.output, [DELIVERED]);
        assert_eq!(woken.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_start_that_says_nothing_delivers_nothing() {
        let inbox = Inbox::default();
        let (woken, wake) = counter();
        for input in [&b""[..], b"no line end"] {
            let mut conn = Fake::new(input);
            serve(&mut conn, &inbox, &wake);
            assert_eq!(inbox.take(), None);
            assert!(conn.output.is_empty());
        }
        assert_eq!(woken.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn what_cannot_be_a_link_arrives_empty_for_the_window_to_refuse() {
        let long = vec![b'a'; 64 * 1024];
        assert_eq!(receive(&mut Fake::new(&long)), Some(String::new()), "no line end");
        let mut line = vec![b'a'; MAX_LEN + 1];
        line.push(b'\n');
        assert_eq!(receive(&mut Fake::new(&line)), Some(String::new()), "ends just past the limit");
        assert_eq!(receive(&mut Fake::new(b"\xff\xfe\n")), Some(String::new()), "not text");
        let mut fits = vec![b'a'; MAX_LEN];
        fits.push(b'\n');
        assert_eq!(receive(&mut Fake::new(&fits)), Some("a".repeat(MAX_LEN)));
    }

    #[test]
    fn the_sender_sends_only_what_the_window_can_read() {
        assert_eq!(wire(" link \n"), b"link\n");
        assert_eq!(wire("two\nlines"), b"\n");
        assert_eq!(wire(&"a".repeat(MAX_LEN + 1)), b"\n");
    }

    #[test]
    fn the_newest_link_is_the_one_that_waits() {
        let inbox = Inbox::default();
        inbox.put("first".into());
        inbox.put("second".into());
        assert_eq!(inbox.take().as_deref(), Some("second"));
        assert_eq!(inbox.take(), None);
    }

    #[test]
    fn the_socket_is_in_the_private_directory() {
        let run = || Some(std::ffi::OsString::from("/run/user/1000"));
        let base = std::path::Path::new("/run/user/1000");
        assert_eq!(socket_path(run(), None), Some(base.join("nightjar-link")));
        assert_eq!(
            socket_path(run(), Some("io.github.ramazankara.nightjar".into())),
            Some(base.join("app").join("io.github.ramazankara.nightjar").join("nightjar-link")),
            "shared between a Flatpak's starts"
        );
        assert_eq!(socket_path(None, None), None, "no channel without it");
        assert_eq!(socket_path(Some("".into()), None), None);
    }

    #[cfg(windows)]
    fn test_address(tag: &str) -> String {
        format!(r"\\.\pipe\nightjar-link-test-{}-{tag}", std::process::id())
    }

    #[cfg(unix)]
    fn test_address(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("nightjar-link-test-{}-{tag}", std::process::id()))
    }

    #[test]
    fn a_link_crosses_the_real_channel() {
        let address = test_address("real");
        assert!(!os::send_to(&address, "rubylight://pair?v=1"), "no window yet: this process opens one");
        let inbox = Arc::new(Inbox::default());
        let (woken, wake) = counter();
        os::listen_at(&address, inbox.clone(), wake.clone()).unwrap();
        assert!(os::listen_at(&address, Arc::new(Inbox::default()), wake).is_err(), "a second window has no channel");
        for link in ["rubylight://pair?v=1", "rubylight://pair?v=2"] {
            assert!(os::send_to(&address, link), "{link}");
            assert_eq!(inbox.take().as_deref(), Some(link));
        }
        assert_eq!(woken.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn the_packages_give_the_system_the_link() {
        let entry = include_str!("../packaging/linux/io.github.ramazankara.nightjar.desktop");
        assert!(entry.lines().any(|line| line == "Exec=nightjar %u"), "the link is the one argument");
        assert!(entry.lines().any(|line| line == "MimeType=x-scheme-handler/rubylight;"));
        let setup = include_str!("../packaging/windows/nightjar.iss");
        assert!(setup.contains(r#"Subkey: "Software\Classes\rubylight"; ValueType: string; ValueName: "URL Protocol""#));
        // Quoted, so a link is one argument however it is written.
        assert!(setup.contains(r#"ValueData: """{app}\nightjar.exe"" ""%1""""#));
    }
}
