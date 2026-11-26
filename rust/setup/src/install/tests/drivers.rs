//! Tests for virtual display, gamepad and USBip driver setup and repair.
use super::super::*;

#[test]
fn driver_reboot_exit_codes_are_successful_but_warnings_are_shown() {
    let mut notes = Vec::new();
    assert!(driver_result(3010, "", &mut notes, "display"));
    assert!(notes.is_empty());
    assert!(!driver_result(0, "", &mut notes, "gamepad"));
    assert!(!driver_result(
        0,
        "VIRTUAL_DISPLAY_DRIVER_WARNING",
        &mut notes,
        "display"
    ));
    assert_eq!(notes.len(), 1);
    assert!(!driver_result(1, "failed", &mut notes, "gamepad"));
    assert_eq!(notes.len(), 2);
    // The script's own reason reaches the note.
    driver_result(
        0,
        "WARNING: [SunshineVirtualDisplay] VIRTUAL_DISPLAY_DRIVER_WARNING: Optional virtual display driver setup did not complete.\nWARNING: [SunshineVirtualDisplay] Installer best-effort driver action failed: nefconc.exe failed with exit code 5.\n",
        &mut notes,
        "virtual display",
    );
    assert!(
        notes[2].starts_with(
            "The virtual display driver could not be set up: nefconc.exe failed with exit code 5."
        ),
        "{}",
        notes[2]
    );
    assert!(driver_result(
        1,
        "RESTART_REQUIRED\nUSBIP_DRIVER_WARNING: Restart Windows or repair USBip separately.",
        &mut notes,
        "USB audio and haptics",
    ));
    assert!(notes[3].contains("Run setup again with USB audio and haptics selected"));
    assert!(!notes[3].contains("when its service starts"));
}
#[test]
fn a_driver_script_past_the_deadline_is_not_reported_as_failed() {
    let late = anyhow::Error::from(system::StillRunning {
        program: "powershell.exe".into(),
        seconds: 600,
    });
    let note = driver_error_note(&late, "USB audio and haptics");
    assert!(
        note.starts_with(
            "The USB audio and haptics driver was still being set up after 600 s and goes on in the background."
        ),
        "{note}"
    );
    assert!(!note.contains("could not"), "{note}");
    // The task's own log is removed at the deadline, so its result
    // never reaches the setup log.
    assert!(!note.contains("setup log"), "{note}");
    let late = late.context("waiting for the task");
    assert!(driver_error_note(&late, "virtual gamepad").contains("still being set up"));
    let failed = anyhow::anyhow!("creating the setup task failed (1)");
    assert_eq!(
        driver_error_note(&failed, "virtual gamepad"),
        "The virtual gamepad driver could not be set up: creating the setup task failed (1)"
    );
}

#[test]
fn a_pc_that_refuses_the_locally_signed_driver_gets_a_note_naming_why() {
    let mut notes = Vec::new();
    let output = "WARNUNG: [RubylightVirtualDisplay] DRIVER_UNSUPPORTED: This PC refuses a display driver signed on the PC (Smart App Control is on). A signed Rubylight release installs it; until then streams use a physical display.\n";
    assert!(!driver_result(0, output, &mut notes, "virtual display"));
    assert_eq!(
        notes,
        [
            "The virtual display driver was not installed: This PC refuses a display driver signed on the PC (Smart App Control is on). A signed Rubylight release installs it; until then streams use a physical display."
        ]
    );
}
#[test]
fn libvirtualdisplay_goes_only_once_rubylights_device_runs() {
    assert!(libvirtualdisplay_replaced(false, false, false));
    // Smart App Control, S mode, App Control or a failed install: the
    // driver step added a note, and the old driver stays.
    let mut notes = Vec::new();
    let refused = "WARNING: [RubylightVirtualDisplay] DRIVER_UNSUPPORTED: Smart App Control is on, and it refuses a display driver signed on this PC, so streams use a physical display.\n";
    let restart = driver_result(0, refused, &mut notes, "virtual display");
    assert!(!libvirtualdisplay_replaced(
        false,
        !notes.is_empty(),
        restart
    ));
    let mut notes = Vec::new();
    let failed = "WARNING: [RubylightVirtualDisplay] DRIVER_WARNING: Rubylight display driver setup did not complete.\nWARNING: [RubylightVirtualDisplay] Installer best-effort driver action failed: Windows did not start the display driver (Error, problem 31)\n";
    let restart = driver_result(0, failed, &mut notes, "virtual display");
    assert!(!libvirtualdisplay_replaced(
        false,
        !notes.is_empty(),
        restart
    ));
    // Installed, but the device starts only after a restart.
    let mut notes = Vec::new();
    let restart = driver_result(3010, "RESTART_REQUIRED\n", &mut notes, "virtual display");
    assert!(!libvirtualdisplay_replaced(
        false,
        !notes.is_empty(),
        restart
    ));
}
/// A kept host may still use SudoVDA: its driver and HDR layer stay
/// through every run, even with Rubylight's own device up and running.
#[test]
fn side_by_side_never_replaces_libvirtualdisplay() {
    assert!(!libvirtualdisplay_replaced(true, false, false));
    let display_step = include_str!("../drivers.rs")
        .split("fn display_step(")
        .nth(1)
        .and_then(|rest| rest.split("/// The installer of").next())
        .unwrap();
    assert!(display_step.contains("detect::side_by_side()"));
}

