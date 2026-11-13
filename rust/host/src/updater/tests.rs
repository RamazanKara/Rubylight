use super::*;
struct Fixture {
    host: Shared,
    directory: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let directory =
            std::env::temp_dir().join(format!("butterpollo-update-test-{}", uuid::Uuid::new_v4()));
        let host =
            crate::state::Host::load(directory.clone(), directory.join("assets"), None).unwrap();
        Self { host, directory }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}
fn launch(role: butterpollo_core::session::Role) -> crate::state::Launch {
    crate::state::Launch {
        id: "fixture".into(),
        client: butterpollo_core::state::Client {
            name: "fixture".into(),
            cert: String::new(),
            uuid: "fixture".into(),
            perm: u32::MAX,
            enabled: true,
            extra: Default::default(),
        },
        peer: "127.0.0.1".parse().unwrap(),
        app_id: 1,
        background: None,
        key: [0; 16],
        key_id: 1,
        ping: "fixture".into(),
        connect_data: 1,
        role,
        created: std::time::Instant::now(),
        rtsp_encrypted: true,
        rtsp_counter: Default::default(),
        rtsp_received: Default::default(),
        preparation: Default::default(),
        vrr_requested: false,
        host_audio: false,
        requested_rate: 0,
        options: Default::default(),
        audio_preparation: Default::default(),
        preparing: Default::default(),
        transport: Default::default(),
        roaming_offered: Default::default(),
        claimed_until: None,
        nucleus: None,
        held: None,
        warnings: Default::default(),
    }
}
#[test]
fn connections_reset_the_idle_wait_including_during_downloads() {
    use butterpollo_core::{rtsp::Negotiated, session::Role};
    let f = Fixture::new();
    let h = &f.host;
    for role in [Role::Stream, Role::RemoteMonitor, Role::InputOnly] {
        for phase in ["waiting", "downloading", "ready"] {
            *h.updates.lock().unwrap() = json!({"phase":phase,"idle_since":now()-120});
            h.sessions.lock().unwrap().queue(launch(role)).unwrap();
            poll(h);
            assert_eq!(h.updates.lock().unwrap()["phase"], phase);
            assert_eq!(h.updates.lock().unwrap()["idle_since"], 0);
            h.sessions
                .lock()
                .unwrap()
                .start(launch(role), Negotiated::default())
                .unwrap();
            poll(h);
            assert_eq!(h.updates.lock().unwrap()["idle_since"], 0);
            h.sessions.lock().unwrap().active.clear();
            poll(h);
            assert!(h.updates.lock().unwrap()["idle_since"].as_u64().unwrap() > 0);
            assert_eq!(h.updates.lock().unwrap()["phase"], phase);
        }
    }
}
#[test]
fn a_devices_install_now_waits_for_another_devices_stream_but_not_its_own() {
    let f = Fixture::new();
    let h = &f.host;
    // "fixture" streams; the phone asked, so its install waits, even once
    // the download is done and a stream starts later.
    h.sessions
        .lock()
        .unwrap()
        .queue(launch(butterpollo_core::session::Role::Stream))
        .unwrap();
    for phase in ["waiting", "ready"] {
        *h.updates.lock().unwrap() =
            json!({"phase":phase,"idle_since":0,"now":true,"now_by":"phone"});
        poll(h);
        assert_eq!(h.updates.lock().unwrap()["phase"], phase);
        assert_eq!(
            status(h)["waiting_for"],
            "fixture is starting a stream.",
            "{phase}"
        );
    }
    // The asking device's own stream ends: on to the installer, which
    // this test host, no installed service, refuses.
    *h.updates.lock().unwrap() =
        json!({"phase":"ready","idle_since":0,"now":true,"now_by":"fixture"});
    assert_eq!(status(h)["waiting_for"], Value::Null);
    poll(h);
    assert_eq!(h.updates.lock().unwrap()["phase"], "failed");
    // So does the console's, whoever streams.
    *h.updates.lock().unwrap() = json!({"phase":"ready","idle_since":0,"now":true});
    poll(h);
    assert_eq!(h.updates.lock().unwrap()["phase"], "failed");
}
#[test]
fn install_now_again_says_whether_it_changed_anything() {
    let mut state = json!({"phase":"waiting","now":false});
    assert!(install_now_again(&mut state, Some("phone")));
    assert_eq!(
        (&state["now"], &state["now_by"]),
        (&json!(true), &json!("phone"))
    );
    // The same device again: nothing new, so no second update.started.
    assert!(!install_now_again(&mut state, Some("phone")));
    // The console's ends any stream.
    assert!(install_now_again(&mut state, None));
    assert_eq!(state["now_by"], Value::Null);
    assert!(!install_now_again(&mut state, Some("phone")));
    assert_eq!(state["now_by"], Value::Null);
    let mut installing = json!({"phase":"installing","now":false});
    assert!(!install_now_again(&mut installing, Some("phone")));
    assert_eq!(installing["now"], false);
}
#[test]
fn install_now_does_not_wait_for_an_idle_host() {
    let f = Fixture::new();
    let h = &f.host;
    h.sessions
        .lock()
        .unwrap()
        .queue(launch(butterpollo_core::session::Role::Stream))
        .unwrap();
    *h.updates.lock().unwrap() = json!({"phase":"ready","idle_since":0,"now":true});
    poll(h);
    // Past the idle gate, straight to the installer; this test host is
    // no installed service, so that step fails.
    let state = h.updates.lock().unwrap();
    assert_eq!(state["phase"], "failed");
    assert!(
        state["error"]
            .as_str()
            .unwrap()
            .contains("installed Windows service"),
        "{state}"
    );
    drop(state);
    // Without it the busy host keeps the update waiting.
    *h.updates.lock().unwrap() = json!({"phase":"ready","idle_since":0});
    poll(h);
    assert_eq!(h.updates.lock().unwrap()["phase"], "ready");
    assert_eq!(h.updates.lock().unwrap()["idle_since"], 0);
}
#[test]
fn the_installer_is_hashed_with_no_lock_held_and_checked_again_after() {
    let f = Fixture::new();
    let h = &f.host;
    let ready = json!({"phase":"ready","idle_since":0,"now":true,
                       "download_id":"a","installer_path":"C:\\setup.exe"});
    let hashes = std::cell::Cell::new(0);
    // Unchanged meanwhile: on to the installer, which this test host,
    // no installed service, refuses.
    *h.updates.lock().unwrap() = ready.clone();
    poll_with(h, None, &|path| {
        hashes.set(hashes.get() + 1);
        assert_eq!(path, std::path::Path::new("C:\\setup.exe"));
        assert!(h.launch_transition.try_lock().is_ok());
        assert!(h.updates.try_lock().is_ok());
        Ok((1, "00".into()))
    });
    assert_eq!(hashes.get(), 1);
    let state = h.updates.lock().unwrap().clone();
    assert_eq!(state["phase"], "failed");
    assert!(
        state["error"]
            .as_str()
            .unwrap()
            .contains("installed Windows service")
    );
    // Cancelled and downloaded again while it was hashed: nothing starts,
    // and the next poll hashes the new download.
    *h.updates.lock().unwrap() = ready.clone();
    poll_with(h, None, &|_| {
        hashes.set(hashes.get() + 1);
        h.updates.lock().unwrap()["download_id"] = json!("b");
        Ok((1, "00".into()))
    });
    assert_eq!(hashes.get(), 2);
    let state = h.updates.lock().unwrap().clone();
    assert_eq!(state["phase"], "ready");
    assert_eq!(state["download_id"], "b");
    // A busy host by then keeps the update waiting.
    *h.updates.lock().unwrap() = json!({"phase":"ready","idle_since":now()-3600,
                                        "download_id":"a","installer_path":"C:\\setup.exe"});
    poll_with(h, None, &|_| {
        h.sessions
            .lock()
            .unwrap()
            .queue(launch(butterpollo_core::session::Role::Stream))
            .unwrap();
        Ok((1, "00".into()))
    });
    let state = h.updates.lock().unwrap().clone();
    assert_eq!(state["phase"], "ready");
    assert_eq!(state["idle_since"], 0);
}
#[test]
fn cancellation_invalidates_a_download_and_stops_automatic_requeue() {
    let f = Fixture::new();
    let h = &f.host;
    *h.updates.lock().unwrap() =
        json!({"phase":"downloading","download_id":"old","latest_version":"2.1.0"});
    cancel(h).unwrap();
    let state = h.updates.lock().unwrap();
    assert_eq!(state["phase"], "idle");
    assert!(state["download_id"].is_null());
    assert_eq!(state["attempted_version"], "2.1.0");
    drop(state);
    h.updates.lock().unwrap()["phase"] = json!("installing");
    assert!(cancel(h).is_err());
}
#[test]
fn disabling_automatic_updates_cancels_an_automatic_queue_only() {
    let f = Fixture::new();
    let h = &f.host;
    for automatic in [true, false] {
        *h.updates.lock().unwrap() =
            json!({"phase":"waiting","automatic":automatic,"idle_since":0});
        poll(h);
        assert_eq!(
            h.updates.lock().unwrap()["phase"],
            if automatic { "idle" } else { "waiting" }
        );
    }
    assert!(!status(h)["install_supported"].as_bool().unwrap());
    assert_eq!(status(h)["update_check_interval"], 86400);
    h.config
        .write()
        .unwrap()
        .values
        .insert("update_check_interval".into(), "0".into());
    assert_eq!(status(h)["update_check_interval"], 0);
    assert!(queue(h, false, false, None).is_err());
}
fn set_interval(h: &Shared, seconds: &str) {
    h.config
        .write()
        .unwrap()
        .values
        .insert("update_check_interval".into(), seconds.into());
}
#[test]
fn the_schedule_follows_the_wall_clock_and_the_current_interval() {
    let day = 86400;
    let t = 1_000_000_000;
    // Off, or never checked.
    assert_eq!(next_check_at(0, t, 0, t), None);
    assert_eq!(next_check_at(-1, t, 0, t), None);
    assert_eq!(next_check_at(day, 0, 0, t), Some(0));
    // Not due until a whole interval after the last check; the console's
    // floor of an hour applies to a smaller value.
    assert_eq!(
        next_check_at(day, t - 100, 0, t),
        Some(t - 100 + day as u64)
    );
    assert_eq!(next_check_at(60, t - 100, 0, t), Some(t - 100 + 3600));
    // A PC that slept for days is overdue the moment it wakes.
    assert!(next_check_at(day, t - 3 * 86400, 0, t).unwrap() <= t);
    // A clock set back must not postpone the check by the difference.
    assert_eq!(next_check_at(day, t + 5000, 0, t), Some(0));
    // A step of a second or two (time sync) is not a clock set back.
    assert_eq!(next_check_at(day, t + 1, 0, t), Some(t + 1 + day as u64));
    assert_eq!(
        next_check_at(day, t + 300, 0, t),
        Some(t + 300 + day as u64)
    );
    assert_eq!(next_check_at(day, t + 301, 0, t), Some(0));
    // A failed check (network not back after sleep) is retried within
    // minutes, not after a whole interval.
    let retry = next_check_at(day, t - 10, 1, t).unwrap();
    assert!(retry > t && retry <= t + 300, "{retry}");
}
#[test]
fn repeated_failures_back_off_up_to_the_interval() {
    let t = 1_000_000_000;
    let day = 86400;
    let wait = |failures, interval| next_check_at(interval, t, failures, t + 1).unwrap() - t;
    assert_eq!(wait(1, day), 120);
    assert_eq!(wait(2, day), 240);
    assert_eq!(wait(3, day), 480);
    // Capped by the interval, and by the hour floor for a smaller one.
    assert_eq!(wait(20, day), day as u64);
    assert_eq!(wait(20, 7200), 7200);
    assert_eq!(wait(20, 60), 3600);
    // A huge count must not overflow the shift.
    assert_eq!(wait(u64::MAX, day), day as u64);
    // A success resets the count.
    assert_eq!(wait(0, day), day as u64);
}
#[test]
fn a_changed_interval_applies_without_a_restart_and_a_failed_check_is_retried() {
    let f = Fixture::new();
    let h = &f.host;
    // Checked two hours ago with the default daily interval: not due.
    let t = now();
    *h.updates.lock().unwrap() =
        json!({"checking":false,"check_failed":false,"checked_at":t - 7200});
    assert!(!check_due(h));
    assert_eq!(status(h)["next_check_at"], t - 7200 + 86400);
    // The user picks the minimum: due now, same running host.
    set_interval(h, "3600");
    assert!(check_due(h));
    // Turned off.
    set_interval(h, "0");
    assert!(!check_due(h));
    assert!(status(h)["next_check_at"].is_null());
    // A check in flight is not started again.
    set_interval(h, "3600");
    h.updates.lock().unwrap()["checking"] = json!(true);
    assert!(!check_due(h));
    // A check that failed a moment ago waits briefly; one that failed
    // minutes ago is retried even with a daily interval.
    set_interval(h, "86400");
    *h.updates.lock().unwrap() =
        json!({"checking":false,"check_failed":true,"check_failures":1,"checked_at":t - 10});
    assert!(!check_due(h));
    h.updates.lock().unwrap()["checked_at"] = json!(t - 600);
    assert!(check_due(h));
}
#[test]
fn a_stream_defers_the_check() {
    let f = Fixture::new();
    let h = &f.host;
    *h.updates.lock().unwrap() = json!({"checking":false,"checked_at":0});
    assert!(check_due(h));
    h.sessions
        .lock()
        .unwrap()
        .queue(launch(butterpollo_core::session::Role::Stream))
        .unwrap();
    assert!(!check_due(h));
    // Due but held back: the console says so.
    assert_eq!(status(h)["deferred"], true);
}
#[test]
fn a_lingering_game_or_monitor_does_not_defer_the_check() {
    let f = Fixture::new();
    let h = &f.host;
    *h.updates.lock().unwrap() = json!({"checking":false,"checked_at":0});
    // (A held virtual display is `busy` the same way, but a `Retained`
    // cannot be built without a display driver.)
    *h.current_app.lock().unwrap() = Some(
        crate::process::RunningApp::with_environment(
            &butterpollo_core::state::App::desktop(),
            Default::default(),
        )
        .unwrap(),
    );
    assert!(busy(h), "the install path still waits for an idle host");
    assert!(check_due(h));
    assert_eq!(status(h)["deferred"], false);
}
fn set_time(h: &Shared, time: &str) {
    h.config
        .write()
        .unwrap()
        .values
        .insert("update_check_time".into(), time.into());
}
/// The time this PC's clock showed `hours` ago, as the setting takes it.
fn hours_ago(hours: i64) -> String {
    let local = local_clock(now()) - hours * 3600;
    format!(
        "{:02}:{:02}",
        local.rem_euclid(86400) / 3600,
        local.rem_euclid(3600) / 60
    )
}
#[test]
fn a_time_of_day_decides_the_check_in_place_of_the_interval() {
    let f = Fixture::new();
    let h = &f.host;
    let t = now();
    let checked = |seconds_ago: u64| {
        *h.updates.lock().unwrap() =
            json!({"checking":false,"check_failed":false,"checked_at":t - seconds_ago});
    };
    assert!(status(h)["update_check_time"].is_null());
    // The time was four hours ago and the last check eight hours ago: due,
    // although the daily interval has not passed.
    let time = hours_ago(4);
    set_time(h, &time);
    checked(8 * 3600);
    assert!(check_due(h));
    assert_eq!(status(h)["update_check_time"], time);
    assert!(status(h)["next_check_at"].as_u64().unwrap() <= t);
    // Checked since: not again today, however short the interval.
    set_interval(h, "3600");
    checked(2 * 3600);
    assert!(!check_due(h));
    let next = status(h)["next_check_at"].as_u64().unwrap();
    assert!(next > t + 18 * 3600 && next < t + 22 * 3600, "{next}");
    // Without the time the interval decides again, same running host.
    set_time(h, "");
    assert!(check_due(h));
    assert!(status(h)["update_check_time"].is_null());
    // So does it with a time the host cannot read.
    for unreadable in ["3am", "25:00", "0300"] {
        set_time(h, unreadable);
        assert!(check_due(h), "{unreadable}");
        assert!(status(h)["update_check_time"].is_null(), "{unreadable}");
        assert_eq!(status(h)["next_check_at"], t - 2 * 3600 + 3600);
    }
    // An interval of 0 turns automatic checks off with a time as well.
    set_time(h, &time);
    set_interval(h, "0");
    checked(8 * 3600);
    assert!(!check_due(h));
    assert!(status(h)["next_check_at"].is_null());
    // A stream defers the daily check like any other.
    set_interval(h, "86400");
    assert!(check_due(h));
    h.sessions
        .lock()
        .unwrap()
        .queue(launch(butterpollo_core::session::Role::Stream))
        .unwrap();
    assert!(!check_due(h));
    assert_eq!(status(h)["deferred"], true);
}
#[test]
fn the_daily_check_counts_the_last_check_before_a_restart() {
    let f = Fixture::new();
    let h = &f.host;
    let t = now();
    let time = hours_ago(4);
    // Never checked: at once.
    set_time(h, &time);
    assert!(check_due(h));
    // A check two hours ago succeeded; then the host restarts.
    h.updates.lock().unwrap()["checked_at"] = json!(t - 2 * 3600);
    remember_check(h);
    let saved = butterpollo_core::state::load_json(&h.aliases_path, json!(null)).unwrap();
    assert_eq!(saved["root"][LAST_CHECK], t - 2 * 3600);
    let again =
        crate::state::Host::load(f.directory.clone(), f.directory.join("assets"), None).unwrap();
    assert_eq!(again.updates.lock().unwrap()["checked_at"], 0);
    // Today's check is done: the restarted host waits for tomorrow's time.
    set_time(&again, &time);
    assert!(!check_due(&again));
    assert!(status(&again)["next_check_at"].as_u64().unwrap() > t);
    // With an interval it checks at every start, as before.
    set_time(&again, "");
    assert!(check_due(&again));
    // A restart after the time passed catches the check up.
    again.aliases.lock().unwrap()["root"][LAST_CHECK] = json!(t - 8 * 3600);
    set_time(&again, &time);
    assert!(check_due(&again));
    // A check that failed in this run is retried by its own time, not
    // held back by the one remembered.
    again.aliases.lock().unwrap()["root"][LAST_CHECK] = json!(t - 2 * 3600);
    *again.updates.lock().unwrap() =
        json!({"checking":false,"check_failed":true,"check_failures":1,"checked_at":t - 600});
    assert!(check_due(&again));
}
#[tokio::test]
#[ignore = "downloads the official public installer; never executes it"]
async fn official_release_download_verifies_with_github_digest() -> Result<()> {
    let f = Fixture::new();
    let releases: Vec<Value> = super::releases(&client(Duration::from_secs(35))?)
        .await?
        .json()
        .await?;
    let candidate = releases
        .iter()
        .find_map(|r| installer(r).ok())
        .context("no release installer")?;
    f.host.updates.lock().unwrap()["download_id"] = json!("download-test");
    let path = f.directory.join("setup.exe");
    transfer(&f.host, "download-test", &candidate, &path).await?;
    assert_eq!(std::fs::metadata(&path)?.len(), candidate.size);
    eprintln!(
        "Verified {}: {} bytes, SHA256 {}",
        candidate.version, candidate.size, candidate.digest
    );
    Ok(())
}
fn release() -> Value {
    // Each build's own installer.
    #[cfg(not(feature = "nvidia-proprietary"))]
    let (repository, name) = ("Butterpollo", "butterpollo-setup-2.0.0-rc.7.exe");
    #[cfg(feature = "nvidia-proprietary")]
    let (repository, name) = ("Rubylight", "rubylight-nvidia-setup-2.0.0-rc.7.exe");
    json!({"tag_name":"2.0.0-rc.7","assets":[{"name":name,"browser_download_url":format!("https://github.com/RamazanKara/{repository}/releases/download/2.0.0-rc.7/{name}"),"size":32,"digest":format!("sha256:{}", "ab".repeat(32))}]})
}
#[test]
fn each_build_updates_only_from_its_own_installer() {
    let url = |name: &str| {
        format!("https://github.com/RamazanKara/Rubylight/releases/download/2.0.0-rc.7/{name}")
    };
    let mut both = release();
    both["assets"] = json!([]);
    for name in [
        "rubylight-setup-2.0.0-rc.7.exe",
        "rubylight-nvidia-setup-2.0.0-rc.7.exe",
    ] {
        let mut asset = release()["assets"][0].clone();
        asset["name"] = json!(name);
        asset["browser_download_url"] = json!(url(name));
        both["assets"].as_array_mut().unwrap().push(asset);
    }
    let chosen = installer(&both).unwrap().url;
    let own = if cfg!(feature = "nvidia-proprietary") {
        "rubylight-nvidia-setup-2.0.0-rc.7.exe"
    } else {
        "rubylight-setup-2.0.0-rc.7.exe"
    };
    assert_eq!(chosen, url(own));
    // A release without this build's installer offers no update instead of the other build.
    let mut other = both.clone();
    other["assets"]
        .as_array_mut()
        .unwrap()
        .retain(|asset| asset["name"] != own);
    assert!(installer(&other).is_err());
}
#[test]
fn an_unresolved_rollback_cannot_be_overwritten_by_another_update() -> Result<()> {
    let f = Fixture::new();
    let path = f.directory.join("update-result.json");
    for phase in ["installing", "recovery_failed"] {
        let record = json!({"version":"2.0.0-rc.22","phase":phase,
            "backup":f.directory.join("updates/transaction-1-2/previous")});
        butterpollo_core::state::write_json(&path, &record)?;
        assert!(check_recovery(&f.directory).is_err());
        assert_eq!(
            butterpollo_core::state::load_json(&path, Value::Null)?,
            record
        );
    }
    for phase in ["installed", "rolled_back", "failed", "superseded"] {
        butterpollo_core::state::write_json(&path, &json!({"phase":phase}))?;
        assert!(check_recovery(&f.directory).is_ok());
    }
    std::fs::write(&path, b"{")?;
    let error = check_recovery(&f.directory).unwrap_err().to_string();
    assert!(error.contains("Run the Rubylight installer"), "{error}");
    Ok(())
}
#[test]
fn a_setup_that_stops_early_reports_its_recorded_reason() -> Result<()> {
    use std::os::windows::process::ExitStatusExt;
    let f = Fixture::new();
    let path = f.directory.join("update-result.json");
    let exited = || Ok(std::process::ExitStatus::from_raw(1));
    // What this host writes before it starts setup.
    butterpollo_core::state::write_json(
        &path,
        &json!({"version":"v2.2.22","phase":"installing","started_at":1}),
    )?;
    let error = setup_failure(&f.directory, exited()).to_string();
    assert_eq!(
        error,
        "Installer exited before replacing this host (exit code 1). See the setup log."
    );
    let reason = "Rubylight cannot be updated in D:\\Games\\Rubylight: D:\\Games must be owned by Administrators, SYSTEM or TrustedInstaller.";
    butterpollo_core::state::write_json(
        &path,
        &json!({"version":"2.2.22","phase":"failed","error":reason}),
    )?;
    assert_eq!(setup_failure(&f.directory, exited()).to_string(), reason);
    Ok(())
}
#[tokio::test]
async fn busy_host_defers_download_before_contacting_the_network() {
    let f = Fixture::new();
    f.host
        .sessions
        .lock()
        .unwrap()
        .queue(launch(butterpollo_core::session::Role::Stream))
        .unwrap();
    let candidate = installer(&release()).unwrap();
    let error = transfer(&f.host, "test", &candidate, &f.directory.join("setup.exe"))
        .await
        .unwrap_err();
    assert!(error.is::<DownloadPaused>());
    assert!(!f.directory.join("setup.exe").exists());
    // A device's Install now pauses beside another device's stream too.
    *f.host.updates.lock().unwrap() = json!({"phase":"downloading","now":true,"now_by":"phone"});
    let error = transfer(&f.host, "test", &candidate, &f.directory.join("setup.exe"))
        .await
        .unwrap_err();
    assert!(error.is::<DownloadPaused>());
    assert!(!f.directory.join("setup.exe").exists());
}
#[test]
fn only_exact_release_assets_with_digests_can_be_installed() {
    assert!(installer(&release()).is_ok());
    for (key, value) in [
        (
            "browser_download_url",
            json!("https://example.org/setup.exe"),
        ),
        ("digest", Value::Null),
        ("size", json!(MAX_INSTALLER + 1)),
        ("name", json!("butterpollo-setup-2.0.0-rc.7-candidate.exe")),
    ] {
        let mut release = release();
        release["assets"][0][key] = value;
        assert!(installer(&release).is_err(), "{key}");
    }
}
#[test]
#[cfg(not(feature = "nvidia-proprietary"))]
fn installers_from_before_and_after_the_rename_are_accepted() {
    for (repository, name) in [
        ("Rubylight", "rubylight-setup-2.0.0-rc.7.exe"),
        ("Rubylight", "butterpollo-setup-2.0.0-rc.7.exe"),
        ("Butterpollo", "rubylight-setup-2.0.0-rc.7.exe"),
        ("Butterpollo", "butterpollo-setup-2.0.0-rc.7.exe"),
    ] {
        let url = format!(
            "https://github.com/RamazanKara/{repository}/releases/download/2.0.0-rc.7/{name}"
        );
        let mut release = release();
        release["assets"][0]["name"] = json!(name);
        release["assets"][0]["browser_download_url"] = json!(url);
        assert_eq!(installer(&release).unwrap().url, url);
    }
    // The current name wins when a release carries both.
    let mut both = release();
    let mut current = both["assets"][0].clone();
    current["name"] = json!("rubylight-setup-2.0.0-rc.7.exe");
    current["browser_download_url"] = json!(
        "https://github.com/RamazanKara/Rubylight/releases/download/2.0.0-rc.7/rubylight-setup-2.0.0-rc.7.exe"
    );
    both["assets"].as_array_mut().unwrap().push(current);
    assert!(
        installer(&both)
            .unwrap()
            .url
            .ends_with("/rubylight-setup-2.0.0-rc.7.exe")
    );
    // Another repository's copy is refused.
    let mut foreign = release();
    foreign["assets"][0]["browser_download_url"] = json!(
        "https://github.com/someone/Rubylight/releases/download/2.0.0-rc.7/butterpollo-setup-2.0.0-rc.7.exe"
    );
    assert!(installer(&foreign).is_err());
}
#[test]
fn only_the_two_newest_update_transactions_are_kept() {
    let names: Vec<String> = [
        "transaction-9876-1700000000000000300",
        "transaction-12-1700000000000000100",
        "transaction-4-1700000000000000400",
        "transaction-123456-1700000000000000200",
        "transaction-x-1700000000000000000",
        "transaction-5-",
        "backup",
    ]
    .map(String::from)
    .into();
    let mut stale = stale_transactions(&names);
    stale.sort_unstable();
    assert_eq!(
        stale,
        [
            "transaction-12-1700000000000000100",
            "transaction-123456-1700000000000000200"
        ]
    );
    assert!(stale_transactions(&names[..2]).is_empty());
}
#[test]
fn truncated_or_changed_installer_is_rejected() {
    let candidate = installer(&release()).unwrap();
    assert!(verify(32, &candidate.digest, &candidate).is_ok());
    assert!(verify(31, &candidate.digest, &candidate).is_err());
    assert!(verify(32, &"00".repeat(32), &candidate).is_err());
}
/// A temporary folder, removed with what is in it.
struct Temporary(PathBuf);
impl Temporary {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "butterpollo-download-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn elevated() -> bool {
    butterpollo_windows::process::elevated().unwrap_or(false)
}
/// The host downloaded setup.exe into ProgramData\Butterpollo\config\
/// updates and ran it elevated from there, which Defender scores as
/// malware. It downloads into the installation folder's staging folder,
/// admin-only from the start, and removes earlier downloads first.
#[test]
fn the_installer_is_downloaded_into_the_installation_folder() -> Result<()> {
    if !elevated() {
        return Ok(());
    }
    let temporary = Temporary::new();
    let install = temporary.0.join("Rubylight");
    std::fs::create_dir(&install)?;
    let staging = install.join(".staging");
    let earlier = staging.join(format!("download-{}", uuid::Uuid::new_v4()));
    let backup = staging.join("transaction-1-2").join("previous");
    std::fs::create_dir_all(&earlier)?;
    std::fs::create_dir_all(&backup)?;
    std::fs::write(earlier.join("setup.exe"), b"earlier")?;
    std::fs::write(backup.join("butterpollo.exe"), b"previous")?;
    let id = uuid::Uuid::new_v4().to_string();
    let directory = prepare_download(&install, &id)?;
    assert_eq!(directory, staging.join(format!("download-{id}")));
    assert!(!directory.starts_with(butterpollo_core::paths::installed_profile()));
    assert_eq!(std::fs::read_dir(&directory)?.count(), 0);
    assert!(!earlier.exists());
    assert_eq!(std::fs::read(backup.join("butterpollo.exe"))?, b"previous");
    std::fs::write(directory.join("setup.exe"), b"downloaded")?;
    // A failed or finished download goes; a failed update's backup stays.
    remove_download(&directory);
    assert!(!directory.exists());
    assert!(backup.exists());
    std::fs::remove_dir_all(staging.join("transaction-1-2"))?;
    let directory = prepare_download(&install, &uuid::Uuid::new_v4().to_string())?;
    remove_download(&directory);
    assert!(!staging.exists());
    assert!(install.exists());
    Ok(())
}
#[test]
fn a_link_at_or_in_the_staging_folder_is_refused_not_followed() -> Result<()> {
    if !elevated() {
        return Ok(());
    }
    let temporary = Temporary::new();
    let install = temporary.0.join("Rubylight");
    std::fs::create_dir(&install)?;
    let outside = temporary.0.join("outside");
    let planted = outside.join(format!("download-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&planted)?;
    std::fs::write(planted.join("setup.exe"), b"unchanged")?;
    let staging = install.join(".staging");
    for link in [staging.clone(), staging.join("package")] {
        if let Some(parent) = link.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::os::windows::fs::symlink_dir(&outside, &link)?;
        let error = prepare_download(&install, &uuid::Uuid::new_v4().to_string())
            .err()
            .context("a link was followed")?;
        assert!(format!("{error:#}").contains("reparse point"), "{error:#}");
        assert_eq!(std::fs::read(planted.join("setup.exe"))?, b"unchanged");
        std::fs::remove_dir(&link)?;
    }
    Ok(())
}
#[test]
fn a_download_a_setup_still_runs_from_is_left_for_later() -> Result<()> {
    use std::os::windows::fs::OpenOptionsExt;
    let staging = Temporary::new();
    let running = staging.0.join(format!("download-{}", uuid::Uuid::new_v4()));
    let done = staging.0.join(format!("download-{}", uuid::Uuid::new_v4()));
    for folder in [&running, &done] {
        std::fs::create_dir(folder)?;
        std::fs::write(folder.join("setup.exe"), b"setup")?;
    }
    for other in ["package", "transaction-1-2", "download-not-a-uuid"] {
        std::fs::create_dir(staging.0.join(other))?;
    }
    // As Windows holds a running program.
    let held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0x1)
        .open(running.join("setup.exe"))?;
    assert_eq!(remove_downloads(&staging.0), 1);
    assert!(!done.exists());
    assert!(running.join("setup.exe").exists());
    drop(held);
    assert_eq!(remove_downloads(&staging.0), 0);
    let mut left: Vec<String> = std::fs::read_dir(&staging.0)?
        .map(|entry| Ok(entry?.file_name().to_string_lossy().into_owned()))
        .collect::<Result<_>>()?;
    left.sort();
    assert_eq!(left, ["download-not-a-uuid", "package", "transaction-1-2"]);
    Ok(())
}
#[test]
fn downloads_left_in_the_profile_are_removed() -> Result<()> {
    let profile = Temporary::new();
    let updates = profile.0.join("updates");
    let download = updates.join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&download)?;
    std::fs::write(download.join("setup.exe"), b"earlier")?;
    let transactions = [
        "transaction-1-100",
        "transaction-2-400",
        "transaction-3-300",
        "transaction-4-200",
    ];
    for name in transactions {
        std::fs::create_dir_all(updates.join(name).join("previous"))?;
    }
    std::fs::create_dir(updates.join("other"))?;
    remove_legacy_downloads(&updates);
    let mut left: Vec<String> = std::fs::read_dir(&updates)?
        .map(|entry| Ok(entry?.file_name().to_string_lossy().into_owned()))
        .collect::<Result<_>>()?;
    left.sort();
    assert_eq!(left, ["other", "transaction-2-400", "transaction-3-300"]);
    for name in left {
        std::fs::remove_dir_all(updates.join(name))?;
    }
    std::fs::create_dir(&download)?;
    remove_legacy_downloads(&updates);
    assert!(!updates.exists());
    if elevated() {
        // A link there goes itself; what it leads to stays.
        let outside = profile.0.join("outside");
        std::fs::create_dir_all(&updates)?;
        std::fs::create_dir(&outside)?;
        std::fs::write(outside.join("file"), b"unchanged")?;
        std::os::windows::fs::symlink_dir(&outside, &download)?;
        remove_legacy_downloads(&updates);
        assert!(std::fs::symlink_metadata(&download).is_err());
        assert_eq!(std::fs::read(outside.join("file"))?, b"unchanged");
    }
    Ok(())
}
