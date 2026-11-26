//! Tests of the Quick Suspend journal and its recovery.
use super::journal::{self, Journal, Record};
use super::{Resumed, Thaw, recover, stale};
use std::collections::BTreeMap;

fn journal() -> Journal {
    let mut journal = Journal::new((4100, 133_000), "Hollow Knight", 1_763_772_200_000);
    journal.processes = vec![
        Record {
            pid: 22_104,
            started: 133_765,
            name: "crash_handler.exe".into(),
        },
        Record {
            pid: 22_100,
            started: 133_760,
            name: "hollow_knight.exe".into(),
        },
    ];
    journal
}

/// A process table: what runs (pid → creation time), and what was resumed.
#[derive(Default)]
struct Table {
    running: BTreeMap<u32, u64>,
    denied: Vec<u32>,
    resumed: Vec<u32>,
}

impl Thaw for Table {
    fn resume(&mut self, pid: u32, started: u64) -> Result<Resumed, String> {
        if self.running.get(&pid) != Some(&started) {
            return Ok(Resumed::Gone);
        }
        if self.denied.contains(&pid) {
            return Err("access denied".into());
        }
        self.resumed.push(pid);
        Ok(Resumed::Done)
    }
}

#[test]
fn recovery_resumes_parents_first_and_leaves_reused_ids_alone() {
    let mut table = Table {
        running: BTreeMap::from([(22_100, 133_760), (22_104, 999)]),
        ..Default::default()
    };
    let recovery = recover(&journal(), &mut table);
    // 22104 is now another program: never touched.
    assert_eq!(table.resumed, [22_100]);
    assert_eq!(recovery.resumed, ["hollow_knight.exe"]);
    assert_eq!(recovery.gone, 1);
    assert!(recovery.complete());
}

#[test]
fn a_failed_resume_is_reported_and_the_rest_still_runs() {
    let mut table = Table {
        running: BTreeMap::from([(22_100, 133_760), (22_104, 133_765)]),
        denied: vec![22_100],
        ..Default::default()
    };
    let recovery = recover(&journal(), &mut table);
    assert_eq!(table.resumed, [22_104]);
    assert_eq!(recovery.failed.len(), 1);
    assert_eq!(recovery.failed[0].0.pid, 22_100);
    assert_eq!(recovery.failed[0].1, "access denied");
    assert!(!recovery.complete());
}

#[test]
fn a_host_recovers_any_journal_but_its_own_or_a_running_hosts() {
    let journal = journal();
    let none_alive = |_: u32, _: u64| false;
    // A crashed or killed host, a restarted one, or a reboot.
    assert!(stale(&journal, (5000, 140_000), none_alive));
    // Its own journal it resumes itself.
    assert!(!stale(&journal, (4100, 133_000), none_alive));
    // Another host that still runs from the same profile keeps its own.
    assert!(!stale(&journal, (5000, 140_000), |pid, started| {
        (pid, started) == (4100, 133_000)
    }));
    // Its id reused by another program after a reboot: the journal is stale.
    assert!(stale(&journal, (5000, 140_000), |pid, started| {
        (pid, started) == (4100, 1)
    }));
}

#[test]
fn the_journal_is_written_whole_read_back_and_removed() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(journal::load(dir.path()).unwrap(), None);
    journal::save(dir.path(), &journal()).unwrap();
    assert_eq!(journal::load(dir.path()).unwrap(), Some(journal()));
    // No temporary file is left beside it.
    let names: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names, [journal::FILE]);
    journal::remove(dir.path()).unwrap();
    journal::remove(dir.path()).unwrap();
    assert_eq!(journal::load(dir.path()).unwrap(), None);
}

#[test]
fn an_unreadable_journal_is_an_error_and_can_be_set_aside() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(journal::path(dir.path()), b"{\"version\":1,\"host_pid\"").unwrap();
    assert!(journal::load(dir.path()).is_err());
    let aside = journal::set_aside(dir.path()).unwrap();
    assert!(aside.ends_with("quick-suspend.json.bad"));
    assert_eq!(journal::load(dir.path()).unwrap(), None);
    // A newer format is refused rather than half understood.
    let mut newer = journal();
    newer.version = journal::VERSION + 1;
    journal::save(dir.path(), &newer).unwrap();
    assert!(journal::load(dir.path()).is_err());
    // A blank file names nothing.
    std::fs::write(journal::path(dir.path()), b"  \r\n").unwrap();
    assert_eq!(journal::load(dir.path()).unwrap(), None);
}

#[test]
fn unknown_fields_from_a_later_build_are_ignored() {
    let json = br#"{"version":1,"host_pid":1,"host_started":2,"processes":[
        {"pid":3,"started":4,"name":"game.exe","audio_muted":true}],"display":{}}"#;
    let journal: Journal = serde_json::from_slice(json).unwrap();
    assert_eq!(journal.processes[0].pid, 3);
    assert_eq!(journal.app_title, "");
}
