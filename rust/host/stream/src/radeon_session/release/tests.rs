//! The forced Vertical sync of virtual-display streams, on a fake NVIDIA global profile.
use super::*;
mod session;
use crate::radeon_session::{Radeon, WorkerRef};
use butterpollo_win_graphics::{
    nvidia_settings::{self, Store},
    radeon::{BackendError, GpuInfo, RadeonBackend, Setting, worker::Worker},
};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

const VSYNCMODE: u32 = 0x00A8_79CF;
const ON: u32 = 0x4781_4940;
const OFF: u32 = 0x0841_6747;
/// What the driver does when the profile stores nothing.
const APPLICATION: u32 = 0x6092_5292;
const WAIT: Duration = Duration::from_secs(10);

#[derive(Clone, Default)]
struct Profile {
    values: Arc<Mutex<BTreeMap<u32, u32>>>,
    missing: Arc<AtomicBool>,
    /// Writes fail, as a restore that cannot be put back.
    read_only: Arc<AtomicBool>,
}
impl Profile {
    fn vsync(&self) -> u32 {
        let values = self.values.lock().unwrap();
        values.get(&VSYNCMODE).copied().unwrap_or(APPLICATION)
    }
}
impl Store for Profile {
    fn get(&mut self, id: u32) -> anyhow::Result<Option<u32>> {
        Ok(self.values.lock().unwrap().get(&id).copied())
    }
    fn set(&mut self, id: u32, value: u32) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.read_only.load(Ordering::SeqCst),
            "the profile is read-only"
        );
        self.values.lock().unwrap().insert(id, value);
        Ok(())
    }
    fn delete(&mut self, id: u32) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.read_only.load(Ordering::SeqCst),
            "the profile is read-only"
        );
        self.values.lock().unwrap().remove(&id);
        Ok(())
    }
    fn save(&mut self) -> anyhow::Result<()> {
        Ok(())
    }
    fn driver(&mut self) -> Option<u32> {
        Some(59000)
    }
}
impl RadeonBackend for Profile {
    fn read(&mut self, _gpu: i32) -> Result<Vec<Setting>, BackendError> {
        nvidia_settings::read(self).map_err(|e| BackendError(e.to_string()))
    }
    fn write(&mut self, _gpu: i32, id: &str, value: &model::Value) -> Result<(), BackendError> {
        nvidia_settings::write(self, id, value)
    }
    fn snapshot_keys(
        &mut self,
        _gpu: i32,
        id: &str,
    ) -> Result<Option<butterpollo_win_graphics::radeon::RawSettingKeys>, BackendError> {
        nvidia_settings::snapshot_keys(self, id)
    }
    fn restore_raw(
        &mut self,
        entry: &butterpollo_win_graphics::radeon::Entry,
    ) -> Result<bool, BackendError> {
        nvidia_settings::restore_raw_keys(self, entry)
    }
    fn gpus(&mut self) -> Result<Vec<GpuInfo>, BackendError> {
        if self.missing.load(Ordering::SeqCst) {
            return Err(BackendError("nvapi64.dll is missing".into()));
        }
        Ok(vec![GpuInfo {
            id: 0,
            name: "NVIDIA".into(),
            driver: "590.00".into(),
        }])
    }
}

/// An NVIDIA worker for `radeon` that writes to `profile`.
fn attach(radeon: &Radeon, profile: &Profile, directory: &std::path::Path) {
    radeon.driver.set(Driver::Nvidia).unwrap();
    let worker = Worker::start_machine(
        profile.clone(),
        directory.join("nvidia-journal.json"),
        Default::default(),
    );
    assert!(radeon.attach(WorkerRef::Owned(Arc::new(worker))));
}

