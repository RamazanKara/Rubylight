//! The native API's Radeon endpoints (plan item A3, brief 4) against a fake driver. No test
//! here touches the real GPU: the worker is the real one, the backend under it is `Fake`.
use super::*;
use crate::radeon_session::WorkerRef;
use butterpollo_core::{
    config::Config,
    native_api::radeon::{
        self as api, ControlState, Kind as ApiKind, Restore as ApiRestore, Value as Api,
    },
};
use butterpollo_windows::radeon::{
    BackendError, Entry, GpuInfo, Journal, RadeonBackend,
    control::JOURNAL_FILE,
    model::{ChoiceOption, Kind, Range, Setting as Driven, State, Value as Raw, Writable},
    worker::{Phase, Timing, Worker},
};
use serde_json::json;
use std::{
    collections::BTreeMap,
    sync::Mutex,
    time::{Duration, Instant as Clock},
};

/// An ADLX unique id above `i32::MAX` once read as unsigned, as real ones can be.
const GPU_ID: u32 = 0xC000_0001;
const GPU: u64 = GPU_ID as u64;

#[derive(Default)]
struct Driver {
    settings: Vec<Driven>,
    gpus: Vec<GpuInfo>,
    writes: Vec<String>,
    reads: usize,
    /// A write of this id fails.
    refuse: Option<&'static str>,
    /// A write of this id succeeds and changes nothing.
    ignore: Option<&'static str>,
    /// Every write fails.
    refuse_all: bool,
    unreadable: bool,
    /// Reads wait until this clears (a driver call that takes its time).
    hold: bool,
}
#[derive(Clone, Default)]
struct Fake(Arc<Mutex<Driver>>);
impl Fake {
    fn with(&self, change: impl FnOnce(&mut Driver)) {
        change(&mut self.0.lock().unwrap());
    }
    fn value(&self, id: &str) -> Raw {
        let d = self.0.lock().unwrap();
        d.settings
            .iter()
            .find(|s| s.id == id)
            .and_then(|s| s.value.clone())
            .unwrap()
    }
    fn set(&self, id: &str, value: Raw) {
        self.with(|d| {
            d.settings.iter_mut().find(|s| s.id == id).unwrap().value = Some(value);
        });
    }
    fn writes(&self) -> Vec<String> {
        self.0.lock().unwrap().writes.clone()
    }
    fn reads(&self) -> usize {
        self.0.lock().unwrap().reads
    }
}
impl RadeonBackend for Fake {
    fn read(&mut self, _gpu: i32) -> Result<Vec<Driven>, BackendError> {
        while self.0.lock().unwrap().hold {
            std::thread::sleep(Duration::from_millis(5));
        }
        let mut d = self.0.lock().unwrap();
        d.reads += 1;
        if d.unreadable {
            return Err(BackendError("ADLX_FAIL".into()));
        }
        Ok(d.settings.clone())
    }
    fn write(&mut self, _gpu: i32, id: &str, value: &Raw) -> Result<(), BackendError> {
        let mut d = self.0.lock().unwrap();
        d.writes.push(format!("{id}={}", value.display()));
        if d.refuse_all || d.refuse == Some(id) {
            return Err(BackendError("ADLX_NOT_SUPPORTED".into()));
        }
        if d.ignore == Some(id) {
            return Ok(());
        }
        if let Some(setting) = d.settings.iter_mut().find(|s| s.id == id) {
            setting.value = Some(value.clone());
        }
        Ok(())
    }
    fn gpus(&mut self) -> Result<Vec<GpuInfo>, BackendError> {
        Ok(self.0.lock().unwrap().gpus.clone())
    }
}

fn driven(id: &'static str, kind: Kind, state: State, value: Raw) -> Driven {
    Driven {
        id,
        group: id.split('.').next().unwrap(),
        label: id,
        interface: "fake",
        kind,
        state,
        supported: Some(state != State::Unsupported),
        readable: Some(true),
        writable: Writable::Yes,
        value: Some(value),
        unit: None,
        range: None,
        options: vec![],
        depends: vec![],
        notes: vec![],
        adlx: BTreeMap::new(),
    }
}
fn toggle(id: &'static str, state: State, on: bool) -> Driven {
    driven(id, Kind::Toggle, state, Raw::Bool(on))
}
fn integer(id: &'static str, state: State, value: i64) -> Driven {
    let mut setting = driven(id, Kind::Integer, state, Raw::Int(value));
    setting.range = Some(Range {
        min: 30,
        max: 300,
        step: 10,
    });
    setting.unit = Some("fps");
    setting
}
fn choice(id: &'static str, value: &str) -> Driven {
    let mut setting = driven(
        id,
        Kind::Choice,
        State::Configurable,
        Raw::Choice(value.into()),
    );
    setting.options = ["a", "on_unless_app_specifies", "c"]
        .iter()
        .enumerate()
        .map(|(raw, name)| ChoiceOption {
            value: (*name).into(),
            raw: raw as i32,
        })
        .collect();
    setting
}
fn gpu() -> GpuInfo {
    GpuInfo {
        id: GPU_ID as i32,
        name: "AMD Radeon RX 7900 XT".into(),
        driver: "Adrenalin 26.2.1".into(),
    }
}
/// One setting of every state.
fn driver() -> Fake {
    let fake = Fake::default();
    fake.with(|d| {
        d.gpus = vec![gpu()];
        d.settings = vec![
            toggle("anti_lag.enabled", State::Configurable, false),
            integer("pacing.limit", State::Configurable, 140),
            choice("tessellation.mode", "a"),
            toggle("rsr.enabled", State::ReadOnly, false),
            toggle("sam.enabled", State::NeedsExternalSetup, false),
            integer("chill.min_fps", State::TemporarilyUnavailable, 60),
            toggle("boost.enabled", State::Unsupported, false),
        ];
    });
    fake
}
/// A panel of only `settings`.
fn only(settings: Vec<Driven>) -> Fake {
    let fake = driver();
    fake.with(|d| d.settings = settings);
    fake
}

