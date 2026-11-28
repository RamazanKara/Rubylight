//! Tests for departure repair, mode reports, primary watch and restoration,
//! recovery retry scheduling, baseline journaling, capture targets, and HDR profiles.
use super::*;

#[test]
fn a_display_that_left_lays_out_again_with_what_the_watch_keeps_on() {
    use butterpollo_media_policy::display_policy::{LayoutAction, LayoutWatch};
    let start = std::time::Instant::now();
    let mut watch = LayoutWatch::default();
    // A display that keeps coming back is left on, without a reapply of its own.
    let mut now = start;
    let mut kept = false;
    for _ in 0..40 {
        now += Duration::from_secs(1);
        if let LayoutAction::Reapply { displays, kept_now } = watch.observe(&["tv".into()], now) {
            kept |= !kept_now.is_empty();
            watch.reapplied(&displays, now);
        }
    }
    assert!(kept);
    assert_eq!(watch.kept(), ["tv"]);
    // Nothing left since this stream last looked.
    let mut seen = 3;
    notice_departure(&mut seen, 3, &mut watch, now);
    assert!(!watch.departure_due(now + LayoutWatch::DEPARTURE_SETTLE));
    notice_departure(&mut seen, 4, &mut watch, now);
    assert_eq!(seen, 4);
    assert!(!watch.departure_due(now));
    now += LayoutWatch::DEPARTURE_SETTLE;
    assert!(watch.departure_due(now));
    // A failed attempt stays due, and the watch waits before it looks again.
    let warnings = Arc::new(Warnings::default());
    let report = ModeReport::new(warnings.clone(), (None, None, None), 60_000);
    let error = lay_out_after_departure(&mut watch, Vec::new(), now, &report, |_| {
        anyhow::bail!("no")
    });
    assert!(error.is_err());
    assert!(watch.departure_due(now));
    assert!(!watch.due(now));
    assert!(codes(&warnings).is_empty());
    // The kept display stays on beside the remote monitors.
    let mut asked = Vec::new();
    lay_out_after_departure(
        &mut watch,
        vec!["monitor".into()],
        now,
        &report,
        |retained| {
            asked = retained.to_vec();
            Ok(None)
        },
    )
    .unwrap();
    assert_eq!(asked, ["monitor", "tv"]);
    assert!(!watch.departure_due(now));
    assert!(codes(&warnings).is_empty());
    // A layout that went through after a repair that did not: nothing to try again, and the
    // card says so.
    watch.display_left(now);
    now += LayoutWatch::DEPARTURE_SETTLE;
    lay_out_after_departure(&mut watch, Vec::new(), now, &report, |_| {
        Ok(Some("refused".into()))
    })
    .unwrap();
    assert!(!watch.departure_due(now));
    assert_eq!(codes(&warnings), ["display_repair"]);
    assert!(warnings.snapshot()[0].message.contains("refused"));
    // The next departure's repair goes through.
    watch.display_left(now);
    now += LayoutWatch::DEPARTURE_SETTLE;
    lay_out_after_departure(&mut watch, Vec::new(), now, &report, |_| Ok(None)).unwrap();
    assert!(codes(&warnings).is_empty());
}
#[test]
fn a_resume_after_a_display_left_repairs_before_it_records_the_layout() {
    use butterpollo_media_policy::display_policy::LayoutWatch;
    let warnings = Arc::new(Warnings::default());
    let report = ModeReport::new(warnings.clone(), (None, None, None), 60_000);
    let chosen = |unsettled| {
        let ran = std::cell::Cell::new("nothing");
        reapply_settling(
            unsettled,
            &report,
            || {
                ran.set("reapply");
                Ok(())
            },
            || {
                ran.set("repair");
                Ok(None)
            },
        )
        .unwrap();
        ran.get()
    };
    let now = std::time::Instant::now();
    let mut watch = LayoutWatch::default();
    // Nothing left during the pause.
    assert_eq!(chosen(departure_unsettled(4, 4, &watch)), "reapply");
    // A display left during it: without `dd_hold_layout_while_paused` the heartbeat
    // never looked, and what is on now is what Windows recalled.
    assert_eq!(chosen(departure_unsettled(3, 4, &watch)), "repair");
    // It left just before the pause: seen, its repair still to come.
    watch.display_left(now);
    assert_eq!(chosen(departure_unsettled(4, 4, &watch)), "repair");
    watch.departure_handled();
    assert_eq!(chosen(departure_unsettled(4, 4, &watch)), "reapply");
    // A refused layout still fails the resume.
    assert!(reapply_settling(true, &report, || Ok(()), || anyhow::bail!("refused")).is_err());
    assert!(codes(&warnings).is_empty());
    // A repair that failed under a layout that went through: the resume goes on, and says so.
    reapply_settling(true, &report, || Ok(()), || Ok(Some("no entry".into()))).unwrap();
    assert_eq!(codes(&warnings), ["display_repair"]);
    assert!(warnings.snapshot()[0].message.contains("no entry"));
}

