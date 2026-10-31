//! Radeon reads and writes in a short-lived helper process.
//!
//! A long-running process's ADLX 3D-settings view never refreshes after another process changes a
//! setting: a new session, `ADLXInitializeWithIncompatibleDriver` and a 12 s wait all kept the
//! stale values, terminating ADLX crashes, and a write of the value the stale view believes is
//! current is silently dropped (`docs/hardware-matrix.md`, "ADLX view refresh"). A fresh process
//! sees the change in about 130 ms. So the host never reads or writes ADLX settings itself: each
//! call runs the host exe with the hidden `--radeon-helper` flag, which does that one read or
//! write against a brand-new ADLX and exits. The helper is killed if it hangs, so a driver stall
//! cannot freeze the host either.
use super::{AdlxBackend, BackendError, RadeonBackend, Setting, model::Value};
use crate::process::{Process, Target};
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    io::{BufRead, Read, Write},
    path::PathBuf,
    process::{Command, Stdio},
    sync::{Arc, Mutex, PoisonError, mpsc},
    time::{Duration, Instant},
};

/// The hidden command-line flag that makes the host exe act as the helper.
pub const FLAG: &str = "--radeon-helper";
/// How long one helper call may take before it is killed. A read or write takes well under a
/// second; this only bounds a hung driver.
pub const TIMEOUT: Duration = Duration::from_secs(30);
/// Marks the helper's answer; anything else it prints, even earlier on the same line (a test
/// harness prints "test name ... " first), is ignored.
const MARKER: &str = "RADEON-HELPER-RESPONSE ";

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub(super) enum Request {
    Read { gpu: i32 },
    Write { gpu: i32, id: String, value: Value },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(super) enum Response {
    Settings { settings: Vec<Setting> },
    Done,
    Error { message: String },
}

/// Answer one request against a backend. A request that does not parse is an error response, not
/// a panic.
pub(super) fn handle(request: &str, backend: &mut impl RadeonBackend) -> Response {
    let request: Request = match serde_json::from_str(request) {
        Ok(request) => request,
        Err(error) => {
            return Response::Error {
                message: format!("unreadable helper request: {error}"),
            };
        }
    };
    let answer = match request {
        Request::Read { gpu } => backend
            .read(gpu)
            .map(|settings| Response::Settings { settings }),
        Request::Write { gpu, id, value } => {
            backend.write(gpu, &id, &value).map(|()| Response::Done)
        }
    };
    answer.unwrap_or_else(|error| Response::Error { message: error.0 })
}

/// The helper's entry point (`butterpollo.exe --radeon-helper`): read one request from stdin, do
/// it against a fresh ADLX, print one answer line. The exit code is non-zero only when no answer
/// could be produced.
pub fn run() -> i32 {
    run_with(&mut AdlxBackend, refuse_system)
}

/// The helper never runs as SYSTEM. The installed host is SYSTEM in the console session, and a
/// SYSTEM process reads some settings (the AFMF modes) as their defaults and silently drops writes
/// of them, while the user's own view is the one the Radeon software shows
/// (`docs/hardware-matrix.md`, "Which account"). So a SYSTEM host starts the helper with the
/// signed-in user's token (`HelperBackend::start`), and a helper that finds itself SYSTEM anyway
/// answers with an error instead of the wrong account's settings. Impersonation is not an option:
/// a SYSTEM process that loads the driver's DLLs while impersonating a user resolves their paths
/// through that user's device map.
fn refuse_system() -> Result<(), String> {
    if crate::process::is_system() {
        Err(
            "the Radeon helper was started as SYSTEM; the host must start it as the signed-in user"
                .into(),
        )
    } else {
        Ok(())
    }
}

/// Answer a request after `setup` succeeded; a setup that fails is the answer.
fn answer(
    request: &str,
    backend: &mut impl RadeonBackend,
    setup: impl FnOnce() -> Result<(), String>,
) -> Response {
    match setup() {
        Ok(()) => handle(request, backend),
        Err(message) => Response::Error { message },
    }
}

// The helper talks to its parent only through stdin, stdout and stderr.
#[allow(clippy::print_stdout, clippy::print_stderr)]
fn run_with(backend: &mut impl RadeonBackend, setup: impl FnOnce() -> Result<(), String>) -> i32 {
    // Exactly one request line: the helper never waits for stdin to close, because another
    // process that inherited a copy of the host's end can keep it open.
    let mut request = String::new();
    if let Err(error) = std::io::stdin().lock().read_line(&mut request) {
        eprintln!("radeon helper: cannot read its request: {error}");
        return 2;
    }
    let response = answer(&request, backend, setup);
    match serde_json::to_string(&response) {
        Ok(line) => {
            println!("{MARKER}{line}");
            0
        }
        Err(error) => {
            eprintln!("radeon helper: cannot encode its answer: {error}");
            3
        }
    }
}

/// How long a killed helper gets to disappear before it is abandoned.
const KILL_GRACE: Duration = Duration::from_secs(3);
/// How much of a helper's stderr goes into an error.
const STDERR_LIMIT: usize = 400;

/// Read a pipe to its end on a thread; the text arrives on the channel once the pipe closes.
fn drain(mut pipe: impl Read + Send + 'static) -> mpsc::Receiver<String> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = pipe.read_to_end(&mut bytes);
        let _ = sender.send(String::from_utf8_lossy(&bytes).into_owned());
    });
    receiver
}