/// The host with a Radeon worker over a fake driver.
struct Rig {
    s: Setup,
    fake: Fake,
}
impl std::ops::Deref for Rig {
    type Target = Setup;
    fn deref(&self) -> &Setup {
        &self.s
    }
}
impl Rig {
    fn new() -> Self {
        Self::with(driver())
    }
    fn with(fake: Fake) -> Self {
        let rig = Self::start(Setup::new(), fake);
        rig.ready();
        rig
    }
    /// The worker starts its recovery at once and this returns without waiting for it.
    fn start(s: Setup, fake: Fake) -> Self {
        let worker = Worker::start(
            fake.clone(),
            s.host().directory.join(JOURNAL_FILE),
            Timing::default(),
        );
        assert!(s.host().radeon.attach(WorkerRef::Owned(Arc::new(worker))));
        Self { s, fake }
    }
    fn ready(&self) {
        let until = Clock::now() + Duration::from_secs(15);
        while self.host().radeon.recovering() {
            assert!(Clock::now() < until, "the worker never finished recovering");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            self.host().radeon.status().unwrap().phase,
            Phase::Ready,
            "the startup recovery failed"
        );
    }
    /// The journal on disk.
    fn journal(&self) -> Vec<Entry> {
        Journal::load(&self.host().directory.join(JOURNAL_FILE))
            .unwrap()
            .entries
    }
    /// Every restore the session machine handed on has finished.
    fn flush(&self) {
        assert!(self.host().radeon.flush(Duration::from_secs(15)));
    }
    async fn panel(&self) -> api::Radeon {
        radeon(&self.get("/radeon").await)
    }
    /// A write of `changes` to the GPU.
    async fn write(&self, changes: serde_json::Value) -> Reply {
        self.write_body(&json!({"gpu": GPU, "changes": changes}).to_string())
            .await
    }
    async fn write_body(&self, body: &str) -> Reply {
        post_json(&self.s.f, &self.s.connection, "/radeon/settings", body).await
    }
    /// Host control, and a game being streamed.
    fn owner(&self) -> Arc<Session> {
        self.host_control(true);
        self.streaming()
    }
    /// An owner who changed `pacing.limit` from 140 to 200.
    async fn changed(&self) -> Arc<Session> {
        let stream = self.owner();
        let panel = radeon(
            &self
                .write(json!([{"id": "pacing.limit", "value": 200}]))
                .await,
        );
        assert_eq!(
            setting(&panel, "pacing.limit").value,
            Some(Api::Integer(200))
        );
        assert_eq!(self.fake.value("pacing.limit"), Raw::Int(200));
        assert_eq!(self.journal().len(), 1);
        stream
    }
    /// The change was put back and the journal is empty.
    fn assert_restored(&self) {
        assert_eq!(self.fake.value("pacing.limit"), Raw::Int(140));
        assert_eq!(self.journal(), vec![]);
    }
    /// The change is still there and still journaled.
    fn assert_kept(&self) {
        assert_eq!(self.fake.value("pacing.limit"), Raw::Int(200));
        assert_eq!(self.journal().len(), 1);
    }
}

async fn post_json(f: &Fixture, connection: &Connection, path: &str, body: &str) -> Reply {
    send(
        f,
        true,
        connection,
        "POST",
        &format!("{PREFIX}{path}"),
        Some("application/json"),
        body,
    )
    .await
}
fn radeon(reply: &Reply) -> api::Radeon {
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    assert_eq!(reply.headers[header::CACHE_CONTROL], "no-store");
    serde_json::from_slice(&reply.body).unwrap()
}
fn setting<'a>(panel: &'a api::Radeon, id: &str) -> &'a api::Setting {
    panel.gpus[0]
        .settings
        .iter()
        .find(|setting| setting.id == id)
        .unwrap_or_else(|| panic!("the panel has no {id}"))
}
/// The error names this setting.
fn assert_names(reply: &Reply, id: &str) {
    assert_eq!(reply.json()["error"]["setting"], id, "{}", reply.text());
}