#[test]
fn a_recovered_display_mode_is_reported_on_the_card_of_the_launch_using_it() {
    let first = Arc::new(Warnings::default());
    let report = ModeReport::new(
        first.clone(),
        (Some((3840, 2160)), Some(1_000_000), Some(true)),
        116_000,
    );
    report.publish(Ok((3840, 2160, 1_000_000, true)));
    assert!(codes(&first).is_empty());
    // Windows switched the display back on at its saved 60 Hz mode and
    // refused the stream's: the card says so, the stream continues.
    report.publish(Ok((3840, 2160, 60_000, true)));
    assert_eq!(codes(&first), ["display_refresh"]);
    assert!(first.snapshot()[0].message.contains("60.000 Hz"));
    // A resumed launch sees the current finding on its own card.
    let second = Arc::new(Warnings::default());
    report.attach(second.clone());
    assert_eq!(codes(&second), ["display_refresh"]);
    report.publish(Err(anyhow::anyhow!("display unavailable")));
    assert_eq!(codes(&second), ["display_refresh", "display_verify"]);
    report.publish(Ok((3840, 2160, 1_000_000, true)));
    assert!(codes(&second).is_empty());
    // Nothing found yet: attaching reports nothing.
    let fresh = ModeReport::new(first.clone(), (None, None, None), 60_000);
    let third = Arc::new(Warnings::default());
    fresh.attach(third.clone());
    assert!(codes(&third).is_empty());
}
#[test]
fn a_refused_primary_restore_stays_on_the_card_until_the_display_has_it_again() {
    let warnings = Arc::new(Warnings::default());
    let report = ModeReport::new(warnings.clone(), (None, None, None), 60_000);
    assert!(codes(&warnings).is_empty());
    report.primary_refused("Windows refused the display layout (with modes: 0)");
    assert_eq!(codes(&warnings), ["display_primary"]);
    let message = &warnings.snapshot()[0].message;
    assert!(message.contains("main screen"), "{message}");
    assert!(message.contains("refused"), "{message}");
    // A resume reads the same finding on its own card, and clearing it is on that card.
    let resumed = Arc::new(Warnings::default());
    report.attach(resumed.clone());
    assert_eq!(codes(&resumed), ["display_primary"]);
    report.primary_restored();
    assert!(codes(&resumed).is_empty());
}
#[test]
fn a_layout_windows_accepted_but_that_left_the_primary_is_not_restored() {
    assert!(Prepared::primary_taken_back(&Ok(Some(true))));
    assert!(!Prepared::primary_taken_back(&Ok(Some(false))));
    assert!(!Prepared::primary_taken_back(&Ok(None)));
    assert!(!Prepared::primary_taken_back(&Err(anyhow::anyhow!(
        "Windows refused the display layout"
    ))));
}
#[test]
fn a_primary_windows_keeps_taking_back_stays_on_the_card() {
    let warnings = Arc::new(Warnings::default());
    let report = ModeReport::new(warnings.clone(), (None, None, None), 60_000);
    report.primary_gave_up(PrimaryStop::KeptTaking);
    assert_eq!(codes(&warnings), ["display_primary_kept"]);
    let message = &warnings.snapshot()[0].message;
    assert!(
        message.contains("kept taking the main screen back"),
        "{message}"
    );
    assert!(message.contains("Exclusive"), "{message}");
    // It is not a refusal to retry: clearing the refusal leaves the stream's last word.
    report.primary_restored();
    assert_eq!(codes(&warnings), ["display_primary_kept"]);
    // A restore Windows refused says so instead: it never had the main screen.
    report.primary_gave_up(PrimaryStop::Refused);
    let message = &warnings.snapshot()[0].message;
    assert!(message.contains("refused"), "{message}");
    assert!(!message.contains("kept taking"), "{message}");
}
#[test]
fn only_a_primary_the_layout_makes_for_this_stream_is_watched() {
    use butterpollo_media_policy::display_policy::Arrangement;
    let applies = |planned: Arrangement, lease_primary, active, secure| {
        Prepared::primary_watch_applies(planned, lease_primary, active, secure, true)
    };
    assert!(applies(Arrangement::Primary, true, true, false));
    // A second stream, another device's display or a second screen joins beside the first
    // owner's, and the layout makes only that one primary: re-laying this one out could
    // never change it, yet the old watch did so every 750 ms forever.
    assert!(!applies(Arrangement::Primary, false, true, false));
    // Extended keeps the PC's primary; exclusive keeps it by switching the others off.
    for planned in [Arrangement::Extended, Arrangement::Exclusive] {
        assert!(!applies(planned, true, true, false));
    }
    // A paused stream holds its layout but has nobody to show a move to.
    assert!(!applies(Arrangement::Primary, true, false, false));
    // The secure desktop is Windows' own rearranging: a restore then could not tell its
    // choice from a refusal, so nothing is reported. A session switch is not skipped by the
    // layout watch's excuse any more: the watch's own window ends a move that does not come.
    assert!(!applies(Arrangement::Primary, true, true, true));
    // A refused restore waits before the next try.
    assert!(!Prepared::primary_watch_applies(
        Arrangement::Primary,
        true,
        true,
        false,
        false
    ));
}
#[test]
fn a_primary_move_without_a_windows_change_is_left_alone() {
    use butterpollo_media_policy::display_policy::{
        LayoutWatch, PrimaryAction, PrimaryChange, PrimaryWatch,
    };
    let start = std::time::Instant::now();
    let mut watch = PrimaryWatch::default();
    // The user picks the monitor as the main screen in Windows, or a game takes it: no
    // display arrived or left and the session did not change, so nothing starts the watch,
    // however long the move lasts.
    for seconds in [0, 1, 10, 60, 600] {
        let now = start + Duration::from_secs(seconds);
        assert_eq!(watch.observe(true, now), PrimaryAction::Wait, "{seconds}s");
    }
    // Windows changed the layout: the move that follows is put back after the settle.
    watch.changed(PrimaryChange::Display, start);
    assert_eq!(watch.observe(true, start), PrimaryAction::Wait);
    assert_eq!(
        watch.observe(true, start + LayoutWatch::SETTLE),
        PrimaryAction::Restore
    );
}
#[test]
fn a_user_pick_before_a_later_windows_change_is_not_undone() {
    use butterpollo_media_policy::display_policy::PrimaryChange;
    let arms = |last_shown: Option<bool>, change: Option<PrimaryChange>| {
        Prepared::primary_change_arms(last_shown, change)
    };
    // A change while the stream's display had the primary can have taken it away.
    assert!(arms(Some(true), Some(PrimaryChange::Display)));
    assert!(arms(Some(true), Some(PrimaryChange::Secure)));
    assert!(arms(Some(true), Some(PrimaryChange::Session)));
    // The user picked the monitor as the main screen before the change: that pick stays,
    // whatever Windows does later.
    assert!(!arms(Some(false), Some(PrimaryChange::Secure)));
    assert!(!arms(Some(false), Some(PrimaryChange::Session)));
    assert!(!arms(Some(false), Some(PrimaryChange::Display)));
    // A display that was off says nothing about a pick before it, and no change arms any.
    assert!(!arms(None, Some(PrimaryChange::Display)));
    assert!(!arms(Some(true), None));
}

