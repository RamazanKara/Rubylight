//! Setting the virtual display and gamepad drivers up again, and the record
//! of it, driver-repair.json in the profile. The service runs
//! `--repair-drivers --driver <name>...` when a driver has been missing for a
//! minute (rust/service/src/driver_watch.rs); a person runs it with `--now`,
//! and `--repair` (crate::repair) runs it for both drivers.
//!
//! The record keeps, per driver, the version and time of the last limited
//! attempt and whether the driver was left out at install. The display
//! driver's entry is the record's top level (`declined`, `version`, `unix`),
//! where setups before the gamepad driver had one wrote and read it; the
//! gamepad driver's is under `gamepad`. The service reads `declined`; keep
//! the two in step.
use super::{
    drivers::{display_step, driver_lock, gamepad_step},
    probe, profile,
    side_by_side::{ensure_idle_positive, serverinfo},
};
use crate::{log::line, system, ui::Progress};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{path::Path, time::Duration};

pub(super) const REPAIR_RECORD: &str = "driver-repair.json";
/// What the log tells a person when the service's repair was held back.
pub(super) const REPAIR_BY_HAND: &str = "To try again now, run \"butterpollo-setup.exe --repair-drivers --now\" from the Rubylight folder in an administrator shell.";

/// A driver setup sets up again, as the service names it (`--driver`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Driver {
    Display,
    Gamepad,
}
impl Driver {
    pub const ALL: [Self; 2] = [Self::Display, Self::Gamepad];
    /// The driver `--driver <name>` names.
    pub fn parse(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "display" => Some(Self::Display),
            "gamepad" => Some(Self::Gamepad),
            _ => None,
        }
    }
    /// Its name in notes and the log.
    pub fn name(self) -> &'static str {
        match self {
            Self::Display => "virtual display",
            Self::Gamepad => "virtual gamepad",
        }
    }
    /// The install option that leaves it out.
    fn left_out_by(self) -> &'static str {
        match self {
            Self::Display => "it was left out with --no-display-driver",
            Self::Gamepad => "it was left out with --no-gamepad-driver",
        }
    }
}
/// Who asks for a repair, which decides what holds it back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepairRun {
    /// The service's plain `--repair-drivers`: each driver at most once a day
    /// per version, never one left out at install, and only when the host
    /// says it is idle.
    Service,
    /// `--repair-drivers --now`: a person asks for these drivers, so neither
    /// the daily limit nor the choice made at install applies.
    ByHand,
    /// `--repair`: a person repairs Rubylight. No daily limit, but a driver
    /// left out at install stays out.
    Repair,
}
impl RepairRun {
    /// A person waits for the result: a refusal is an error, and a host that
    /// hides its counts (a LAN bind_address) does not stop the repair.
    fn by_hand(self) -> bool {
        self != Self::Service
    }
}

/// The record as read: an unreadable or missing one is empty, so it holds
/// nothing back.
pub(super) fn read_record(bytes: Option<&[u8]>) -> Value {
    bytes
        .and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}))
}
fn entry(record: &Value, driver: Driver) -> &Value {
    match driver {
        Driver::Display => record,
        Driver::Gamepad => &record["gamepad"],
    }
}
/// Why `run` does not set `driver` up again now, if it does not.
pub(super) fn repair_skipped(
    record: &Value,
    driver: Driver,
    version: &str,
    now: u64,
    run: RepairRun,
) -> Option<&'static str> {
    if run == RepairRun::ByHand {
        return None;
    }
    let entry = entry(record, driver);
    if entry["declined"] == true {
        return Some(driver.left_out_by());
    }
    (run == RepairRun::Service
        && entry["version"] == version
        && entry["unix"]
            .as_u64()
            .is_some_and(|at| now.saturating_sub(at) < 86_400))
    .then_some("this version already tried today")
}
/// The record once `driver` is set up again at `now`: its entry holds this
/// attempt and no longer a choice to leave it out; the other's stays.
pub(super) fn record_attempt(record: Value, driver: Driver, version: &str, now: u64) -> Value {
    let mut record = if record.is_object() {
        record
    } else {
        json!({})
    };
    match driver {
        Driver::Display => {
            if let Some(fields) = record.as_object_mut() {
                fields.remove("declined");
                fields.insert("version".into(), json!(version));
                fields.insert("unix".into(), json!(now));
            }
        }
        Driver::Gamepad => record["gamepad"] = json!({"version": version, "unix": now}),
    }
    record
}
/// What a new installation records: only the drivers it left out. None when
/// it installed both, so the record goes and both may be repaired again.
pub(super) fn choices(display_driver: bool, gamepad_driver: bool) -> Option<Value> {
    let mut record = json!({});
    if !display_driver {
        record["declined"] = json!(true);
    }
    if !gamepad_driver {
        record["gamepad"] = json!({"declined": true});
    }
    (record != json!({})).then_some(record)
}
/// A new installation may repair the drivers again, unless it was installed
/// with --no-display-driver or --no-gamepad-driver.
pub(super) fn remember_driver_choice(profile: &Path, display_driver: bool, gamepad_driver: bool) {
    let record = profile.join(REPAIR_RECORD);
    let _ = match choices(display_driver, gamepad_driver) {
        None => std::fs::remove_file(&record),
        Some(choices) => std::fs::create_dir_all(profile)
            .and_then(|()| std::fs::write(&record, serde_json::to_vec(&choices)?)),
    };
}
/// Whether setup ran with --no-display-driver.
pub(crate) fn display_driver_declined() -> bool {
    let record = read_record(std::fs::read(profile().join(REPAIR_RECORD)).ok().as_deref());
    entry(&record, Driver::Display)["declined"] == true
}