/// Rubylight's own virtual gamepad driver fails or gets refused -> becomes a setup note,
/// and the earlier gamepad driver stays installed until replacement succeeds without reboot.
#[test]
fn a_failed_rubylight_gamepad_install_becomes_a_setup_note() {
    let mut notes = Vec::new();
    let output = "WARNING: [RubylightVirtualGamepad] DRIVER_WARNING: Rubylight virtual gamepad driver setup did not complete.\nWARNING: [RubylightVirtualGamepad] Installer best-effort driver action failed: pnputil /add-driver failed with exit code 5\n";
    assert!(!driver_result(0, output, &mut notes, "virtual gamepad"));
    assert_eq!(
        notes,
        [
            "The virtual gamepad driver could not be set up: pnputil /add-driver failed with exit code 5 The Rubylight service tries again within a day, or run \"butterpollo-setup.exe --repair-drivers --now\" in an administrator shell; the setup log has the details."
        ]
    );
    assert!(!libvirtualdisplay_replaced(false, true, false));
    assert!(!libvirtualdisplay_replaced(false, false, true));
    assert!(!libvirtualdisplay_replaced(true, false, false));
    assert!(libvirtualdisplay_replaced(false, false, false));
}

#[test]
fn a_failed_libvirtualgamepad_removal_is_a_note_about_removing_it() {
    use crate::libvirtualdisplay::Removal;
    let failed = Removal {
        restart: false,
        error: Some(anyhow::anyhow!(
            "pnputil /delete-driver oem9.inf /uninstall /force failed with exit code 5"
        )),
    };
    assert_eq!(
        driver_removal_note(&failed, LIBVIRTUALGAMEPAD).as_deref(),
        Some(
            "The earlier virtual gamepad driver could not be removed: pnputil /delete-driver oem9.inf /uninstall /force failed with exit code 5 Setup tries again when it next runs; the setup log has the details."
        )
    );
    let later = Removal {
        restart: true,
        error: None,
    };
    assert_eq!(driver_removal_note(&later, LIBVIRTUALGAMEPAD), None);
}
#[test]
fn a_failed_libvirtualdisplay_removal_is_a_note_about_removing_it() {
    use crate::libvirtualdisplay::Removal;
    let failed = Removal {
        restart: false,
        error: Some(anyhow::anyhow!(
            "pnputil /delete-driver oem7.inf /uninstall /force failed with exit code 5"
        )),
    };
    assert_eq!(
        libvirtualdisplay_note(&failed).as_deref(),
        Some(
            "The old libvirtualdisplay driver could not be removed: pnputil /delete-driver oem7.inf /uninstall /force failed with exit code 5 Setup tries again when it next runs; the setup log has the details."
        )
    );
    // A display still in use goes at the next restart, without a note.
    let later = Removal {
        restart: true,
        error: None,
    };
    assert_eq!(libvirtualdisplay_note(&later), None);
}
#[test]
fn a_failed_rubylight_display_install_becomes_a_setup_note() {
    let mut notes = Vec::new();
    let output = "WARNING: [RubylightVirtualDisplay] DRIVER_WARNING: Rubylight display driver setup did not complete.\nWARNING: [RubylightVirtualDisplay] Installer best-effort driver action failed: pnputil /add-driver failed with exit code 5\n";
    assert!(!driver_result(0, output, &mut notes, "Rubylight display"));
    assert_eq!(
        notes,
        [
            "The Rubylight display driver could not be set up: pnputil /add-driver failed with exit code 5 The Rubylight service tries again within a day, or run \"butterpollo-setup.exe --repair-drivers --now\" in an administrator shell; the setup log has the details."
        ]
    );
    let mut notes = Vec::new();
    assert!(driver_result(
        0,
        "RESTART_REQUIRED\n",
        &mut notes,
        "Rubylight display"
    ));
    assert!(notes.is_empty());
}
#[test]
fn the_repair_record_reads_old_and_unreadable_files() {
    use Driver::*;
    let read = |text: &str| read_record(Some(text.as_bytes()));
    // Missing, unreadable or not an object: nothing holds a repair back.
    for record in [
        read_record(None),
        read("not json"),
        read("null"),
        read("[1]"),
    ] {
        assert_eq!(record, serde_json::json!({}));
        for driver in Driver::ALL {
            assert_eq!(
                repair_skipped(&record, driver, "7.0.0", 100, RepairRun::Service),
                None
            );
        }
    }
    // What setups before the gamepad driver's entry wrote: the display
    // driver's at the top level, and nothing for the gamepad driver.
    let declined = read(r#"{"declined":true}"#);
    assert!(repair_skipped(&declined, Display, "7.0.0", 100, RepairRun::Service).is_some());
    assert_eq!(
        repair_skipped(&declined, Gamepad, "7.0.0", 100, RepairRun::Service),
        None
    );
    let tried = read(r#"{"version":"7.0.0","unix":86400}"#);
    assert!(repair_skipped(&tried, Display, "7.0.0", 86_460, RepairRun::Service).is_some());
    assert_eq!(
        repair_skipped(&tried, Gamepad, "7.0.0", 86_460, RepairRun::Service),
        None
    );
    // The gamepad driver's attempt keeps the display driver's entry where
    // those setups read it, and the other way round.
    let both = record_attempt(tried, Gamepad, "7.0.0", 90_000);
    assert_eq!(
        both,
        serde_json::json!({"version":"7.0.0","unix":86400,"gamepad":{"version":"7.0.0","unix":90000}})
    );
    let both = record_attempt(both, Display, "7.0.1", 95_000);
    assert_eq!(both["version"], "7.0.1");
    assert_eq!(both["gamepad"]["unix"], 90_000);
    // A repair a person asked for ends the choice to leave a driver out.
    let display = record_attempt(declined, Display, "7.0.0", 1);
    assert!(display.get("declined").is_none());
    let gamepad = record_attempt(
        read(r#"{"declined":true,"gamepad":{"declined":true}}"#),
        Gamepad,
        "7.0.0",
        1,
    );
    assert_eq!(gamepad["declined"], true);
    assert!(gamepad["gamepad"].get("declined").is_none());
}
#[test]
fn each_driver_is_repaired_once_a_day_per_version_on_its_own() {
    use Driver::*;
    let day = 86_400;
    let version = "2.0.0-rc.27";
    let skipped = |record: &serde_json::Value, driver, now| {
        repair_skipped(record, driver, version, now, RepairRun::Service)
    };
    // The service's run at second `day` repairs the display driver.
    let mut record = read_record(None);
    assert_eq!(skipped(&record, Display, day), None);
    record = record_attempt(record, Display, version, day);
    // The same day the display driver waits, the gamepad driver does not.
    assert!(skipped(&record, Display, day + 60).is_some());
    assert_eq!(skipped(&record, Gamepad, day + 60), None);
    record = record_attempt(record, Gamepad, version, day + 60);
    assert!(skipped(&record, Gamepad, day + 120).is_some());
    // A day after each attempt, each may go again, on its own clock.
    assert_eq!(skipped(&record, Display, 2 * day), None);
    assert!(skipped(&record, Gamepad, 2 * day).is_some());
    assert_eq!(skipped(&record, Gamepad, 2 * day + 60), None);
    // A new version may try again the same day.
    for driver in Driver::ALL {
        assert_eq!(
            repair_skipped(
                &record,
                driver,
                "2.0.0-rc.28",
                day + 120,
                RepairRun::Service
            ),
            None
        );
    }
    // A person running --repair-drivers --now is never held back by an
    // automatic attempt that failed earlier the same day, nor by a choice
    // made at install; --repair is not held back by the day either, but
    // keeps a driver left out at install out.
    let declined = read_record(Some(
        &br#"{"declined":true,"gamepad":{"declined":true}}"#[..],
    ));
    for driver in Driver::ALL {
        for record in [&record, &declined] {
            assert_eq!(
                repair_skipped(record, driver, version, day + 120, RepairRun::ByHand),
                None
            );
        }
        assert_eq!(
            repair_skipped(&record, driver, version, day + 120, RepairRun::Repair),
            None
        );
        for run in [RepairRun::Service, RepairRun::Repair] {
            assert!(
                repair_skipped(&declined, driver, version, day, run)
                    .unwrap()
                    .contains("left out with --no-")
            );
        }
    }
}
#[test]
fn an_installation_records_only_the_drivers_it_left_out() {
    use Driver::*;
    assert_eq!(choices(true, true), None);
    let display = choices(false, true).unwrap();
    assert_eq!(display, serde_json::json!({"declined":true}));
    let gamepad = choices(true, false).unwrap();
    for (record, left_out) in [
        (display, [true, false]),
        (gamepad, [false, true]),
        (choices(false, false).unwrap(), [true, true]),
    ] {
        for (driver, left_out) in [Display, Gamepad].into_iter().zip(left_out) {
            let skipped = repair_skipped(&record, driver, "7.0.0", 1, RepairRun::Service);
            assert_eq!(skipped.is_some(), left_out, "{record} {driver:?}");
        }
    }
    assert_eq!(Driver::parse("Gamepad"), Some(Gamepad));
    assert_eq!(Driver::parse("display"), Some(Display));
    assert_eq!(Driver::parse("usbip"), None);
}
#[test]
fn the_unattended_repair_needs_a_positive_idle_answer() {
    let idle = |counts: [&str; 3]| {
        Ok(format!(
            "<RustHostSessionCount>{}</RustHostSessionCount><RustHostPendingSessionCount>{}</RustHostPendingSessionCount><RustHostApplicationActive>{}</RustHostApplicationActive>",
            counts[0], counts[1], counts[2]
        ))
    };
    let refused = |answer: std::io::Result<String>, running| {
        ensure_idle_positive(&answer, || running, false).is_err()
    };
    assert!(!refused(idle(["0", "0", "0"]), true));
    // A stream, a queued launch or a running app.
    assert!(refused(idle(["1", "0", "0"]), true));
    assert!(refused(idle(["0", "1", "0"]), true));
    assert!(refused(idle(["0", "0", "1"]), true));
    // A LAN caller sees blank counts, an old host none: it may stream.
    assert!(refused(idle(["", "", ""]), false));
    assert!(refused(idle(["0", "0", ""]), false));
    assert!(refused(Ok("<root/>".into()), false));
    assert!(refused(Ok(String::new()), false));
    // No answer: busy while a host process runs, idle when none does.
    let silent = || Err(std::io::Error::from(std::io::ErrorKind::TimedOut));
    assert!(refused(silent(), true));
    assert!(!refused(silent(), false));
    // --now refuses the same streams and a host that runs but is silent,
    // but takes blank counts (a LAN bind_address blanks them for this
    // PC too), which would otherwise leave that host unrepairable.
    let by_hand = |answer: std::io::Result<String>, running| {
        ensure_idle_positive(&answer, || running, true).is_err()
    };
    assert!(by_hand(idle(["1", "0", "0"]), true));
    assert!(by_hand(idle(["0", "0", "1"]), true));
    assert!(by_hand(silent(), true));
    assert!(!by_hand(silent(), false));
    assert!(!by_hand(idle(["0", "0", "0"]), true));
    assert!(!by_hand(idle(["", "", ""]), true));
}
#[test]
fn a_repair_that_finds_the_driver_lock_taken_does_not_report_success() {
    // The service's run waits for the service to be restarted and asks
    // again; a person is told nothing was repaired.
    assert!(!when_locked(false).unwrap());
    let error = when_locked(true).unwrap_err().to_string();
    assert_eq!(
        error,
        "Setup is installing the drivers right now; try again when it finishes."
    );
}
#[test]
fn only_a_drivers_own_notes_fail_the_repair() {
    let notes = [
        "The virtual display driver could not be set up: pnputil failed".to_owned(),
        "The old libvirtualdisplay driver could not be removed: busy".to_owned(),
        "The virtual gamepad driver could not be set up: pnputil failed".to_owned(),
    ];
    // The display step added the first one: that is the failure.
    assert_eq!(step_notes(&notes, 0, 1), &notes[..1]);
    // Only the removal warned: the driver is repaired, the note stays.
    assert!(step_notes(&notes, 1, 0).is_empty());
    // The gamepad step, after both of the display step's.
    assert_eq!(step_notes(&notes, 2, 1), &notes[2..]);
    assert!(step_notes(&[], 0, 0).is_empty());
    assert!(step_notes(&notes, 5, 2).is_empty());
}
#[test]
fn a_person_is_told_which_drivers_were_set_up_again() {
    use Driver::*;
    let told = |repaired: Vec<Driver>, restart| DriverRepair { restart, repaired }.text();
    assert_eq!(
        told(vec![Display], false),
        "The virtual display driver was set up again."
    );
    assert_eq!(
        told(vec![Gamepad], true),
        "The virtual gamepad driver was set up again. Windows must restart to finish."
    );
    assert_eq!(
        told(vec![Display, Gamepad], false),
        "The virtual display and gamepad drivers were set up again."
    );
    assert!(told(Vec::new(), false).contains("left out at install stays out"));
}