#[tokio::test]
async fn each_setting_shows_one_of_the_five_states() {
    let rig = Rig::new();
    let panel = rig.panel().await;
    assert_eq!(panel.state, ControlState::Configurable);
    assert_eq!((panel.reason.clone(), panel.message.clone()), (None, None));
    assert!(panel.revision > 0);
    let gpu = &panel.gpus[0];
    assert_eq!(
        (gpu.id, gpu.name.as_str(), gpu.driver.as_str()),
        (GPU, "AMD Radeon RX 7900 XT", "Adrenalin 26.2.1")
    );
    let pacing = setting(&panel, "pacing.limit");
    assert_eq!(
        (pacing.kind, pacing.state, pacing.value.clone()),
        (
            ApiKind::Integer,
            ControlState::Configurable,
            Some(Api::Integer(140))
        )
    );
    assert_eq!(
        pacing.range,
        Some(api::Range {
            min: 30,
            max: 300,
            step: 10
        })
    );
    assert_eq!(pacing.unit.as_deref(), Some("fps"));
    assert_eq!(pacing.restore, None);
    let anti_lag = setting(&panel, "anti_lag.enabled");
    assert_eq!(
        (anti_lag.kind, anti_lag.value.clone()),
        (ApiKind::Toggle, Some(Api::Bool(false)))
    );
    let tessellation = setting(&panel, "tessellation.mode");
    assert_eq!(tessellation.kind, ApiKind::Choice);
    assert_eq!(
        tessellation.options,
        [
            ("a", "A"),
            ("on_unless_app_specifies", "On unless app specifies"),
            ("c", "C")
        ]
        .map(|(value, label)| api::Choice {
            value: value.into(),
            label: label.into()
        })
    );
    // Read only: the value shows, the reason says why it cannot change.
    let read_only = setting(&panel, "rsr.enabled");
    assert_eq!(read_only.state, ControlState::ReadOnly);
    assert_eq!(read_only.value, Some(Api::Bool(false)));
    assert_eq!(read_only.reason.as_deref(), Some("read_only"));
    // Needs a one-time setup in Adrenalin.
    let external = setting(&panel, "sam.enabled");
    assert_eq!(external.state, ControlState::NeedsExternalSetup);
    assert_eq!(external.reason.as_deref(), Some("external_setup"));
    assert!(
        external
            .message
            .as_deref()
            .unwrap()
            .contains("AMD Software")
    );
    // Unavailable and unsupported settings show nothing a client could take for a value.
    let busy = setting(&panel, "chill.min_fps");
    assert_eq!(busy.state, ControlState::TemporarilyUnavailable);
    assert_eq!(busy.reason.as_deref(), Some("driver_busy"));
    assert_eq!(
        (busy.value.clone(), busy.range, busy.unit.clone()),
        (None, None, None)
    );
    let unsupported = setting(&panel, "boost.enabled");
    assert_eq!(unsupported.state, ControlState::Unsupported);
    assert_eq!(unsupported.value, None);
}

#[tokio::test]
async fn the_panel_takes_the_most_useful_state_any_setting_has() {
    let cases = [
        (
            vec![toggle("anti_lag.enabled", State::Configurable, true)],
            ControlState::Configurable,
        ),
        (
            vec![toggle("rsr.enabled", State::ReadOnly, true)],
            ControlState::ReadOnly,
        ),
        (
            vec![toggle("sam.enabled", State::NeedsExternalSetup, true)],
            ControlState::NeedsExternalSetup,
        ),
        (
            vec![integer("chill.min_fps", State::TemporarilyUnavailable, 60)],
            ControlState::TemporarilyUnavailable,
        ),
        (
            vec![toggle("boost.enabled", State::Unsupported, true)],
            ControlState::Unsupported,
        ),
        (vec![], ControlState::Unsupported),
        (
            vec![
                toggle("boost.enabled", State::Unsupported, true),
                toggle("sam.enabled", State::NeedsExternalSetup, true),
                toggle("rsr.enabled", State::ReadOnly, true),
            ],
            ControlState::ReadOnly,
        ),
    ];
    for (settings, expected) in cases {
        let rig = Rig::with(only(settings));
        let panel = rig.panel().await;
        assert_eq!(panel.state, expected);
        assert_eq!(panel.gpus.len(), 1);
    }
}