#[test]
fn the_start_layout_warning_goes_once_the_primary_is_restored() {
    let warnings = Arc::new(Warnings::default());
    let report = ModeReport::new(warnings.clone(), (None, None, None), 60_000);
    warnings.set(
        "display_layout",
        "Windows did not apply the extended_primary display layout; the stream's display is extended. Check that no other display tool rearranges displays, then reconnect.",
    );
    assert_eq!(codes(&warnings), ["display_layout"]);
    // The watch put the primary back: the stream needs no reconnect and the card drops it.
    report.start_layout_restored();
    assert!(codes(&warnings).is_empty());
}

#[test]
fn a_change_that_arrives_while_paused_arms_at_the_resume() {
    use butterpollo_media_policy::display_policy::PrimaryChange;
    let resumed = |paused, change| Prepared::resumed_primary_change(paused, change);
    // A display changed while the stream had nobody to show a restore to: the resume arms
    // the watch with it, at the resume time.
    assert_eq!(
        resumed(Some(PrimaryChange::Display), None),
        Some(PrimaryChange::Display)
    );
    assert_eq!(
        resumed(Some(PrimaryChange::Display), Some(PrimaryChange::Secure)),
        Some(PrimaryChange::Display)
    );
    assert_eq!(
        resumed(None, Some(PrimaryChange::Secure)),
        Some(PrimaryChange::Secure)
    );
    assert_eq!(resumed(None, None), None);
}

