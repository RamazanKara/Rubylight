use super::*;
use crate::display::hdr_matches_applied as restore_hdr_needed;

#[test]
fn hdr_is_restored_only_while_the_display_has_the_applied_value() {
    assert!(restore_hdr_needed(true, true, true));
    assert!(restore_hdr_needed(false, false, false));
    assert!(!restore_hdr_needed(false, false, true));
    assert!(!restore_hdr_needed(true, true, false));
    // Windows 11 exclusive-fullscreen SDR app drops active mode while leaving
    // the HDR user switch armed: applied=true still needs restoring to off,
    // while applied=false must respect that the user re-enabled HDR.
    assert!(restore_hdr_needed(false, true, true));
    assert!(!restore_hdr_needed(false, true, false));
}

#[test]
fn a_crashed_streams_mode_is_restored_only_while_the_display_shows_what_the_stream_set() {
    let original = (3840, 2160, 60000);
    let requested = (1920, 1080, 120000);
    // A TV can take the resolution and keep its old rate, or the rate alone.
    let kept_rate = (1920, 1080, 60000);
    let kept_size = (3840, 2160, 120000);
    // A mode the user set by hand after the crash.
    let by_hand = (2560, 1440, 144000);
    // A clean end records the mode the display actually took: it is put back
    // only while the display still has it, and a change the user made since
    // stays.
    assert!(restore_mode_needed(false, kept_rate, kept_rate, original));
    assert!(!restore_mode_needed(false, requested, kept_rate, original));
    // A crash can leave the requested mode recorded while the display landed
    // on part of it: the original still comes back.
    assert!(restore_mode_needed(true, requested, requested, original));
    assert!(restore_mode_needed(true, kept_rate, requested, original));
    assert!(restore_mode_needed(true, kept_size, requested, original));
    // A mode that is none of it was set by the user since: it stays.
    assert!(!restore_mode_needed(true, by_hand, requested, original));
    assert!(!restore_mode_needed(
        true,
        (3840, 2160, 144000),
        requested,
        original
    ));
}

#[test]
fn a_hand_set_mode_with_the_same_refresh_survives_a_crashed_apply() {
    // 3840x2160@60 desktop, the stream asks 1920x1080@60, the host crashes during the
    // apply, and the user then sets 2560x1440@60: only the refresh matches.
    let original = (3840, 2160, 60000);
    let requested = (1920, 1080, 60000);
    assert!(!restore_mode_needed(
        true,
        (2560, 1440, 60000),
        requested,
        original
    ));
    // The requested mode, or the original that never changed, still comes back.
    assert!(restore_mode_needed(true, requested, requested, original));
    assert!(restore_mode_needed(true, original, requested, original));
}

#[test]
fn a_requested_mode_entry_keeps_its_marker_until_released() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("display-recovery.json");
    std::fs::write(
        &path,
        serde_json::to_vec(&serde_json::json!({
            "pid": 0,
            "started": 0,
            "entries": {"tv": {
                "output": r"\\.\DISPLAY2",
                "mode": null,
                "hdr": null,
                "mode_rate": [[3840, 2160, 60000], [1920, 1080, 120000]],
                "mode_rate_requested": true,
            }},
        }))?,
    )?;
    // The marker survives the read so recovery restores unconditionally, and
    // a journal from before the marker existed reads as an actual mode.
    let journal: Journal = serde_json::from_slice(&std::fs::read(&path)?)?;
    assert!(journal.entries["tv"].mode_rate_requested);
    let older: Entry = serde_json::from_str(
        r#"{"output":"","mode":null,"hdr":null,"mode_rate":[[3840,2160,60000],[1920,1080,120000]]}"#,
    )?;
    assert!(!older.mode_rate_requested);
    // A clean end clears the mode and its marker together.
    release_in(&path, "tv")?;
    let journal: Journal = serde_json::from_slice(&std::fs::read(&path)?)?;
    assert!(journal.entries.is_empty());
    Ok(())
}