#[tokio::test]
async fn a_host_without_radeon_controls_or_a_driver_says_so() {
    // No worker: the host did not start the Radeon layer.
    let s = Setup::new();
    let panel = radeon(&s.get("/radeon").await);
    assert_eq!(panel.state, ControlState::Unsupported);
    assert_eq!(panel.reason.as_deref(), Some("no_radeon"));
    assert!(panel.gpus.is_empty());
    s.host_control(true);
    let _stream = s.streaming();
    let reply = post_json(
        &s.f,
        &s.connection,
        "/radeon/settings",
        r#"{"gpu":1,"changes":[{"id":"pacing.limit","value":200}]}"#,
    )
    .await;
    reply.assert_error(404, "not_found");
    // ADLX lists no AMD GPU.
    let fake = panel_fake_without_gpu();
    let rig = Rig::with(fake);
    let panel = rig.panel().await;
    assert_eq!(panel.state, ControlState::Unsupported);
    assert_eq!(panel.reason.as_deref(), Some("no_amd_gpu"));
    assert!(panel.gpus.is_empty());
    // ADLX fails: a retry hint, not an empty panel.
    let fake = driver();
    fake.with(|d| d.unreadable = true);
    let rig = Rig::with(fake.clone());
    let panel = rig.panel().await;
    assert_eq!(panel.state, ControlState::TemporarilyUnavailable);
    assert_eq!(panel.reason.as_deref(), Some("adlx_unavailable"));
    assert!(panel.message.unwrap().contains("ADLX_FAIL"));
    // It comes back without a restart.
    fake.with(|d| d.unreadable = false);
    rig.host().radeon.forget();
    assert_eq!(rig.panel().await.state, ControlState::Configurable);
}
fn panel_fake_without_gpu() -> Fake {
    let fake = driver();
    fake.with(|d| d.gpus.clear());
    fake
}

#[tokio::test]
async fn capabilities_list_the_radeon_features_only_with_a_worker() {
    let without = Setup::new();
    let features = |reply: &Reply| {
        serde_json::from_slice::<Capabilities>(&reply.body)
            .unwrap()
            .features
    };
    assert!(
        !features(&without.get("/capabilities").await)
            .iter()
            .any(|feature| feature.starts_with("radeon"))
    );
    let rig = Rig::new();
    assert_eq!(
        features(&rig.get("/capabilities").await),
        [
            "library",
            "session",
            "session.close_game",
            "radeon.read",
            "radeon.write"
        ]
    );
}

#[tokio::test]
async fn reading_needs_view_or_launch_and_is_cached_for_a_second() {
    let s = Setup::with(LIST);
    let rig = Rig::start(s, driver());
    rig.ready();
    rig.get("/radeon")
        .await
        .assert_error(403, "permission_required");
    for perm in [VIEW_BIT, LAUNCH] {
        rig.host().paired.write().unwrap().clients[0].perm = perm;
        let panel = rig.panel().await;
        // Without Host control the panel can be read and never written.
        assert!(!panel.host_control && !panel.writable_now);
    }
    // A burst of reads is one driver read.
    rig.host().radeon.forget();
    let first = rig.panel().await;
    let reads = rig.fake.reads();
    let second = rig.panel().await;
    assert_eq!(rig.fake.reads(), reads);
    assert_eq!(first, second);
    // A change on the PC shows after the cache is gone, and moves the revision. A read that
    // finds the same thing does not.
    rig.host().radeon.forget();
    assert_eq!(rig.panel().await.revision, first.revision);
    rig.fake.set("pacing.limit", Raw::Int(150));
    rig.host().radeon.forget();
    let third = rig.panel().await;
    assert!(third.revision > first.revision);
    assert_eq!(
        setting(&third, "pacing.limit").value,
        Some(Api::Integer(150))
    );
}

#[tokio::test]
async fn a_write_needs_host_control_and_the_owner_of_a_session() {
    let rig = Rig::new();
    let change = json!([{"id": "pacing.limit", "value": 200}]);
    // Host control first, even for a body that is no good.
    let _stream = rig.streaming();
    rig.write(change.clone())
        .await
        .assert_error(403, "host_control_required");
    rig.write_body("{")
        .await
        .assert_error(403, "host_control_required");
    rig.post("/radeon/restore")
        .await
        .assert_error(403, "host_control_required");
    rig.host_control(true);
    // Not the owner.
    let other = other_device(&rig.f, ALL, true);
    let body = json!({"gpu": GPU, "changes": change}).to_string();
    post_json(&rig.f, &other, "/radeon/settings", &body)
        .await
        .assert_error(409, "not_session_owner");
    api(&rig.f, &other, "POST", "/radeon/restore")
        .await
        .assert_error(409, "not_session_owner");
    // A session being torn down.
    let teardown = rig
        .host()
        .session_teardown(crate::session_control::Scope::Session);
    rig.write(change.clone())
        .await
        .assert_error(409, "session_busy");
    drop(teardown);
    assert_eq!(rig.fake.writes(), Vec::<String>::new());
    assert_eq!(rig.journal(), vec![]);
    // No session at all.
    assert_eq!(
        rig.post("/session/end").await.session().state,
        SessionState::Idle
    );
    rig.write(change).await.assert_error(409, "no_session");
    rig.post("/radeon/restore")
        .await
        .assert_error(409, "no_session");
    assert_eq!(rig.fake.writes(), Vec::<String>::new());
    assert_eq!(rig.journal(), vec![]);
}