#[test]
fn a_shared_virtual_display_raised_by_a_joining_stream_is_no_finding() {
    let warnings = Arc::new(Warnings::default());
    let requested = (Some((1920, 1080)), Some(120_000), Some(false));
    let report = ModeReport::new(warnings.clone(), requested, 60_000).shared();
    // A VRR stream that joined raised it to 1000 Hz: that serves this one.
    report.publish(Ok((1920, 1080, 1_000_000, false)));
    assert!(codes(&warnings).is_empty());
    // A slower refresh than requested is still reported.
    report.publish(Ok((1920, 1080, 60_000, false)));
    assert_eq!(codes(&warnings), ["display_refresh"]);
    // A display of its own still reports a refresh it did not ask for.
    let own = Arc::new(Warnings::default());
    ModeReport::new(own.clone(), requested, 60_000).publish(Ok((1920, 1080, 1_000_000, false)));
    assert_eq!(codes(&own), ["display_refresh"]);
}

#[test]
fn a_display_that_cannot_follow_a_turned_stream_says_so_on_the_card() {
    let first = Arc::new(Warnings::default());
    let report = ModeReport::new(first.clone(), (None, None, None), 60_000);
    report.followed(&Err(anyhow::anyhow!("rotation refused")));
    assert_eq!(codes(&first), ["display_follow"]);
    assert!(first.snapshot()[0].message.contains("rotation refused"));
    // A later mode finding keeps it; a resumed launch sees it on its own card.
    report.publish(Ok((1920, 1080, 60_000, false)));
    assert_eq!(codes(&first), ["display_follow"]);
    let second = Arc::new(Warnings::default());
    report.attach(second.clone());
    assert_eq!(codes(&second), ["display_follow"]);
    // The next turn follows: the warning goes.
    report.followed(&Ok(()));
    assert!(codes(&second).is_empty());
}

#[test]
fn a_monitor_left_on_after_repeated_returns_says_so_on_the_card() {
    let first = Arc::new(Warnings::default());
    let report = ModeReport::new(first.clone(), (None, None, None), 60_000);
    report.keep_on();
    assert_eq!(codes(&first), ["display_kept_on"]);
    // A later mode finding keeps it; a resumed launch sees it on its own card.
    report.publish(Ok((1920, 1080, 60_000, false)));
    assert_eq!(codes(&first), ["display_kept_on"]);
    let second = Arc::new(Warnings::default());
    report.attach(second.clone());
    assert_eq!(codes(&second), ["display_kept_on"]);
}

#[test]
fn a_repair_that_failed_says_so_on_the_card_of_the_launch_in_progress() {
    let first = Arc::new(Warnings::default());
    let report = ModeReport::new(first.clone(), (None, None, None), 60_000);
    report.unrepaired_at_start(None);
    assert!(codes(&first).is_empty());
    // At the start of the stream.
    report.unrepaired_at_start(Some("the entry is gone"));
    assert_eq!(codes(&first), ["display_repair"]);
    assert!(first.snapshot()[0].message.contains("the entry is gone"));
    // A later mode finding keeps it; a resumed launch sees it on its own card.
    report.publish(Ok((1920, 1080, 60_000, false)));
    assert_eq!(codes(&first), ["display_repair"]);
    let second = Arc::new(Warnings::default());
    report.attach(second.clone());
    assert_eq!(codes(&second), ["display_repair"]);
    // A repair after a departure going through says nothing about the display's arrival.
    report.repaired(None);
    assert_eq!(codes(&second), ["display_repair"]);
    // After a departure, when the start was fine: a later repair clears it.
    let third = Arc::new(Warnings::default());
    let report = ModeReport::new(third.clone(), (None, None, None), 60_000);
    report.repaired(Some("refused".into()));
    assert_eq!(codes(&third), ["display_repair"]);
    report.repaired(None);
    assert!(codes(&third).is_empty());
}

