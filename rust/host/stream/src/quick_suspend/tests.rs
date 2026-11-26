//! Quick Suspend on the host: off by default, who may act, and what devices see.
use super::*;
use crate::state::test_support::Fixture;
use butterpollo_session::state::LAUNCH_APPS;

fn turn_on(f: &Fixture) {
    f.host
        .config
        .write()
        .unwrap()
        .values
        .insert("quick_suspend".into(), "manual".into());
}

fn host_control(f: &Fixture) -> Client {
    let mut client = f.client(LAUNCH_APPS);
    client.uuid = "controller-device".into();
    client
        .extra
        .insert("host_control".into(), serde_json::Value::Bool(true));
    client
}

#[test]
fn off_by_default_nothing_is_offered_or_paused() {
    let f = Fixture::new();
    assert!(!crate::extensions::quick_suspend(
        &f.host.config.read().unwrap()
    ));
    let state = status(&f.host);
    assert_eq!(
        (state.state, state.reason),
        (State::Unavailable, Reason::DisabledForApp)
    );
    let client = f.client(LAUNCH_APPS);
    let refused = suspend(&f.host, Who::Device(&client));
    assert_eq!(
        (refused.state, refused.reason),
        (State::Refused, Reason::DisabledForApp)
    );
    assert!(refused.message.unwrap().contains("Quick Suspend is off"));
    // Nothing was written: the journal exists only around a suspend.
    assert!(!journal::path(&f.host.directory).exists());
    assert!(!f.host.quick_suspend.suspended());
}

#[test]
fn only_the_owner_host_control_or_the_console_get_past_the_permission_check() {
    let f = Fixture::new();
    turn_on(&f);
    assert!(crate::extensions::quick_suspend(
        &f.host.config.read().unwrap()
    ));
    assert_eq!(status(&f.host).reason, Reason::NoSession);
    // No session owns the game here: a plain device is no owner.
    let device = f.client(LAUNCH_APPS);
    assert_eq!(
        suspend(&f.host, Who::Device(&device)).reason,
        Reason::NotOwner
    );
    // Host control and the console pass, and find no game to pause.
    let controller = host_control(&f);
    assert_eq!(
        suspend(&f.host, Who::Device(&controller)).reason,
        Reason::NoSession
    );
    assert_eq!(suspend(&f.host, Who::Console).reason, Reason::NoSession);
    assert!(!journal::path(&f.host.directory).exists());
    // A resume with nothing paused answers the state.
    let resumed = resume(&f.host, Who::Device(&device));
    assert_eq!(resumed.state, State::Unavailable);
    resume_for(&f.host, "a test");
    tick(&f.host);
    assert!(!f.host.quick_suspend.suspended());
}

#[test]
fn devices_read_the_same_state_on_the_wire_and_from_the_native_api() {
    let refused = Status::refused(
        &Refused {
            reason: Reason::AntiCheatDetected,
            program: Some("Easy Anti-Cheat".into()),
        },
        Some("Arena".into()),
    );
    let wire = refused.wire(7);
    assert_eq!(
        (wire.request_id, wire.state, wire.reason),
        (7, State::Refused, Reason::AntiCheatDetected)
    );
    let native = refused.native();
    assert_eq!(native.state, SuspendState::Refused);
    assert_eq!(native.reason, "anti_cheat_detected");
    assert!(native.message.unwrap().contains("Easy Anti-Cheat"));
    assert_eq!(native.suspended_since_unix_ms, None);
    let mut journal = Journal::new((1, 2), "Hollow Knight", 1_763_772_200_000);
    journal.processes.push(Record {
        pid: 3,
        started: 4,
        name: "hollow_knight.exe".into(),
    });
    let suspended = suspended_status(&Suspended {
        journal,
        count: 1,
        by: "Phone".into(),
    });
    let native = suspended.native();
    assert_eq!(native.state, SuspendState::Suspended);
    assert_eq!(native.reason, "none");
    assert_eq!(native.message, None);
    assert_eq!(native.suspended_since_unix_ms, Some(1_763_772_200_000));
    assert_eq!(native.suspended_by.as_deref(), Some("Phone"));
    assert_eq!(suspended.wire(0).suspended_count, 1);
}

/// Resumes that fail, or find every process gone.
struct Fake {
    fails: bool,
}

impl Thaw for Fake {
    fn resume(
        &mut self,
        _: u32,
        _: u64,
    ) -> Result<butterpollo_session::quick_suspend::Resumed, String> {
        if self.fails {
            Err("access denied".into())
        } else {
            Ok(butterpollo_session::quick_suspend::Resumed::Gone)
        }
    }
}

#[test]
fn a_process_that_stays_suspended_stays_in_the_journal_and_blocks_a_new_suspend() {
    let f = Fixture::new();
    turn_on(&f);
    let mut paused = Journal::new(suspend_watch::me(), "Arena", 1_763_772_200_000);
    paused.processes.push(Record {
        pid: 0x7fff_fff0,
        started: 5,
        name: "arena.exe".into(),
    });
    *f.host.quick_suspend.lock() = Some(Suspended {
        journal: paused.clone(),
        count: 1,
        by: "Phone".into(),
    });
    // The resume fails: the answer names the process, and the journal keeps it.
    let failed = resume_with(&f.host, Who::Console, &mut Fake { fails: true });
    assert_eq!(
        (failed.state, failed.reason),
        (State::Refused, Reason::IoOrOsError)
    );
    assert!(failed.message.unwrap().contains("arena.exe"));
    assert!(!f.host.quick_suspend.suspended());
    let left = journal::load(&f.host.directory).unwrap().unwrap();
    assert_eq!(left.processes, paused.processes);
    // A new suspend tries it again first, and pauses nothing while it fails.
    let refused = leftovers(&f.host, &mut Fake { fails: true }).unwrap_err();
    assert_eq!(refused.reason, Reason::IoOrOsError);
    assert_eq!(refused.program.as_deref(), Some("arena.exe"));
    assert!(journal::path(&f.host.directory).exists());
    // Once it runs again, or is gone, the journal goes.
    leftovers(&f.host, &mut Fake { fails: false }).unwrap();
    assert!(!journal::path(&f.host.directory).exists());
    // A resume that works clears the journal at once.
    *f.host.quick_suspend.lock() = Some(Suspended {
        journal: paused,
        count: 1,
        by: "Phone".into(),
    });
    let resumed = resume_with(&f.host, Who::Console, &mut Fake { fails: false });
    assert_ne!(resumed.state, State::Refused);
    assert!(!journal::path(&f.host.directory).exists());
}

#[test]
fn every_way_a_session_ends_resumes_the_game_first() {
    // The teardown points every stop passes, the next stream of the game, a
    // clean shutdown and the housekeeping tick.
    let control = include_str!("../session_control.rs");
    assert_eq!(control.matches("quick_suspend::resume_for(self").count(), 3);
    assert!(control.contains("crate::quick_suspend::tick(self);"));
    let launch = include_str!("../nvhttp/launch_steps.rs");
    let resume = launch.find("quick_suspend::resume_for(h,").unwrap();
    let start = launch.find("crate::process::launch(h,").unwrap();
    assert!(resume < start);
    // Another app closes it only after it runs again, and a stream of it
    // runs it again before its display is prepared.
    let replace = &launch[launch.find("fn replace_launch_app(").unwrap()..];
    let replace = &replace[..replace
        .find(
            "
}
",
        )
        .unwrap()];
    let resumed = replace.find("another app replaces the game").unwrap();
    assert!(resumed < replace.find("h.end_app(previous").unwrap());
    assert!(replace.contains("\"a stream of the game starts\""));
}