#[tokio::test]
async fn a_write_changes_the_driver_and_answers_the_read_back_panel() {
    let rig = Rig::new();
    let _stream = rig.owner();
    let reply = rig
        .write(json!([
            {"id": "pacing.limit", "value": 200},
            {"id": "anti_lag.enabled", "value": true},
            {"id": "tessellation.mode", "value": "c"},
        ]))
        .await;
    let panel = radeon(&reply);
    assert_eq!(rig.fake.value("pacing.limit"), Raw::Int(200));
    assert_eq!(rig.fake.value("anti_lag.enabled"), Raw::Bool(true));
    assert_eq!(rig.fake.value("tessellation.mode"), Raw::Choice("c".into()));
    assert!(panel.host_control && panel.writable_now);
    let pacing = setting(&panel, "pacing.limit");
    assert_eq!(pacing.value, Some(Api::Integer(200)));
    // The panel shows what goes back at the end of the session.
    assert_eq!(
        pacing.restore,
        Some(ApiRestore {
            original: Api::Integer(140),
            written: Api::Integer(200)
        })
    );
    assert_eq!(
        setting(&panel, "anti_lag.enabled").restore,
        Some(ApiRestore {
            original: Api::Bool(false),
            written: Api::Bool(true)
        })
    );
    assert_eq!(setting(&panel, "rsr.enabled").restore, None);
    // The journal names the device whose session it is.
    let journal = rig.journal();
    assert_eq!(journal.len(), 3);
    assert!(journal.iter().all(|entry| entry.session == "parity-device"));
    assert!(journal.iter().all(|entry| entry.gpu == GPU_ID as i32));
    // A later read shows the same.
    assert_eq!(rig.panel().await.gpus, panel.gpus);
}

#[tokio::test]
async fn a_suspended_session_may_still_write_and_a_stranger_may_not_see_it_writable() {
    let rig = Rig::new();
    let stream = rig.owner();
    stream.stop();
    assert_eq!(rig.state(), SessionState::Suspended);
    assert!(rig.panel().await.writable_now);
    radeon(
        &rig.write(json!([{"id": "pacing.limit", "value": 210}]))
            .await,
    );
    assert_eq!(rig.fake.value("pacing.limit"), Raw::Int(210));
    // Another device with Host control reads it, and cannot write.
    let other = other_device(&rig.f, ALL, true);
    let panel = radeon(&api(&rig.f, &other, "GET", "/radeon").await);
    assert!(panel.host_control && !panel.writable_now);
}

#[tokio::test]
async fn writable_now_needs_host_control_a_session_and_ownership() {
    let rig = Rig::new();
    // Nothing is running.
    rig.host_control(true);
    let panel = rig.panel().await;
    assert!(panel.host_control && !panel.writable_now);
    let _stream = rig.streaming();
    assert!(rig.panel().await.writable_now);
    rig.host_control(false);
    let panel = rig.panel().await;
    assert!(!panel.host_control && !panel.writable_now);
    rig.host_control(true);
    let other = other_device(&rig.f, ALL, true);
    assert!(!radeon(&api(&rig.f, &other, "GET", "/radeon").await).writable_now);
    let teardown = rig
        .host()
        .session_teardown(crate::session_control::Scope::Session);
    assert!(!rig.panel().await.writable_now);
    drop(teardown);
}

#[tokio::test]
async fn bad_values_are_422_naming_the_setting_and_change_nothing() {
    let rig = Rig::new();
    let _stream = rig.owner();
    let cases = [
        // Type.
        ("anti_lag.enabled", json!(1), "invalid_value"),
        ("anti_lag.enabled", json!("on"), "invalid_value"),
        ("pacing.limit", json!(true), "invalid_value"),
        ("pacing.limit", json!("200"), "invalid_value"),
        ("pacing.limit", json!(150.5), "invalid_value"),
        ("tessellation.mode", json!(1), "invalid_value"),
        ("pacing.limit", json!(null), "invalid_value"),
        ("pacing.limit", json!([200]), "invalid_value"),
        ("pacing.limit", json!({"v": 200}), "invalid_value"),
        // Range.
        ("pacing.limit", json!(20), "invalid_value"),
        ("pacing.limit", json!(310), "invalid_value"),
        // Step.
        ("pacing.limit", json!(205), "invalid_value"),
        // Not an option.
        ("tessellation.mode", json!("nope"), "invalid_value"),
        // Not configurable.
        ("rsr.enabled", json!(true), "not_writable"),
        ("sam.enabled", json!(true), "not_writable"),
        ("chill.min_fps", json!(100), "not_writable"),
        ("boost.enabled", json!(true), "not_writable"),
    ];
    for (id, value, code) in cases {
        let reply = rig.write(json!([{"id": id, "value": value}])).await;
        reply.assert_error(422, code);
        assert_names(&reply, id);
    }
    // A setting listed twice.
    let reply = rig
        .write(json!([
            {"id": "pacing.limit", "value": 200},
            {"id": "pacing.limit", "value": 210},
        ]))
        .await;
    reply.assert_error(422, "invalid_value");
    assert_names(&reply, "pacing.limit");
    // One bad value spoils the whole request: the good one is not applied.
    let reply = rig
        .write(json!([
            {"id": "anti_lag.enabled", "value": true},
            {"id": "pacing.limit", "value": 205},
        ]))
        .await;
    reply.assert_error(422, "invalid_value");
    assert_names(&reply, "pacing.limit");
    assert_eq!(rig.fake.writes(), Vec::<String>::new());
    assert_eq!(rig.journal(), vec![]);
    assert_eq!(rig.fake.value("anti_lag.enabled"), Raw::Bool(false));
}