#[test]
fn a_recovery_sets_the_mode_once_while_its_layout_is_retried() {
    let now = std::time::Instant::now();
    let mut recovery = Recovery::new();
    assert!(!recovery.due(now));
    assert!(!recovery.take_mode());
    recovery.start(now);
    assert!(recovery.due(now));
    assert!(recovery.take_mode());
    // The layout or HDR profile failed: retried a second later, but a
    // refused mode is not requested from Windows again.
    assert!(!recovery.due(now + Duration::from_millis(100)));
    assert!(recovery.due(now + Duration::from_secs(1)));
    assert!(!recovery.take_mode());
    recovery.finish();
    assert!(!recovery.due(now + Duration::from_secs(5)));
    // The next recovery sets it again.
    recovery.start(now + Duration::from_secs(6));
    assert!(recovery.due(now + Duration::from_secs(6)));
    assert!(recovery.take_mode());
}

#[test]
fn the_saved_layout_is_restored_once_after_the_last_stream() {
    let mut baseline = Baseline::new();
    let journal = Cell::new(false);
    let restores = Cell::new(0);
    baseline
        .acquire(|| {
            journal.set(true);
            Ok("layout")
        })
        .unwrap();
    baseline
        .acquire(|| panic!("a second stream must join the recorded layout"))
        .unwrap();
    let restore = |layout: &str| {
        assert_eq!(layout, "layout");
        restores.set(restores.get() + 1);
        journal.set(false);
    };
    baseline.release(restore);
    assert_eq!(restores.get(), 0);
    assert!(journal.get(), "the crash journal must stay pending");
    baseline.release(restore);
    assert_eq!(restores.get(), 1);
    assert!(!journal.get());
    // A failed record leaves no user behind; the next stream records again.
    assert!(baseline.acquire(|| anyhow::bail!("journal busy")).is_err());
    baseline.acquire(|| Ok("next")).unwrap();
    baseline.release(|layout| assert_eq!(layout, "next"));
}

#[test]
fn capture_target_stays_readable_during_refresh_and_publishes_identity_together() {
    let target = CaptureTarget::new(("old-output".into(), 0));
    let publisher = target.clone();
    let (entered, waiting) = mpsc::channel();
    let (release, resume) = mpsc::channel();
    let refresh = std::thread::spawn(move || {
        // Stand in for a slow driver call: publication happens only after
        // maintenance returns, while current capture must remain usable.
        entered.send(()).unwrap();
        resume.recv().unwrap();
        publisher.publish(("new-output".into(), 1));
    });
    waiting.recv_timeout(Duration::from_secs(10)).unwrap();
    let reader = target.clone();
    let (read, result) = mpsc::channel();
    let read_thread = std::thread::spawn(move || read.send(reader.current()).unwrap());
    // The refresh is held until the read returns, so a read that waited
    // for it would time out however long this is. 100 ms also failed
    // when a busy PC was slow to start the reader thread.
    let current = result.recv_timeout(Duration::from_secs(10));
    release.send(()).unwrap();
    refresh.join().unwrap();
    read_thread.join().unwrap();
    assert_eq!(current.unwrap(), ("old-output".into(), 0));
    assert_eq!(target.current(), ("new-output".into(), 1));

    let publisher = target.clone();
    let refresh = std::thread::spawn(move || {
        for generation in 2..10000 {
            publisher.publish((format!("output-{generation}"), generation));
        }
    });
    while !refresh.is_finished() {
        let (output, generation) = target.current();
        if generation > 1 {
            assert_eq!(output, format!("output-{generation}"));
        }
    }
    refresh.join().unwrap();
    assert_eq!(target.current(), ("output-9999".into(), 9999));
}

#[test]
fn an_app_window_is_laid_over_the_display_target_which_the_heartbeat_keeps_publishing() {
    let target = CaptureTarget::new((r"\.\DISPLAY9".into(), 3));
    let window = r"window:0x2056c/812@\.\DISPLAY9".to_owned();
    assert!(target.show_window(Some(window.clone())));
    assert!(!target.show_window(Some(window.clone())), "unchanged");
    assert_eq!(target.current(), (window.clone(), 0));
    // A recreated display while the window shows: the window stays.
    target.publish((r"\.\DISPLAY10".into(), 4));
    assert_eq!(target.current(), (window, 0));
    // Taken off, the stream shows the display as published last.
    assert!(target.show_window(None));
    assert_eq!(target.current(), (r"\.\DISPLAY10".into(), 4));
}