/// What a driver repair did.
#[derive(Debug, Default)]
pub struct DriverRepair {
    /// Windows must restart to finish.
    pub restart: bool,
    /// The drivers set up again.
    pub repaired: Vec<Driver>,
}
impl DriverRepair {
    /// What a person who asked is told.
    pub fn text(&self) -> String {
        let mut text = match self.repaired.as_slice() {
            [] => "No driver was set up again: a driver left out at install stays out.".to_owned(),
            [driver] => format!("The {} driver was set up again.", driver.name()),
            _ => "The virtual display and gamepad drivers were set up again.".to_owned(),
        };
        if self.restart {
            text.push_str(" Windows must restart to finish.");
        }
        text
    }
}

/// `butterpollo-setup.exe --repair-drivers`, with the driver scripts of the
/// installation setup runs from. An old host's uninstaller, an in-app update
/// before rc.27 or a failed driver step leaves the display driver missing,
/// and Artemis then reports the "SudoVDA" driver as not installed; a removed
/// device or a dropped package does the same to the gamepad driver. Sets
/// `drivers` up again (the service names the ones it found missing), never
/// during a stream; the service's run (`RepairRun::Service`) is limited per
/// driver as [`repair_skipped`] says and only a positive idle answer from the
/// host lets it go on. A person (`--now`) is not limited, so a failed
/// automatic attempt cannot lock them out for the day, and a host that hides
/// its counts (a LAN bind_address) does not stop them; a stream still does.
pub fn repair_drivers(
    progress: &Progress,
    run: RepairRun,
    drivers: &[Driver],
) -> Result<DriverRepair> {
    let exe = std::env::current_exe()?;
    let install = system::win32_path(exe.parent().context("setup folder unavailable")?)?;
    repair_drivers_in(&install, progress, run, drivers)
}
/// [`repair_drivers`] with the driver scripts in `install`.
pub(crate) fn repair_drivers_in(
    install: &Path,
    progress: &Progress,
    run: RepairRun,
    drivers: &[Driver],
) -> Result<DriverRepair> {
    let profile = profile();
    // Verify and lock down ProgramData\Butterpollo before reading or writing
    // driver-setup.lock / driver-repair.json as SYSTEM.
    super::prepare_profile(&profile, true)
        .context("checking the Rubylight settings folder before repairing drivers")?;
    let path = profile.join(REPAIR_RECORD);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    let version = env!("CARGO_PKG_VERSION");
    let mut record = read_record(std::fs::read(&path).ok().as_deref());
    // In the order of Driver::ALL, each once.
    let due: Vec<Driver> = Driver::ALL
        .into_iter()
        .filter(|driver| drivers.contains(driver))
        .filter(
            |driver| match repair_skipped(&record, *driver, version, now, run) {
                Some(reason) => {
                    line(format!(
                        "not repairing the {} driver: {reason}. {REPAIR_BY_HAND}",
                        driver.name()
                    ));
                    false
                }
                None => true,
            },
        )
        .collect();
    if due.is_empty() {
        return Ok(DriverRepair::default());
    }
    // Setup or an update that is installing the drivers right now restarts
    // the service when done, and the service checks again.
    let Some(_turn) = driver_lock(Duration::ZERO) else {
        return when_locked(run.by_hand()).map(|_| DriverRepair::default());
    };
    let address = probe(&profile);
    if let Err(error) = ensure_idle_positive(
        &serverinfo(address),
        || system::process_running("butterpollo.exe"),
        run.by_hand(),
    ) {
        if run.by_hand() {
            return Err(error);
        }
        line(format!(
            "not repairing the drivers now: {error:#}. The service asks again later. {REPAIR_BY_HAND}"
        ));
        return Ok(DriverRepair::default());
    }
    for driver in &due {
        record = record_attempt(record, *driver, version, now);
    }
    std::fs::write(&path, serde_json::to_vec(&record)?)?;
    let mut notes = Vec::new();
    let mut failed = Vec::new();
    let mut restart = false;
    // A repair never installs an optional USB/IP driver.
    for driver in &due {
        let before = notes.len();
        let (needs_restart, own) = match driver {
            Driver::Display => display_step(install, progress, &mut notes),
            Driver::Gamepad => gamepad_step(install, progress, &mut notes),
        };
        restart |= needs_restart;
        failed.extend_from_slice(step_notes(&notes, before, own));
    }
    for note in &notes {
        line(format!("note: {note}"));
    }
    if !failed.is_empty() {
        bail!("{}", failed.join("\n"));
    }
    Ok(DriverRepair {
        restart,
        repaired: due,
    })
}
/// Another setup holds the driver lock. It restarts the service when done and
/// the service checks again, so the service's run ends quietly; a person who
/// asked is told nothing was repaired.
pub(super) fn when_locked(by_hand: bool) -> Result<bool> {
    if by_hand {
        bail!("Setup is installing the drivers right now; try again when it finishes.");
    }
    Ok(false)
}
/// The notes a driver's own step added, `own` of them from `before` on: a
/// step that failed or was refused, which fails the repair. A warning about
/// removing the old driver it replaces comes after them and stays a note.
pub(super) fn step_notes(notes: &[String], before: usize, own: usize) -> &[String] {
    let start = before.min(notes.len());
    &notes[start..(before + own).min(notes.len())]
}