#[test]
fn the_next_arrangement_adopts_the_original_layout_after_a_failed_restore() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("display-recovery.json");
    assert!(read_pending_arrangement(&path)?.is_none());
    let before = crate::display::Snapshot::decode(&serde_json::json!({
        "topology": [["physical"]],
        "modes": {"physical": {"w": 2560, "h": 1440, "num": 120, "den": 1}},
        "primary": "physical"
    }))?;
    let mut pending = Some((before, Vec::new()));
    assert!(restored(&mut pending, Err(anyhow::anyhow!("Windows is busy"))).is_err());
    let mut journal = Journal::default();
    journal.entries.insert(
        "display-arrangement".into(),
        Entry {
            arrangement: pending.clone(),
            ..Default::default()
        },
    );
    butterpollo_session::state::write_json(&path, &journal)?;
    let original = read_pending_arrangement(&path)?.expect("unfinished layout");
    assert_eq!(original.nodes[0].device_id, "physical");
    assert!(original.nodes[0].active);
    assert_eq!(original.nodes[0].mode.width, 2560);
    restored(&mut pending, Ok(()))?;
    journal
        .entries
        .get_mut("display-arrangement")
        .unwrap()
        .arrangement = pending;
    butterpollo_session::state::write_json(&path, &journal)?;
    assert!(read_pending_arrangement(&path)?.is_none());
    // Corrupt recovery data must not be overwritten by a new baseline.
    std::fs::write(&path, b"{")?;
    assert!(read_pending_arrangement(&path).is_err());
    Ok(())
}
#[test]
fn microphone_recovery_survives_journal_rewrites() -> Result<()> {
    let entry = Entry {
        microphone: true,
        ..Default::default()
    };
    let retained = entry.display_settings();
    let mut reloaded: Entry = serde_json::from_slice(&serde_json::to_vec(&retained)?)?;
    assert!(reloaded.microphone);
    assert!(reloaded.pending());
    reloaded.microphone = false;
    assert!(!reloaded.pending());
    let old: Entry = serde_json::from_str(r#"{"output":"","mode":null,"hdr":null}"#)?;
    assert!(!old.microphone);
    Ok(())
}
#[test]
fn failed_display_settings_survive_the_next_recovery_journal() -> Result<()> {
    let original = Entry {
        hdr: Some((false, true)),
        mode: Some(((3840, 2160, 60), (1920, 1080, 120))),
        profile: Some((None, "stream.icm".into(), false)),
        ..Default::default()
    };
    let mut pending = original.display_settings();
    assert!(restored(&mut pending.hdr, Err(anyhow::anyhow!("busy"))).is_err());
    restored(&mut pending.mode, Ok(()))?;
    assert!(restored(&mut pending.profile, Err(anyhow::anyhow!("busy"))).is_err());
    let mut reloaded: Entry = serde_json::from_slice(&serde_json::to_vec(&pending)?)?;
    assert_eq!(reloaded.hdr, original.hdr);
    assert_eq!(reloaded.profile, original.profile);
    assert!(reloaded.mode.is_none());
    restored(&mut reloaded.hdr, Ok(()))?;
    restored(&mut reloaded.profile, Ok(()))?;
    assert!(!reloaded.pending());
    Ok(())
}
#[test]
fn failed_limiter_audio_activation_and_layout_restores_stay_pending() -> Result<()> {
    let layout = crate::display::Snapshot::decode(&serde_json::json!({
        "topology": [["tv"]],
        "modes": {"tv": {"w": 3840, "h": 2160, "num": 60, "den": 1}},
        "primary": "tv"
    }))?;
    let original = Entry {
        external: true,
        audio: true,
        activation: Some((layout.clone(), Vec::new())),
        baseline: Some((layout, vec!["virtual".into()])),
        ..Default::default()
    };
    let mut kept = Journal::default();
    kept.entries
        .insert("tv".into(), original.display_settings());
    kept.entries.insert(
        "monitor".into(),
        Entry {
            external: true,
            ..Default::default()
        },
    );
    // The endpoint is missing and the VDD is tearing down: nothing is lost.
    assert!(
        restored_all(
            &mut kept,
            false,
            |e| &mut e.audio,
            Err(anyhow::anyhow!("gone"))
        )
        .is_err()
    );
    let pending = kept.entries.get_mut("tv").unwrap();
    assert!(restored(&mut pending.activation, Err(anyhow::anyhow!("busy"))).is_err());
    assert!(restored(&mut pending.baseline, Err(anyhow::anyhow!("busy"))).is_err());
    // The limiter came back: every entry drops its flag.
    restored_all(&mut kept, false, |e| &mut e.external, Ok(()))?;
    assert!(!kept.entries["monitor"].pending());
    let reloaded: Journal = serde_json::from_slice(&serde_json::to_vec(&kept)?)?;
    let tv = &reloaded.entries["tv"];
    assert!(tv.audio && !tv.external);
    assert!(tv.activation.is_some() && tv.baseline.is_some());
    assert_eq!(tv.baseline.as_ref().unwrap().1, ["virtual"]);
    let mut tv = tv.clone();
    restored(&mut tv.activation, Ok(()))?;
    restored(&mut tv.baseline, Ok(()))?;
    tv.audio = false;
    assert!(!tv.pending());
    Ok(())
}
#[test]
fn a_turned_or_scaled_display_gets_its_original_back_unless_the_user_changed_it() -> Result<()> {
    let mut set = Vec::new();
    put_back(Ok(2), 1, 2, |previous| {
        set.push(previous);
        Ok(())
    })?;
    // Turned again at the PC since: the user's rotation stays.
    put_back(Ok(4), 1, 2, |previous| {
        set.push(previous);
        Ok(())
    })?;
    assert_eq!(set, [1]);
    // A display that cannot be read, or refuses, keeps its entry for later.
    let mut pending = Some((100, 150));
    let gone = put_back(Err(anyhow::anyhow!("gone")), 100, 150, |_| {
        panic!("not read")
    });
    assert!(restored(&mut pending, gone).is_err());
    let refused = put_back(Ok(150), 100, 150, |_| bail!("busy"));
    assert!(restored(&mut pending, refused).is_err());
    assert_eq!(pending, Some((100, 150)));
    // A journal from before rotation and scale were kept reads without them.
    let old: Entry = serde_json::from_str(r#"{"output":"","mode":null,"hdr":null}"#)?;
    assert!(old.rotation.is_none() && old.scale.is_none());
    // Either one alone keeps the entry, and recovery keeps both until restored.
    let turned = Entry {
        rotation: Some((1, 2)),
        scale: Some((100, 150)),
        ..Default::default()
    };
    assert!(turned.pending());
    let kept = turned.display_settings();
    assert_eq!(
        (kept.rotation, kept.scale),
        (Some((1, 2)), Some((100, 150)))
    );
    Ok(())
}
#[test]
fn stalled_watcher_exit_is_bounded() -> Result<()> {
    use std::os::windows::process::CommandExt;
    let mut child = Command::new("cmd.exe")
        .args(["/C", "set /p waiting="])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW.0)
        .spawn()?;
    let started = std::time::Instant::now();
    wait_for_watch(&mut child)?;
    // Bounded by the 100 ms grace and the wait after the kill, not by how
    // quickly a busy PC ends the process.
    assert!(
        started.elapsed()
            < std::time::Duration::from_millis(100 + u64::from(KILLED_WATCH_EXIT_MS) + 1000)
    );
    assert_eq!(
        // SAFETY: Child retains its process handle through this bounded exit check.
        unsafe { WaitForSingleObject(HANDLE(child.as_raw_handle()), 1000) },
        WAIT_OBJECT_0
    );
    assert!(!child.wait()?.success());
    Ok(())
}
/// A killed watcher that takes `exit_ms` to go, as on a busy PC.
fn stop_slow_watch(exit_ms: u32) -> (Result<()>, Vec<u32>, bool) {
    let mut waits = Vec::new();
    let killed = std::cell::Cell::new(false);
    let result = stop_watch(
        |ms| {
            waits.push(ms);
            if killed.get() && ms >= exit_ms {
                WAIT_OBJECT_0
            } else {
                WAIT_TIMEOUT
            }
        },
        || {
            killed.set(true);
            Ok(())
        },
    );
    (result, waits, killed.get())
}
#[test]
fn a_killed_watcher_slow_to_exit_on_a_busy_pc_stops() {
    let (result, waits, killed) = stop_slow_watch(1500);
    result.unwrap();
    assert!(killed);
    assert_eq!(waits, [100, KILLED_WATCH_EXIT_MS]);
    let (result, _, _) = stop_slow_watch(KILLED_WATCH_EXIT_MS + 1);
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("did not stop within 2000 ms of being killed")
    );
}
#[test]
fn completed_journal_wakes_watcher_while_host_is_alive() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let pid = std::process::id();
    let journal = Journal {
        pid,
        started: identity(pid)?.unwrap(),
        ..Default::default()
    };
    butterpollo_session::state::write_json(
        &directory.path().join("display-recovery.json"),
        &journal,
    )?;
    let done = watch_done(pid)?;
    // SAFETY: `done` owns the event while the watcher opens and waits for it.
    unsafe { ResetEvent(HANDLE(done.as_raw_handle()))? };
    let path = directory.path().to_owned();
    let (sent, received) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sent.send(wait_and_recover(pid, &path));
    });
    // SAFETY: `done` still owns this handle. A signal before the wait must be retained.
    unsafe { SetEvent(HANDLE(done.as_raw_handle()))? };
    received.recv_timeout(std::time::Duration::from_secs(2))??;
    let after: Journal = serde_json::from_slice(&std::fs::read(
        directory.path().join("display-recovery.json"),
    )?)?;
    assert_eq!(after.pid, pid);
    Ok(())
}
#[test]
fn a_kept_entry_gives_its_originals_until_released() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("display-recovery.json");
    std::fs::write(
        &path,
        serde_json::to_vec(&serde_json::json!({
            "pid": 0,
            "started": 0,
            "entries": {"tv": {
                "output": r"\\.\DISPLAY2",
                "mode": null,
                "hdr": [false, true],
                "mode_rate": [[3840, 2160, 60000], [1920, 1080, 120000]],
                "profile": [null, "hdr.icm", false],
                "rotation": [1, 2],
                "scale": [100, 150],
            }},
        }))?,
    )?;
    assert_eq!(
        pending_in(&path, "tv")?,
        Pending {
            hdr: Some(false),
            mode: Some(((3840, 2160, 60000), (1920, 1080, 120000))),
            profile: Some(None),
            rotation: Some(1),
            scale: Some(100),
        }
    );
    assert_eq!(pending_in(&path, "monitor")?, Pending::default());
    // Releasing the display settings keeps the colour profile's entry.
    release_in(&path, "tv")?;
    assert_eq!(
        pending_in(&path, "tv")?,
        Pending {
            profile: Some(None),
            ..Default::default()
        }
    );
    Ok(())
}
#[test]
fn exited_process_handles_do_not_keep_recovery_journals_owned() {
    use std::os::windows::process::CommandExt;
    let mut child = std::process::Command::new("cmd.exe")
        .args(["/C", "exit", "0"])
        .current_dir(std::env::temp_dir())
        .creation_flags(CREATE_NO_WINDOW.0)
        .spawn()
        .unwrap();
    let pid = child.id();
    child.wait().unwrap();
    // Child still retains its process handle here, just like SCM/watchers.
    assert_eq!(identity(pid).unwrap(), None);
    assert!(identity(std::process::id()).unwrap().is_some());
}