#[tokio::test]
async fn an_unknown_gpu_or_setting_is_404_and_a_bad_body_is_400() {
    let rig = Rig::new();
    let _stream = rig.owner();
    let unknown_gpu = json!({"gpu": 7, "changes": [{"id": "pacing.limit", "value": 200}]});
    rig.write_body(&unknown_gpu.to_string())
        .await
        .assert_error(404, "not_found");
    // Above the 32 bits ADLX has.
    let huge = json!({"gpu": 1u64 << 40, "changes": [{"id": "pacing.limit", "value": 200}]});
    rig.write_body(&huge.to_string())
        .await
        .assert_error(404, "not_found");
    let reply = rig
        .write(json!([{"id": "nope.enabled", "value": true}]))
        .await;
    reply.assert_error(404, "not_found");
    assert_names(&reply, "nope.enabled");
    for body in [
        "{",
        "[]",
        "{}",
        r#"{"gpu":-1,"changes":[{"id":"pacing.limit","value":200}]}"#,
        r#"{"gpu":"x","changes":[]}"#,
        r#"{"gpu":3221225473}"#,
        r#"{"gpu":3221225473,"changes":[]}"#,
        r#"{"gpu":3221225473,"changes":[{"id":"pacing.limit"}]}"#,
        r#"{"gpu":3221225473,"changes":[{"id":"pacing.limit","value":200,"also":1}]}"#,
        r#"{"gpu":3221225473,"changes":[{"id":"pacing.limit","value":200}],"also":1}"#,
    ] {
        rig.write_body(body)
            .await
            .assert_error(400, "invalid_request");
    }
    assert_eq!(rig.fake.writes(), Vec::<String>::new());
    assert_eq!(rig.journal(), vec![]);
    // The restore endpoint takes no body: nothing, or a JSON object it ignores like the session
    // actions do.
    for body in ["[]", "{", "1"] {
        post_json(&rig.f, &rig.connection, "/radeon/restore", body)
            .await
            .assert_error(400, "invalid_request");
    }
    radeon(&post_json(&rig.f, &rig.connection, "/radeon/restore", "{}").await);
}

#[tokio::test]
async fn a_driver_that_refuses_a_write_is_409_driver_rejected_with_everything_rolled_back() {
    let rig = Rig::new();
    let _stream = rig.owner();
    rig.fake.with(|d| d.refuse = Some("tessellation.mode"));
    let reply = rig
        .write(json!([
            {"id": "pacing.limit", "value": 200},
            {"id": "anti_lag.enabled", "value": true},
            {"id": "tessellation.mode", "value": "c"},
        ]))
        .await;
    reply.assert_error(409, "driver_rejected");
    assert_names(&reply, "tessellation.mode");
    // The first two were written, then put back.
    assert_eq!(rig.fake.value("pacing.limit"), Raw::Int(140));
    assert_eq!(rig.fake.value("anti_lag.enabled"), Raw::Bool(false));
    assert!(rig.fake.writes().contains(&"pacing.limit=200".to_string()));
    assert_eq!(rig.journal(), vec![]);
    assert!(
        rig.panel().await.gpus[0]
            .settings
            .iter()
            .all(|setting| setting.restore.is_none())
    );

    // A write the driver accepts and does not keep is just as rejected.
    rig.fake.with(|d| {
        d.refuse = None;
        d.ignore = Some("pacing.limit");
    });
    let reply = rig
        .write(json!([{"id": "pacing.limit", "value": 200}]))
        .await;
    reply.assert_error(409, "driver_rejected");
    assert_names(&reply, "pacing.limit");
    assert!(
        reply.json()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("reads back 140")
    );
    assert_eq!(rig.fake.value("pacing.limit"), Raw::Int(140));
    assert_eq!(rig.journal(), vec![]);
}

