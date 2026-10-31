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
    assert_eq!(run_with(&mut Stub::default(), || Ok(())), 0);
}

/// A helper whose stdin and stdout ends were inherited by a process that outlives it: the
/// survivor holds a second copy of both open, so the pipes do not close when the helper exits.
#[test]
#[ignore = "child of the helper process tests"]
#[allow(clippy::zombie_processes)] // the survivor is meant to outlive this process
fn leaky_stub_helper() {
    let _survivor = Command::new("ping")
        .args(["-n", "12", "127.0.0.1"])
        .spawn()
        .unwrap();
    assert_eq!(run_with(&mut Stub::default(), || Ok(())), 0);
}

fn stub_args(test: &str) -> Vec<String> {
    [
        "--ignored",
        "--exact",
        test,
        "--nocapture",
        "--test-threads=1",
    ]
    .map(String::from)
    .to_vec()
}

/// Another process holding a copy of the host's stdin end keeps the helper from ever seeing
/// EOF; it answers after the one request line anyway.
#[test]
fn a_helper_answers_after_one_line_though_its_stdin_never_closes() {
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(stub_args("radeon::helper::tests::stub_helper"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(b"{\"op\":\"read\",\"gpu\":1}\n").unwrap();
    let answers = watch_for_answer(child.stdout.take().unwrap());
    let answer = answers
        .recv_timeout(Duration::from_secs(10))
        .expect("an answer with stdin still open");
    assert!(answer.contains("anti_lag.enabled"), "{answer}");
    let deadline = Instant::now() + Duration::from_secs(10);
    while child.try_wait().unwrap().is_none() {
        assert!(
            Instant::now() < deadline,
            "the helper waited for stdin to close"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(stdin);
}

/// A survivor holding copies of the helper's stdin and stdout keeps stdout from closing; the
/// call still takes the answer line and completes.
#[test]
fn a_call_completes_though_a_survivor_holds_the_pipes_open() {
    let mut backend = HelperBackend::with_command(
        std::env::current_exe().unwrap(),
        stub_args("radeon::helper::tests::leaky_stub_helper"),
    );
    let started = Instant::now();
    let settings = backend.read(1).unwrap();
    assert_eq!(settings[0].id, "anti_lag.enabled");
    assert!(
        started.elapsed() < Duration::from_secs(8),
        "the call waited for the pipes to close: {:?}",
        started.elapsed()
    );
}

/// A helper that cannot act as the user must say so and touch nothing, not answer from the
/// wrong account.
#[test]
fn a_helper_that_cannot_act_as_the_user_answers_with_an_error_and_does_nothing() {
    let mut stub = Stub::default();
    let request = r#"{"op":"write","gpu":0,"id":"anti_lag.enabled","value":true}"#;
    match answer(request, &mut stub, || Err("no signed-in user".into())) {
        Response::Error { message } => assert_eq!(message, "no signed-in user"),
        other => panic!("{other:?}"),
    }
    assert!(stub.wrote.is_none());
    assert!(matches!(
        answer(request, &mut stub, || Ok(())),
        Response::Done
    ));
    assert!(stub.wrote.is_some());
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
    let mut backend = HelperBackend::with_command(PathBuf::from(r"C:\no\such\helper.exe"), vec![]);
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
fn what_a_failed_helper_printed_is_in_the_error_and_truncated() {
    let mut backend = HelperBackend::with_command(
        PathBuf::from("cmd"),
        vec!["/c".into(), "echo boom 1>&2 & exit 3".into()],
    );
    let error = backend.read(1).unwrap_err().0;
    assert!(error.contains("it said: boom"), "{error}");

    let long = "x".repeat(STDERR_LIMIT * 3);
    let (sender, receiver) = mpsc::channel();
    sender.send(long).unwrap();
    let said = what_it_said(&receiver);
    assert!(said.ends_with("..."), "{said}");
    assert!(said.len() < STDERR_LIMIT + 40, "{said}");
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

/// A helper that survived its kill blocks every later call, from every clone of the backend,
/// until it is gone; then calls go through again.
#[test]
fn no_helper_starts_while_an_abandoned_one_is_still_alive() {
    let mut backend = HelperBackend::with_command(
        PathBuf::from("cmd"),
        vec!["/c".into(), "exit".into(), "7".into()],
    );
    let survivor = Command::new("ping")
        .args(["-n", "60", "127.0.0.1"])
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let pid = survivor.id();
    backend.hung.lock().unwrap().push(Child::Own(survivor));

    for clone in [&mut backend.clone(), &mut backend] {
        let error = clone.read(1).unwrap_err().0;
        assert!(error.contains("still hung"), "{error}");
        assert!(error.contains(&pid.to_string()), "{error}");
        assert!(
            !error.contains("without an answer"),
            "no helper was started: {error}"
        );
    }

    backend.hung.lock().unwrap()[0].kill();
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline
        && !matches!(backend.hung.lock().unwrap()[0].ended(), Ok(Some(_)))
    {
        std::thread::sleep(Duration::from_millis(20));
    }
    let error = backend.read(1).unwrap_err().0;
    assert!(error.contains("without an answer"), "{error}");
    assert!(backend.hung.lock().unwrap().is_empty());
}

/// The helper refuses to answer as SYSTEM, whatever else is true.
#[test]
fn a_helper_refuses_to_run_as_system() {
    assert_eq!(refuse_system().is_err(), crate::process::is_system());
}