#[test]
fn recover_pending_retries_after_incomplete_and_stops_once_restored() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("display-recovery.json");
    let kept = Journal {
        pid: 0,
        started: 0,
        entries: BTreeMap::from([(
            "display1".into(),
            Entry {
                hdr: Some((false, true)),
                ..Default::default()
            },
        )]),
    };
    butterpollo_session::state::write_json(&path, &kept).unwrap();

    // First retry fails (e.g. Access denied on Winlogon): pending journal remains.
    let err =
        recover_pending_at(&path, |_| anyhow::bail!("Access is denied. (os error 5)")).unwrap_err();
    assert!(err.to_string().contains("Access is denied"));
    assert!(path.exists());

    // Second retry succeeds and removes the journal.
    let restored = recover_pending_at(&path, |p| {
        std::fs::remove_file(p)?;
        Ok(())
    })
    .unwrap();
    assert!(restored);
    assert!(!recover_pending_at(&path, |_| unreachable!()).unwrap());
}

#[test]
fn recover_pending_skips_when_current_process_owns_a_live_journal() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("display-recovery.json");
    let pid = std::process::id();
    let live = Journal {
        pid,
        started: identity(pid).unwrap().unwrap(),
        entries: BTreeMap::from([(
            "display1".into(),
            Entry {
                hdr: Some((false, true)),
                ..Default::default()
            },
        )]),
    };
    butterpollo_session::state::write_json(&path, &live).unwrap();

    let ran = recover_pending_at(&path, |_| panic!("must not recover live host journal")).unwrap();
    assert!(!ran);
}