#[tokio::test]
async fn a_driver_that_cannot_be_read_is_503_with_a_retry_hint() {
    let rig = Rig::new();
    let _stream = rig.owner();
    rig.fake.with(|d| d.unreadable = true);
    rig.host().radeon.forget();
    let reply = rig
        .write(json!([{"id": "pacing.limit", "value": 200}]))
        .await;
    reply.assert_error(503, "temporarily_unavailable");
    assert_eq!(reply.json()["error"]["retry_after_ms"], 5000);
    assert_eq!(rig.journal(), vec![]);
}

#[tokio::test]
async fn end_session_puts_the_changes_back() {
    let rig = Rig::new();
    let stream = rig.changed().await;
    let ended = rig.post("/session/end").await.session();
    assert_eq!(ended.state, SessionState::Idle);
    rig.flush();
    rig.assert_restored();
    assert!(stream.stopping());
    // The panel no longer shows a pending change.
    rig.host().radeon.forget();
    assert_eq!(setting(&rig.panel().await, "pacing.limit").restore, None);
}

#[tokio::test]
async fn close_game_puts_the_changes_back() {
    let rig = Rig::new();
    let _stream = rig.changed().await;
    let closed = rig.post("/session/close-game").await.session();
    assert_eq!(closed.state, SessionState::Idle);
    rig.flush();
    rig.assert_restored();
}

#[tokio::test]
async fn disconnect_keeps_the_changes_through_the_grace_period() {
    let rig = Rig::new();
    let _stream = rig.changed().await;
    let suspended = rig.post("/session/disconnect").await.session();
    assert_eq!(suspended.state, SessionState::Suspended);
    rig.flush();
    rig.assert_kept();
    // Still the owner's, still shown, still writable.
    let panel = rig.panel().await;
    assert!(panel.writable_now);
    assert_eq!(
        setting(&panel, "pacing.limit").restore,
        Some(ApiRestore {
            original: Api::Integer(140),
            written: Api::Integer(200)
        })
    );
    // The same for a repeated Disconnect and a session whose stream ended by itself.
    rig.post("/session/disconnect").await.session();
    rig.flush();
    rig.assert_kept();
    // The grace period then ends the session and the change goes back (next test).
}

#[tokio::test]
async fn the_grace_period_running_out_puts_the_changes_back() {
    let rig = Rig::new();
    *rig.host().config.write().unwrap() = Config::parse("dd_config_revert_delay=0").unwrap();
    let stream = rig.changed().await;
    stream.stop();
    assert_eq!(rig.state(), SessionState::Suspended);
    rig.flush();
    rig.assert_kept();
    rig.host().reap_paused_display();
    assert_eq!(rig.state(), SessionState::Idle);
    rig.flush();
    rig.assert_restored();
}

#[tokio::test]
async fn unpairing_the_owner_puts_the_changes_back() {
    let rig = Rig::new();
    let _stream = rig.changed().await;
    let reply = send(
        &rig.f,
        true,
        &rig.connection,
        "POST",
        "/unpair",
        None,
        vec![],
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK);
    rig.flush();
    rig.assert_restored();
}

#[tokio::test]
async fn a_host_shutdown_puts_the_changes_back_and_waits_for_it() {
    let rig = Rig::new();
    let _stream = rig.changed().await;
    // The stream is still running: a shutdown ends it.
    rig.host().radeon.shutdown(Duration::from_secs(15));
    rig.assert_restored();
}

#[tokio::test]
async fn a_new_stream_of_the_same_device_after_end_starts_clean() {
    let rig = Rig::new();
    let _first = rig.changed().await;
    rig.post("/session/end").await.session();
    rig.flush();
    rig.assert_restored();
    // The next session writes again, and its end restores again.
    let _second = rig.streaming();
    radeon(
        &rig.write(json!([{"id": "pacing.limit", "value": 210}]))
            .await,
    );
    assert_eq!(rig.fake.value("pacing.limit"), Raw::Int(210));
    rig.post("/session/end").await.session();
    rig.flush();
    rig.assert_restored();
}

#[tokio::test]
async fn a_setting_changed_on_the_pc_is_not_put_back() {
    let rig = Rig::new();
    let _stream = rig.changed().await;
    // Someone moved the slider in Adrenalin during the session.
    rig.fake.set("pacing.limit", Raw::Int(150));
    rig.post("/session/end").await.session();
    rig.flush();
    assert_eq!(rig.fake.value("pacing.limit"), Raw::Int(150));
    assert_eq!(rig.journal(), vec![]);
    assert_eq!(rig.fake.writes(), vec!["pacing.limit=200".to_string()]);
}