/// Watch a helper's stdout for its answer line: the answer arrives on the channel as soon as the
/// line does, without waiting for the pipe to close (another process that inherited a copy of
/// the helper's end can keep it open for good). The rest of the output is read and dropped so
/// the helper never blocks on a full pipe. A pipe that ends without an answer closes the channel.
fn watch_for_answer(pipe: impl Read + Send + 'static) -> mpsc::Receiver<String> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = std::io::BufReader::new(pipe);
        let mut line = Vec::new();
        loop {
            line.clear();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
            let text = String::from_utf8_lossy(&line);
            if let Some((_, answer)) = text.split_once(MARKER) {
                let _ = sender.send(answer.trim_end_matches(['\r', '\n']).to_string());
                let _ = std::io::copy(&mut reader, &mut std::io::sink());
                return;
            }
        }
    });
    receiver
}

/// The helper's stderr as a short suffix for an error message (empty when it said nothing).
fn what_it_said(errors: &mpsc::Receiver<String>) -> String {
    let text = errors
        .recv_timeout(Duration::from_millis(500))
        .unwrap_or_default();
    let text = text.trim();
    if text.is_empty() {
        return String::new();
    }
    let shown: String = text.chars().take(STDERR_LIMIT).collect();
    let cut = if text.chars().count() > STDERR_LIMIT {
        "..."
    } else {
        ""
    };
    format!("; it said: {shown}{cut}")
}

/// A helper process: the host's own account, or the signed-in user for a SYSTEM host.
enum Child {
    Own(std::process::Child),
    User(Process),
}
impl std::fmt::Debug for Child {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "helper process {}", self.pid())
    }
}
impl Child {
    fn pid(&self) -> u32 {
        match self {
            Self::Own(child) => child.id(),
            Self::User(process) => process.pid,
        }
    }
    fn kill(&mut self) {
        match self {
            Self::Own(child) => {
                let _ = child.kill();
            }
            Self::User(process) => {
                let _ = process.terminate();
            }
        }
    }
    /// How it ended, once it has.
    fn ended(&mut self) -> Result<Option<String>, String> {
        match self {
            Self::Own(child) => child
                .try_wait()
                .map(|status| status.map(|s| s.to_string()))
                .map_err(|e| e.to_string()),
            Self::User(process) => process
                .exit_code()
                .map(|code| code.map(|c| format!("exit code: {c}")))
                .map_err(|e| format!("{e:#}")),
        }
    }
}

/// A [`RadeonBackend`] whose every call runs in a fresh helper process.
#[derive(Clone, Debug)]
pub struct HelperBackend {
    program: PathBuf,
    args: Vec<String>,
    timeout: Duration,
    /// Helpers that were still running after being killed, shared by every clone. While one is
    /// alive no other starts: it may be mid-write in the driver, and a second process writing
    /// beside it could race it.
    hung: Arc<Mutex<Vec<Child>>>,
}
impl HelperBackend {
    /// The running host exe in helper mode.
    pub fn host() -> Result<Self, BackendError> {
        let program = std::env::current_exe()
            .map_err(|e| BackendError(format!("cannot locate the host exe for the helper: {e}")))?;
        Ok(Self::with_command(program, vec![FLAG.into()]))
    }
    /// Any program that speaks the helper protocol (the hardware tests run the test binary).
    pub fn with_command(program: PathBuf, args: Vec<String>) -> Self {
        Self {
            program,
            args,
            timeout: TIMEOUT,
            hung: Arc::default(),
        }
    }
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Refuse to start a helper while one that did not die is still running; forget the ones that
    /// have gone since.
    fn ensure_no_helper_is_hung(&self) -> Result<(), BackendError> {
        let mut hung = self.hung.lock().unwrap_or_else(PoisonError::into_inner);
        // A helper whose state cannot be read counts as still running.
        hung.retain_mut(|child| !matches!(child.ended(), Ok(Some(_))));
        match hung.first() {
            Some(child) => Err(BackendError(format!(
                "the previous Radeon helper (pid {}) is still hung, so no new one was started; the driver may be stalled",
                child.pid()
            ))),
            None => Ok(()),
        }
    }