#[test]
fn monitor_override_outranks_window_survives_heartbeat_and_restores_on_clear() {
    let target = CaptureTarget::new((r"\\.\DISPLAY1".into(), 1));
    let window = r"window:0x2056c/812@\\.\DISPLAY1".to_owned();
    let initial_rev = target.revision();
    assert!(target.show_window(Some(window.clone())));
    assert!(target.revision() > initial_rev);

    // Override takes precedence over both app window and prepared display.
    assert!(target.set_monitor_override(Some(r"\\.\DISPLAY2".into())));
    assert!(target.has_monitor_override());
    let after_override_rev = target.revision();
    assert_eq!(target.current(), (r"\\.\DISPLAY2".into(), after_override_rev));
    assert_eq!(
        target.display_target(),
        (r"\\.\DISPLAY2".into(), after_override_rev)
    );
    assert_eq!(target.prepared_display_target(), (r"\\.\DISPLAY1".into(), 1));

    // Selecting the same override (case-insensitively) is a no-op.
    assert!(!target.set_monitor_override(Some(r"\\.\display2".into())));
    assert_eq!(target.revision(), after_override_rev);

    // Heartbeat publish updates prepared display without clobbering monitor_override.
    target.publish((r"\\.\DISPLAY1".into(), 2));
    assert_eq!(target.prepared_display_target(), (r"\\.\DISPLAY1".into(), 2));
    assert_eq!(target.current(), (r"\\.\DISPLAY2".into(), after_override_rev));

    // Clearing the override restores the overlaid window (or prepared display).
    assert!(target.set_monitor_override(None));
    assert!(!target.has_monitor_override());
    assert!(target.revision() > after_override_rev);
    assert_eq!(target.current(), (window, 0));
    assert_eq!(target.display_target(), (r"\\.\DISPLAY1".into(), 2));
}
#[test]
fn a_capped_streams_display_keeps_its_size_for_the_mode_matches_and_resumes() {
    use crate::state::test_support::Fixture;
    let mut stream = Negotiated {
        width: 2560,
        height: 1440,
        fps: 60,
        rate_millihz: 60_000,
        ..Default::default()
    };
    stream.set_display_size((3840, 2160), Some((2560, 1440)));
    assert_eq!(stream.width, 2560);
    // The session mode the display reports is the size the client asked for,
    // never the smaller picture the encoder codes.
    assert_eq!(
        super::stream_mode(&stream),
        (3840, 2160, 60_000, false, false)
    );

    let fixture = Fixture::new();
    let config = Config::parse("dd_config_revert_delay=0").unwrap();
    *fixture.host.config.write().unwrap() = config.clone();
    let mut prepared = test_prepared(&fixture.host);
    prepared.mode = super::stream_mode(&stream);
    let limiter =
        limiter::Lease::acquire(&fixture.host.directory, &config, &prepared.framegen).unwrap();
    let display = Ready::prepare((prepared, limiter)).unwrap().display.clone();
    // A display made at the request matches and resumes the capped stream...
    assert!(display.matches(&stream));
    assert!(display.resumes(&stream));
    // ...whereas the coded size (what a regression would compare) does not.
    let coded_only = Negotiated {
        width: 2560,
        height: 1440,
        fps: 60,
        rate_millihz: 60_000,
        ..Default::default()
    };
    assert!(!display.matches(&coded_only));
    assert!(!display.resumes(&coded_only));
}

#[test]
fn a_resume_that_cannot_apply_its_profile_keeps_the_lease_it_had() {
    // Acquiring fails with none held: still none, and the error is reported.
    let mut slot: Option<&str> = None;
    assert!(take_profile(&mut slot, Some("new"), |_, _| anyhow::bail!("refused")).is_err());
    assert_eq!(slot, None);
    // A switch that fails leaves the old lease in place instead of dropping it first.
    let mut slot = Some("old");
    let seen = std::cell::Cell::new(None);
    let result = take_profile(&mut slot, Some("new"), |held, _| {
        seen.set(held.map(|lease| *lease));
        anyhow::bail!("refused")
    });
    assert!(result.is_err());
    assert_eq!(
        seen.get(),
        Some("old"),
        "the old lease is still held while the new one is tried"
    );
    assert_eq!(slot, Some("old"), "and still held after the refusal");
    // A new lease replaces the old; none wanted takes it off.
    take_profile(&mut slot, Some("new"), |_, _| Ok(Some("new"))).unwrap();
    assert_eq!(slot, Some("new"));
    take_profile(&mut slot, None, |_, _| unreachable!()).unwrap();
    assert_eq!(slot, None);
}