#[tokio::test]
async fn a_failed_restore_is_reported_stays_journaled_and_the_restore_endpoint_retries_it() {
    let rig = Rig::new();
    let _stream = rig.changed().await;
    rig.fake.with(|d| d.refuse_all = true);
    let reply = rig.post("/radeon/restore").await;
    reply.assert_error(503, "temporarily_unavailable");
    assert_eq!(reply.json()["error"]["retry_after_ms"], 60_000);
    assert!(
        reply.json()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("pacing.limit")
    );
    rig.assert_kept();
    // The panel tells a client the change is still there and why.
    let panel = rig.panel().await;
    assert_eq!(panel.reason.as_deref(), Some("restore_failed"));
    assert!(panel.message.is_some());
    let pacing = setting(&panel, "pacing.limit");
    assert_eq!(pacing.reason.as_deref(), Some("restore_failed"));
    assert!(pacing.message.as_deref().unwrap().contains("140"));
    assert_eq!(
        pacing.restore,
        Some(ApiRestore {
            original: Api::Integer(140),
            written: Api::Integer(200)
        })
    );
    // The driver recovers; the owner asks again.
    rig.fake.with(|d| d.refuse_all = false);
    let panel = radeon(&rig.post("/radeon/restore").await);
    rig.assert_restored();
    assert_eq!(panel.reason, None);
    assert_eq!(panel.message, None);
    assert_eq!(setting(&panel, "pacing.limit").restore, None);
    assert_eq!(
        setting(&panel, "pacing.limit").value,
        Some(Api::Integer(140))
    );
    assert!(panel.writable_now, "the session goes on");
}

#[tokio::test]
async fn a_restore_that_fails_at_the_end_of_a_session_stays_journaled() {
    let rig = Rig::new();
    let _stream = rig.changed().await;
    rig.fake.with(|d| d.refuse_all = true);
    rig.post("/session/end").await.session();
    rig.flush();
    rig.assert_kept();
    let panel = rig.panel().await;
    assert_eq!(panel.reason.as_deref(), Some("restore_failed"));
    assert_eq!(
        setting(&panel, "pacing.limit")
            .restore
            .as_ref()
            .map(|r| &r.original),
        Some(&Api::Integer(140))
    );
    // Nobody owns a session now, so nothing is writable, and the host still holds the entry for
    // its next attempt.
    assert!(!panel.writable_now);
    rig.fake.with(|d| d.refuse_all = false);
    let worker = rig.host().radeon.worker().unwrap();
    assert!(worker.restore_session("parity-device").unwrap().is_clean());
    rig.assert_restored();
}

#[tokio::test]
async fn the_restore_endpoint_puts_the_changes_back_and_the_session_goes_on() {
    let rig = Rig::new();
    let stream = rig.changed().await;
    let panel = radeon(&rig.post("/radeon/restore").await);
    rig.assert_restored();
    assert_eq!(setting(&panel, "pacing.limit").restore, None);
    assert!(panel.writable_now);
    assert!(!stream.stopping());
    assert_eq!(rig.state(), SessionState::Streaming);
    // Nothing to put back is fine.
    radeon(&rig.post("/radeon/restore").await);
    // A write after a restore is put back at the end of the session again.
    radeon(
        &rig.write(json!([{"id": "pacing.limit", "value": 220}]))
            .await,
    );
    rig.post("/session/end").await.session();
    rig.flush();
    rig.assert_restored();
}

#[tokio::test]
async fn a_journal_from_a_crashed_run_is_restored_when_the_host_starts() {
    let s = Setup::new();
    let fake = driver();
    // The crash left the driver at 200 and the journal holding 140.
    fake.set("pacing.limit", Raw::Int(200));
    let journal = Journal {
        entries: vec![Entry {
            gpu: GPU_ID as i32,
            id: "pacing.limit".into(),
            session: "parity-device".into(),
            original: Raw::Int(140),
            written: Raw::Int(200),
            side_effect: false,
            user: butterpollo_windows::process::user_sid().unwrap(),
        }],
        ..Default::default()
    };
    let path = s.host().directory.join(JOURNAL_FILE);
    journal.save(&path).unwrap();
    // The driver is slow to answer, so the recovery is visible.
    fake.with(|d| d.hold = true);
    let rig = Rig::start(s, fake.clone());
    let reply = rig.get("/radeon").await;
    let panel = radeon(&reply);
    assert_eq!(panel.state, ControlState::TemporarilyUnavailable);
    assert_eq!(panel.reason.as_deref(), Some("recovering"));
    assert!(panel.gpus.is_empty());
    rig.host_control(true);
    fake.with(|d| d.hold = false);
    rig.ready();
    assert_eq!(fake.value("pacing.limit"), Raw::Int(140));
    assert_eq!(rig.journal(), vec![]);
    rig.host().radeon.forget();
    let panel = rig.panel().await;
    assert_eq!(panel.state, ControlState::Configurable);
    assert_eq!(
        setting(&panel, "pacing.limit").value,
        Some(Api::Integer(140))
    );
    assert_eq!(setting(&panel, "pacing.limit").restore, None);
}
