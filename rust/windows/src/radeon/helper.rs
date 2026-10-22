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
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::PathBuf,
    process::{Command, Stdio},
    sync::mpsc,
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
enum Request {
    Read { gpu: i32 },
    Write { gpu: i32, id: String, value: Value },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum Response {
    Settings { settings: Vec<Setting> },
    Done,
    Error { message: String },
}

/// Answer one request against a backend. A request that does not parse is an error response, not
/// a panic.
fn handle(request: &str, backend: &mut impl RadeonBackend) -> Response {
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
    run_with(&mut AdlxBackend)
}

// The helper talks to its parent only through stdin, stdout and stderr.
#[allow(clippy::print_stdout, clippy::print_stderr)]
fn run_with(backend: &mut impl RadeonBackend) -> i32 {
    let mut request = String::new();
    if let Err(error) = std::io::stdin().read_to_string(&mut request) {
        eprintln!("radeon helper: cannot read its request: {error}");
        return 2;
    }
    let response = handle(&request, backend);
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

/// A [`RadeonBackend`] whose every call runs in a fresh helper process.
#[derive(Clone, Debug)]
pub struct HelperBackend {
    program: PathBuf,
    args: Vec<String>,
    timeout: Duration,
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
        }
    }
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    fn call(&self, request: &Request) -> Result<Response, BackendError> {
        let body = serde_json::to_string(request)
            .map_err(|e| BackendError(format!("cannot encode the helper request: {e}")))?;
        let mut command = Command::new(&self.program);
        command
            .args(&self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = command.spawn().map_err(|e| {
            BackendError(format!(
                "cannot start the Radeon helper {}: {e}",
                self.program.display()
            ))
        })?;
        if let Some(mut stdin) = child.stdin.take() {
            // A helper that died early closes the pipe; its exit status explains it below.
            let _ = stdin.write_all(body.as_bytes());
        }
        let mut stdout = child.stdout.take().expect("stdout is piped");
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let mut text = String::new();
            let _ = stdout.read_to_string(&mut text);
            let _ = sender.send(text);
        });
        let deadline = Instant::now() + self.timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(BackendError(format!(
                        "the Radeon helper did not answer within {} s and was stopped; the driver may be stalled, and the setting it was asked to change may or may not have changed",
                        self.timeout.as_secs()
                    )));
                }
                Err(error) => {
                    return Err(BackendError(format!(
                        "cannot wait for the Radeon helper: {error}"
                    )));
                }
            }
        };
        let output = receiver
            .recv_timeout(Duration::from_secs(5))
            .unwrap_or_default();
        output
            .lines()
            .find_map(|line| line.split_once(MARKER).map(|(_, answer)| answer))
            .ok_or_else(|| {
                BackendError(format!(
                    "the Radeon helper exited ({status}) without an answer"
                ))
            })
            .and_then(|line| {
                serde_json::from_str(line)
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
mod tests {
    use super::*;

    const TOGGLE: &str = r#"{"id":"anti_lag.enabled","group":"anti_lag","label":"Anti-Lag","interface":"IADLX3DAntiLag","kind":"toggle","state":"configurable","supported":true,"readable":true,"writable":"yes","value":false,"depends":[{"group":"chill","relation":"excludes","note":""}],"notes":[],"adlx":{"Supported":"ADLX_OK"}}"#;

    /// Answers like a driver with one setting, and remembers a write.
    #[derive(Default)]
    struct Stub {
        wrote: Option<(i32, String, Value)>,
    }
    impl RadeonBackend for Stub {
        fn read(&mut self, gpu: i32) -> Result<Vec<Setting>, BackendError> {
            if gpu == 9 {
                return Err(BackendError("no such GPU".into()));
            }
            Ok(vec![serde_json::from_str(TOGGLE).unwrap()])
        }
        fn write(&mut self, gpu: i32, id: &str, value: &Value) -> Result<(), BackendError> {
            if id == "refused" {
                return Err(BackendError("the driver said no".into()));
            }
            self.wrote = Some((gpu, id.into(), value.clone()));
            Ok(())
        }
    }

    #[test]
    fn a_setting_survives_the_trip_through_the_helper_protocol() {
        let setting: Setting = serde_json::from_str(TOGGLE).unwrap();
        assert_eq!(setting.id, "anti_lag.enabled");
        assert_eq!(setting.value, Some(Value::Bool(false)));
        assert_eq!(setting.depends[0].group, "chill");
        assert_eq!(setting.adlx["Supported"], "ADLX_OK");
        let again: serde_json::Value =
            serde_json::to_value(serde_json::from_str::<Setting>(TOGGLE).unwrap()).unwrap();
        assert_eq!(
            again,
            serde_json::from_str::<serde_json::Value>(TOGGLE).unwrap()
        );
    }

    #[test]
    fn handle_answers_reads_and_writes_and_reports_failures() {
        let mut stub = Stub::default();
        match handle(r#"{"op":"read","gpu":1}"#, &mut stub) {
            Response::Settings { settings } => assert_eq!(settings.len(), 1),
            other => panic!("{other:?}"),
        }
        let write = r#"{"op":"write","gpu":1,"id":"anti_lag.enabled","value":true}"#;
        assert!(matches!(handle(write, &mut stub), Response::Done));
        assert_eq!(
            stub.wrote,
            Some((1, "anti_lag.enabled".into(), Value::Bool(true)))
        );
        match handle(r#"{"op":"read","gpu":9}"#, &mut stub) {
            Response::Error { message } => assert_eq!(message, "no such GPU"),
            other => panic!("{other:?}"),
        }
        match handle(
            r#"{"op":"write","gpu":1,"id":"refused","value":1}"#,
            &mut stub,
        ) {
            Response::Error { message } => assert_eq!(message, "the driver said no"),
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            handle("not json", &mut stub),
            Response::Error { .. }
        ));
    }

    /// The helper side of the process tests below: this test binary acting as the helper.
    #[test]
    #[ignore = "child of the helper process tests"]
    fn stub_helper() {
        assert_eq!(run_with(&mut Stub::default()), 0);
    }

    fn stub_backend() -> HelperBackend {
        HelperBackend::with_command(
            std::env::current_exe().unwrap(),
            [
                "--ignored",
                "--exact",
                "radeon::helper::tests::stub_helper",
                "--nocapture",
                "--test-threads=1",
            ]
            .map(String::from)
            .to_vec(),
        )
    }

    #[test]
    fn a_helper_process_reads_and_writes_over_the_protocol() {
        let mut backend = stub_backend();
        let settings = backend.read(1).unwrap();
        assert_eq!(settings[0].id, "anti_lag.enabled");
        backend
            .write(1, "anti_lag.enabled", &Value::Bool(true))
            .unwrap();
        assert_eq!(
            backend.read(9).unwrap_err().0,
            "no such GPU",
            "a helper-side failure comes back as the error"
        );
        assert_eq!(
            backend.write(1, "refused", &Value::Int(1)).unwrap_err().0,
            "the driver said no"
        );
    }

    #[test]
    fn a_helper_that_cannot_start_is_an_error_naming_it() {
        let mut backend =
            HelperBackend::with_command(PathBuf::from(r"C:\no\such\helper.exe"), vec![]);
        let error = backend.read(1).unwrap_err().0;
        assert!(error.contains("cannot start the Radeon helper"), "{error}");
        assert!(error.contains("helper.exe"), "{error}");
    }

    #[test]
    fn a_helper_that_exits_without_an_answer_is_an_error() {
        let mut backend = HelperBackend::with_command(
            PathBuf::from("cmd"),
            vec!["/c".into(), "exit".into(), "7".into()],
        );
        let error = backend.read(1).unwrap_err().0;
        assert!(error.contains("without an answer"), "{error}");
        assert!(error.contains('7'), "{error}");
    }

    #[test]
    fn a_hung_helper_is_killed_at_the_timeout() {
        let mut backend = HelperBackend::with_command(
            PathBuf::from("ping"),
            ["-n", "60", "127.0.0.1"].map(String::from).to_vec(),
        )
        .with_timeout(Duration::from_millis(400));
        let started = Instant::now();
        let error = backend.write(1, "x", &Value::Bool(true)).unwrap_err().0;
        assert!(error.contains("did not answer"), "{error}");
        assert!(error.contains("may or may not have changed"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(10));
    }
}