#[test]
fn unreadable_display_recovery_journal_is_quarantined_cleanly() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("display-recovery.json");
    std::fs::write(&path, b"{not valid json")?;

    let outcome = initialize_in(&path, |_| {
        unreachable!("corrupt journal must not run recover")
    })?;
    assert!(outcome.is_some());
    assert!(!path.exists());
    assert!(dir.path().join("display-recovery.corrupt.json").exists());
    assert!(!has_pending());
    Ok(())
}

#[test]
fn pending_leftover_entries_remain_recoverable_when_journal_marked_stale() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("display-recovery.json");
    let pid = std::process::id();
    let mixed = Journal {
        pid,
        started: identity(pid)?.unwrap(),
        entries: BTreeMap::from([
            (
                "tv".into(),
                Entry {
                    hdr: Some((false, true)),
                    stale: true,
                    ..Default::default()
                },
            ),
            (
                "monitor".into(),
                Entry {
                    hdr: Some((false, true)),
                    stale: false,
                    ..Default::default()
                },
            ),
        ]),
    };
    butterpollo_session::state::write_json(&path, &mixed)?;

    let mut ran = false;
    let recovered = recover_pending_at(&path, |_| {
        ran = true;
        Ok(())
    })?;
    assert!(recovered);
    assert!(ran);
    Ok(())
}