#[test]
fn machine_phase_stays_in_lockstep_with_active_count_across_pause_recovery_and_multistream() {
    use butterpollo_media_policy::display_session_machine::SessionPhase;

    let fixture = crate::state::test_support::Fixture::new();
    let config = Config::parse("dd_config_revert_delay=0").unwrap();
    *fixture.host.config.write().unwrap() = config.clone();
    let prepared = test_prepared(&fixture.host);
    let limiter =
        limiter::Lease::acquire(&fixture.host.directory, &config, &prepared.framegen).unwrap();
    let first = Ready::prepare((prepared, limiter)).unwrap();
    let display = first.display.clone();

    let phase = || display._state.lock().unwrap().machine.phase;
    assert_eq!(phase(), SessionPhase::Active { streams: 1 });
    assert_eq!(display.active.load(Ordering::Acquire), 1);

    // Pause by dropping the only active transport.
    drop(first);
    assert_eq!(display.active.load(Ordering::Acquire), 0);
    assert_eq!(phase(), SessionPhase::Paused);

    // Simulate recovery running while paused (as Ready::resume's finish_feed does before attach).
    {
        let mut state = display._state.lock().unwrap();
        state.machine.begin_recovery(std::time::Instant::now());
        state.finish_feed(Ok(false)).unwrap();
        assert_eq!(state.machine.phase, SessionPhase::Paused);
    }

    // Resume attaches cleanly to Active { streams: 1 }, then a second concurrent resume to 2.
    let second = display
        .clone()
        .resume(
            &fixture.host.directory,
            &config,
            Arc::new(Warnings::default()),
        )
        .unwrap();
    assert_eq!(display.active.load(Ordering::Acquire), 1);
    assert_eq!(phase(), SessionPhase::Active { streams: 1 });

    let third = display
        .clone()
        .resume(
            &fixture.host.directory,
            &config,
            Arc::new(Warnings::default()),
        )
        .unwrap();
    assert_eq!(display.active.load(Ordering::Acquire), 2);
    assert_eq!(phase(), SessionPhase::Active { streams: 2 });

    // Recovery with 2 active streams preserves stream count across finish_feed and detach.
    {
        let mut state = display._state.lock().unwrap();
        state.machine.begin_recovery(std::time::Instant::now());
        assert_eq!(state.machine.phase, SessionPhase::Recovering { streams: 2 });
        state.finish_feed(Ok(false)).unwrap();
        assert_eq!(state.machine.phase, SessionPhase::Active { streams: 2 });
    }
    drop(second);
    assert_eq!(display.active.load(Ordering::Acquire), 1);
    assert_eq!(phase(), SessionPhase::Active { streams: 1 });
    drop(third);
    assert_eq!(display.active.load(Ordering::Acquire), 0);
    assert_eq!(phase(), SessionPhase::Paused);
}

#[test]
fn finish_feed_propagates_feed_error_before_keep_layout_and_resume_skips_premature_keep_layout() {
    use butterpollo_media_policy::display_policy::PrimaryChange;

    let fixture = crate::state::test_support::Fixture::new();
    let mut prepared = test_prepared(&fixture.host);
    // T2-15: feed error propagates immediately without consuming paused_change in keep_layout.
    prepared.machine.paused_change = Some(PrimaryChange::Display);
    let err = prepared
        .finish_feed(Err(anyhow::anyhow!("driver lease lost")))
        .unwrap_err();
    assert!(err.to_string().contains("driver lease lost"));
    assert_eq!(prepared.machine.paused_change, Some(PrimaryChange::Display));
    // T2-15: Ready::resume's `finish_feed_with(Ok(false), false)` preserves `paused_change`
    // until after `lay_out_again` runs.
    prepared.finish_feed_with(Ok(false), false).unwrap();
    assert_eq!(prepared.machine.paused_change, Some(PrimaryChange::Display));
}
