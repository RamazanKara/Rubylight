//! Tests for tray icon retry logging, console URLs, and launch error notices.
use super::*;

#[test]
fn a_refused_tray_icon_is_logged_once_and_again_when_it_keeps_failing() {
    let start = Instant::now();
    let mut log = AddLog::default();
    assert!(!log.is_added());
    assert_eq!(log.attempt(false, start), Some(AddReport::Refused));
    // The 3 s retries say nothing more until a minute has passed, then once.
    assert_eq!(log.attempt(false, start + Duration::from_secs(3)), None);
    assert_eq!(
        log.attempt(false, start + ADD_REFUSED_LONG),
        Some(AddReport::StillRefused)
    );
    log.note_still_refused(true);
    assert_eq!(log.attempt(false, start + Duration::from_secs(120)), None);
    assert_eq!(
        log.attempt(true, start + Duration::from_secs(123)),
        Some(AddReport::Added(Some(Duration::from_secs(123))))
    );
    assert!(log.is_added());
    // Explorer restarting adds it again: no line; a later refusal is new.
    assert_eq!(log.attempt(true, start + Duration::from_secs(200)), None);
    assert_eq!(
        log.attempt(false, start + Duration::from_secs(300)),
        Some(AddReport::Refused)
    );
    assert!(!log.is_added());
}

#[test]
fn taskbar_creation_restarts_retries_only_when_it_gave_up_without_a_taskbar() {
    let start = Instant::now();
    let mut log = AddLog::default();
    assert_eq!(log.attempt(false, start), Some(AddReport::Refused));
    assert_eq!(
        log.attempt(false, start + ADD_REFUSED_LONG),
        Some(AddReport::StillRefused)
    );
    log.note_still_refused(false);
    log.on_taskbar_created();
    assert_eq!(
        log.attempt(false, start + Duration::from_secs(120)),
        Some(AddReport::Refused)
    );
    assert_eq!(
        log.attempt(false, start + Duration::from_secs(180)),
        Some(AddReport::StillRefused)
    );
    // Once it gave up with the taskbar present, another TaskbarCreated does not warn again,
    // yet still resets the retry budget so the new taskbar gets fresh attempts instead of giving
    // up after a single try.
    log.note_still_refused(true);
    log.on_taskbar_created();
    assert_eq!(log.attempt(false, start + Duration::from_secs(240)), None);
    assert!(!log.gave_up(start + Duration::from_secs(240)));
    assert!(log.gave_up(start + Duration::from_secs(240) + ADD_REFUSED_LONG));
}

#[test]
fn the_first_tray_icon_added_is_logged() {
    let mut log = AddLog::default();
    assert_eq!(
        log.attempt(true, Instant::now()),
        Some(AddReport::Added(None))
    );
    assert_eq!(log.attempt(true, Instant::now()), None);
}

#[test]
fn the_pairing_pages_lead_to_the_devices_page() {
    assert_eq!(console_url(47990, "/"), "https://localhost:47990/");
    assert_eq!(
        console_url(47990, PAGE_ENTER_PIN),
        "https://localhost:47990/devices?pair=pin"
    );
    assert_eq!(
        console_url(47990, PAGE_PAIR_QR),
        "https://localhost:47990/devices?pair=qr"
    );
    assert_eq!(
        console_url(47990, PAGE_BACKUP),
        "https://localhost:47990/troubleshoot#backup"
    );
    assert!(open_web_page(47990, "//evil.example/").is_err());
}

#[test]
fn launch_error_says_what_happened_and_what_to_do_with_a_signed_exit_code() {
    assert_eq!(
        launch_error_notice("Game", 1),
        (
            "Launch failed",
            "Game closed right after it started (exit code 1). Click to end the stream.".into()
        )
    );
    assert!(
        launch_error_notice("Game", 0xc0000005)
            .1
            .contains("(exit code -1073741819).")
    );
}

#[test]
fn a_failed_launch_shows_while_its_app_still_runs() {
    let shown = |app: Option<&str>, failed: Option<&str>| Shown {
        icon: if app.is_some() {
            Icon::Streaming
        } else {
            Icon::Idle
        },
        app: app.map(str::to_owned),
        summary: None,
        failed: failed.map(str::to_owned),
    };
    assert_eq!(shown(Some("Game"), Some("Game")).status(), Status::Error);
    assert_eq!(
        shown(Some("Other"), Some("Game")).status(),
        Status::Streaming
    );
    assert_eq!(shown(None, Some("Game")).status(), Status::Idle);
    assert_eq!(shown(Some("Game"), None).status(), Status::Streaming);
}