    /// Start a helper and hand it the request: as the signed-in user when this host is SYSTEM (see
    /// [`refuse_system`]), as this host's own account otherwise. Both run in a job that ends with
    /// the host.
    fn start(
        &self,
        body: &str,
    ) -> Result<(Child, mpsc::Receiver<String>, mpsc::Receiver<String>), BackendError> {
        let cannot = |how: &str, error: String| {
            BackendError(format!(
                "cannot start the Radeon helper {}{how}: {error}",
                self.program.display()
            ))
        };
        // The request is one line (compact JSON has no newline); the helper reads that line and
        // does not wait for the pipe to close.
        let line = format!("{body}\n");
        if crate::process::is_system() {
            let args: Vec<OsString> = self.args.iter().map(OsString::from).collect();
            let mut piped =
                Process::spawn_piped(&self.program, &args, Target::User { elevated: false })
                    .map_err(|e| cannot(" as the signed-in user", format!("{e:#}")))?;
            if let Some(mut stdin) = piped.stdin.take() {
                // A helper that died early closes the pipe; its exit status explains it below.
                let _ = stdin.write_all(line.as_bytes());
            }
            let output = watch_for_answer(piped.stdout.take().expect("stdout is piped"));
            let errors = drain(piped.stderr.take().expect("stderr is piped"));
            return Ok((Child::User(piped.process), output, errors));
        }
        let mut command = Command::new(&self.program);
        command
            .args(&self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = command.spawn().map_err(|e| cannot("", e.to_string()))?;
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(line.as_bytes());
        }
        let output = watch_for_answer(child.stdout.take().expect("stdout is piped"));
        let errors = drain(child.stderr.take().expect("stderr is piped"));
        Ok((Child::Own(child), output, errors))
    }

    fn call(&self, request: &Request) -> Result<Response, BackendError> {
        let body = serde_json::to_string(request)
            .map_err(|e| BackendError(format!("cannot encode the helper request: {e}")))?;
        self.ensure_no_helper_is_hung()?;
        let (mut child, receiver, errors) = self.start(&body)?;
        let deadline = Instant::now() + self.timeout;
        let status = loop {
            match child.ended() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Ok(None) => {
                    child.kill();
                    // A process stuck inside the driver may not die when killed. Wait a few
                    // seconds, then abandon it (its two reader threads stay blocked with it)
                    // rather than hang the caller too; no other helper starts until it is gone.
                    let gone = Instant::now() + KILL_GRACE;
                    let ended = loop {
                        match child.ended() {
                            Ok(Some(_)) => break true,
                            Ok(None) if Instant::now() < gone => {
                                std::thread::sleep(Duration::from_millis(20));
                            }
                            _ => break false,
                        }
                    };
                    let said = what_it_said(&errors);
                    if !ended {
                        self.hung
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .push(child);
                    }
                    return Err(BackendError(format!(
                        "the Radeon helper did not answer within {} s and {}; the driver may be stalled, and the setting it was asked to change may or may not have changed{said}",
                        self.timeout.as_secs(),
                        if ended {
                            "was stopped"
                        } else {
                            "could not be stopped and was abandoned"
                        },
                    )));
                }
                Err(error) => {
                    return Err(BackendError(format!(
                        "cannot wait for the Radeon helper: {error}"
                    )));
                }
            }
        };
        // The helper has exited, so its answer is already in the pipe and arrives at once; a
        // closed channel means it ended without one. Only a pipe some other process keeps open
        // makes this wait out the timeout.
        receiver
            .recv_timeout(Duration::from_secs(2))
            .map_err(|_| {
                BackendError(format!(
                    "the Radeon helper exited ({status}) without an answer{}",
                    what_it_said(&errors)
                ))
            })
            .and_then(|line| {
                serde_json::from_str(&line)
                    .map_err(|e| BackendError(format!("unreadable Radeon helper answer: {e}")))
            })
    }
}

impl RadeonBackend for HelperBackend {
    fn read(&mut self, gpu: i32) -> Result<Vec<Setting>, BackendError> {
        match self.call(&Request::Read { gpu })? {
            Response::Settings { settings } => Ok(settings),
            Response::Error { message } => Err(BackendError(message)),
            Response::Done => Err(BackendError(
                "the Radeon helper answered a read with done".into(),
            )),
        }
    }
    fn write(&mut self, gpu: i32, id: &str, value: &Value) -> Result<(), BackendError> {
        let request = Request::Write {
            gpu,
            id: id.into(),
            value: value.clone(),
        };
        match self.call(&request)? {
            Response::Done => Ok(()),
            Response::Error { message } => Err(BackendError(message)),
            Response::Settings { .. } => Err(BackendError(
                "the Radeon helper answered a write with settings".into(),
            )),
        }
    }
}

#[cfg(test)]
mod tests;