struct Host {
    radeon: Radeon,
    profile: Profile,
    directory: PathBuf,
}
impl Host {
    fn new(profile: Profile) -> Self {
        let directory = std::env::temp_dir().join(format!("rl-vsync-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let radeon = Radeon::default();
        attach(&radeon, &profile, &directory);
        Self {
            radeon,
            profile,
            directory,
        }
    }
    fn launch(&self, device: &str, wanted: bool) -> Result<(), String> {
        let (hold, result) = self.radeon.hold_vsync(device, "", wanted, |_| false);
        hold.keep();
        result
    }
    fn settle(&self, present: &[&str]) {
        self.radeon.sync(BTreeSet::new, || {
            Some(present.iter().map(|d| d.to_string()).collect())
        });
        assert!(self.radeon.flush(WAIT));
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

#[test]
fn forced_until_the_last_stream_ends() {
    let host = Host::new(Profile::default());
    host.launch("a", true).unwrap();
    assert_eq!(host.profile.vsync(), ON);
    host.launch("b", true).unwrap();
    host.settle(&["b"]);
    assert_eq!(host.profile.vsync(), ON);
    // A profile launch releasing other devices keeps what a present one holds.
    host.radeon
        .release_missing(BTreeSet::new, || ["b".to_string()].into(), WAIT);
    assert_eq!(host.profile.vsync(), ON);
    host.settle(&[]);
    assert_eq!(host.profile.vsync(), APPLICATION);
}

#[test]
fn a_failed_launch_puts_it_back() {
    let host = Host::new(Profile::default());
    let (hold, result) = host.radeon.hold_vsync("a", "", true, |_| false);
    result.unwrap();
    assert_eq!(host.profile.vsync(), ON);
    drop(hold);
    assert!(host.radeon.flush(WAIT));
    assert_eq!(host.profile.vsync(), APPLICATION);
}

#[test]
fn a_change_on_the_pc_stays_through_a_resume_and_the_end() {
    let host = Host::new(Profile::default());
    host.launch("a", true).unwrap();
    host.profile.clone().set(VSYNCMODE, OFF).unwrap();
    host.launch("a", true).unwrap();
    host.settle(&[]);
    assert_eq!(host.profile.vsync(), OFF);
}

#[test]
fn a_custom_value_and_an_unwanted_launch_change_nothing() {
    const CUSTOM: u32 = 0x3261_0244;
    let profile = Profile::default();
    profile.clone().set(VSYNCMODE, CUSTOM).unwrap();
    let host = Host::new(profile);
    host.launch("a", true).unwrap();
    assert_eq!(host.profile.vsync(), CUSTOM);
    host.profile.clone().delete(VSYNCMODE).unwrap();
    host.launch("a", false).unwrap();
    assert_eq!(host.profile.vsync(), APPLICATION);
    assert!(!host.radeon.holds(VSYNC_SESSION));
}

#[test]
fn a_missing_nvapi_is_reported() {
    let host = Host::new(Profile::default());
    host.profile.missing.store(true, Ordering::SeqCst);
    let error = host.launch("a", true).unwrap_err();
    assert!(error.contains("nvapi64.dll is missing"), "{error}");
}

/// A device's own write of Vertical sync, as its game's preset or its stream menu makes it.
fn choose(host: &Host, device: &str, value: &str) {
    let change = Change {
        id: VSYNC.into(),
        value: model::Value::Choice(value.into()),
    };
    host.radeon.apply(device, 0, vec![change]).unwrap();
}

#[test]
fn another_devices_value_is_not_forced_over_and_the_force_returns_after_it() {
    let host = Host::new(Profile::default());
    host.launch("b", true).unwrap();
    choose(&host, "a", "off");
    host.launch("b", true).unwrap();
    host.launch("c", true).unwrap();
    assert_eq!(host.profile.vsync(), OFF);
    // `a` ends: its value is put back and the streams still holding get the force again.
    host.settle(&["b", "c"]);
    assert_eq!(host.profile.vsync(), ON);
    host.settle(&[]);
    assert_eq!(host.profile.vsync(), APPLICATION);
}

#[test]
fn a_takeover_on_a_physical_display_does_not_force_for_the_holder_it_replaced() {
    let host = Host::new(Profile::default());
    host.launch("a", true).unwrap();
    choose(&host, "a", "off");
    host.radeon
        .release_missing(BTreeSet::new, || ["b".to_string()].into(), WAIT);
    assert!(host.radeon.flush(WAIT));
    assert_eq!(host.profile.vsync(), APPLICATION);
}

#[test]
fn a_change_on_the_pc_stays_through_another_devices_choice() {
    let host = Host::new(Profile::default());
    host.launch("b", true).unwrap();
    host.profile.clone().set(VSYNCMODE, OFF).unwrap();
    choose(&host, "c", "on");
    host.settle(&["b"]);
    assert_eq!(host.profile.vsync(), OFF);
}

#[test]
fn a_choice_in_the_stream_menu_stays_through_a_resume() {
    let host = Host::new(Profile::default());
    host.launch("a", true).unwrap();
    choose(&host, "a", "off");
    host.launch("a", true).unwrap();
    assert_eq!(host.profile.vsync(), OFF);
    host.settle(&[]);
    assert_eq!(host.profile.vsync(), APPLICATION);
}

#[test]
fn a_failed_recheck_keeps_the_hold_the_device_had() {
    let host = Host::new(Profile::default());
    host.launch("a", true).unwrap();
    host.profile.missing.store(true, Ordering::SeqCst);
    host.launch("a", true).unwrap_err();
    assert!(host.radeon.flush(WAIT));
    assert!(host.radeon.holds(VSYNC_SESSION));
    assert_eq!(host.profile.vsync(), ON);
}

#[test]
fn a_release_takes_the_hold_and_the_force_in_one_step() {
    let host = Host::new(Profile::default());
    host.launch("a", true).unwrap();
    std::thread::scope(|scope| {
        let written = locked(&host.radeon.written);
        let release = scope.spawn(|| host.radeon.release_vsync("a"));
        std::thread::sleep(Duration::from_millis(200));
        // Blocked on `written`, the release has not given up the hold yet.
        assert!(locked(&host.radeon.vsync_holders).contains("a"));
        drop(written);
        release.join().unwrap();
    });
    assert!(host.radeon.flush(WAIT));
    assert_eq!(host.profile.vsync(), APPLICATION);
}

#[test]
fn an_nvidia_host_without_its_worker_says_so() {
    let radeon = Radeon::default();
    let failed = Err(BackendError("the helper did not start".into()));
    crate::radeon_session::adopt(&radeon, Driver::Nvidia, failed).unwrap_err();
    let (_hold, result) = radeon.hold_vsync("a", "", true, |_| false);
    assert_eq!(
        result.unwrap_err(),
        "this host has no driver settings controls"
    );
}

#[test]
fn only_a_virtual_display_with_no_preset_value_wants_it() {
    let app = |settings: serde_json::Value| {
        let mut app = App::desktop();
        app.extra
            .insert(radeon_profile::PROFILE.into(), "custom".into());
        app.extra.insert(radeon_profile::SETTINGS.into(), settings);
        app
    };
    assert!(wants_vsync(true, true, None, None));
    assert!(!wants_vsync(false, true, None, None));
    assert!(!wants_vsync(true, false, None, None));
    let own = app(serde_json::json!({"nvidia_vsync.mode": "off"}));
    assert!(!wants_vsync(true, true, Some(&own), Some(60)));
    let other = app(serde_json::json!({"nvidia_power.mode": "adaptive"}));
    assert!(wants_vsync(true, true, Some(&other), Some(60)));
}

#[test]
fn a_relaunch_over_a_failed_restore_keeps_the_pcs_own_value_as_the_original() {
    let host = Host::new(Profile::default());
    host.launch("b", true).unwrap();
    host.profile.read_only.store(true, Ordering::SeqCst);
    host.settle(&[]);
    // The restore failed: the force's entry stays journaled with the PC's own value.
    assert_eq!(host.profile.vsync(), ON);
    host.profile.read_only.store(false, Ordering::SeqCst);
    host.launch("b", true).unwrap();
    choose(&host, "c", "off");
    // `c` ends and the PC's own value comes back: `b` still holds, so it is forced again.
    host.settle(&["b"]);
    assert_eq!(host.profile.vsync(), ON);
}

#[test]
fn a_takeover_of_a_value_that_could_not_be_put_back_asks_for_a_relaunch() {
    let host = Host::new(Profile::default());
    choose(&host, "a", "off");
    host.profile.read_only.store(true, Ordering::SeqCst);
    host.settle(&[]);
    host.profile.read_only.store(false, Ordering::SeqCst);
    let (hold, result) = host.radeon.hold_vsync("b", "", true, |owner| owner == "a");
    hold.keep();
    let warning = result.unwrap_err();
    assert!(warning.contains("relaunch the game"), "{warning}");
    assert_eq!(host.profile.vsync(), OFF);
}