#[test]
fn a_scale_is_put_back_after_a_crash_unless_it_is_already_the_original() {
    // 150 % before the stream, 100 % requested.
    // A clean end recorded the scale that landed: only that one is put back,
    // and a scale the user picked since stays.
    assert!(restore_scale_needed(false, 100, 100, 150));
    assert!(!restore_scale_needed(false, 125, 100, 150));
    assert!(!restore_scale_needed(false, 150, 100, 150));
    // A crash inside the apply: Windows may have landed on a scale of its own,
    // and the original comes back from anything else.
    assert!(restore_scale_needed(true, 100, 100, 150));
    assert!(restore_scale_needed(true, 125, 100, 150));
    // Nothing to do when the original never changed.
    assert!(!restore_scale_needed(true, 150, 100, 150));
}

#[test]
fn releasing_a_display_clears_its_requested_scale() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("display-recovery.json");
    let mut journal = Journal::default();
    journal.entries.insert(
        "primary".into(),
        Entry {
            scale: Some((150, 100)),
            scale_requested: true,
            ..Default::default()
        },
    );
    butterpollo_session::state::write_json(&path, &journal).unwrap();
    assert_eq!(pending_in(&path, "primary").unwrap().scale, Some(150));
    release_in(&path, "primary").unwrap();
    let journal: Journal = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(journal.entries.is_empty());
}
