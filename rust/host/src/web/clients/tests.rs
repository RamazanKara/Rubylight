use super::handle;
use crate::state::test_support::Fixture;
use butterpollo_core::power_action::SleepStates;
use serde_json::json;
use std::time::Duration;

/// What the console grants an ordinary device.
const PERM: u32 = 0x071f1f00;

#[test]
fn turning_a_device_off_gives_up_its_radeon_settings_but_removing_a_permission_does_not() {
    let f = Fixture::new();
    let client = f.client(PERM);
    f.host.radeon.note_written(&client.uuid, &[]);
    // Removing a permission ends what the device is doing, not its claim on its settings:
    // they go back with the session.
    handle(
        &f.host,
        "POST",
        "/api/clients/update",
        &json!({"uuid": client.uuid, "perm": 0}),
    )
    .unwrap();
    assert!(f.host.radeon.holds(&client.uuid));
    handle(
        &f.host,
        "POST",
        "/api/clients/update",
        &json!({"uuid": client.uuid, "enabled": false}),
    )
    .unwrap();
    assert!(!f.host.radeon.holds(&client.uuid));
}

#[test]
fn forgetting_a_kept_resolution_forgets_the_second_screens_too() {
    use butterpollo_core::state::{REMEMBERED_RESOLUTION, screen_setting};
    let f = Fixture::new();
    let client = f.client(PERM);
    let second = screen_setting(
        &butterpollo_core::state::second_screen_uuid(&client.uuid),
        REMEMBERED_RESOLUTION,
    );
    {
        let extra = &mut f.host.paired.write().unwrap().clients[0].extra;
        extra.insert(REMEMBERED_RESOLUTION.into(), json!("2560x1440"));
        extra.insert(second.clone(), json!("1920x1080"));
    }
    handle(
        &f.host,
        "POST",
        "/api/clients/update",
        &json!({"uuid": client.uuid, "remembered_resolution": null}),
    )
    .unwrap();
    let extra = &f.host.paired.read().unwrap().clients[0].extra;
    assert!(!extra.contains_key(REMEMBERED_RESOLUTION));
    assert!(!extra.contains_key(&second));
}

#[test]
fn turning_off_or_unpairing_a_device_gives_up_its_second_screens_radeon_settings_too() {
    for data in [json!({"enabled": false}), json!(null)] {
        let f = Fixture::new();
        let client = f.client(PERM);
        let second = butterpollo_core::state::second_screen_uuid(&client.uuid);
        f.host.radeon.note_written(&second, &[]);
        let path = if data.is_null() {
            "/api/clients/unpair"
        } else {
            "/api/clients/update"
        };
        let mut data = if data.is_null() { json!({}) } else { data };
        data["uuid"] = json!(client.uuid);
        handle(&f.host, "POST", path, &data).unwrap();
        assert!(!f.host.radeon.holds(&second), "{path}");
    }
}

#[test]
fn turning_off_disconnecting_or_unpairing_a_device_ends_its_second_screen_too() {
    use butterpollo_core::session::Role;
    for (path, data) in [
        ("/api/clients/update", json!({"enabled": false})),
        ("/api/clients/update", json!({"perm": 0})),
        ("/api/clients/disconnect", json!({})),
        ("/api/clients/unpair", json!({})),
    ] {
        let f = Fixture::new();
        let client = f.client(PERM);
        let first = f.session(client.clone(), Role::Stream);
        let second = f.session(client.clone().second_screen(), Role::Stream);
        let mut other = client.clone();
        other.uuid = "other-device".into();
        let theirs = f.session(other, Role::Stream);
        // The device streams only its second screen: it is connected all the same.
        first.stop();
        let listed = handle(&f.host, "GET", "/api/clients/list", &json!({})).unwrap();
        assert_eq!(listed["clients"][0]["connected"], true, "{path} {data}");
        let mut data = data;
        data["uuid"] = json!(client.uuid);
        handle(&f.host, "POST", path, &data).unwrap();
        assert!(second.stopping(), "{path} {data}");
        assert!(!theirs.stopping(), "{path} {data}");
    }
}

#[test]
fn a_name_from_before_direction_characters_were_refused_can_stay_but_not_arrive() {
    let f = Fixture::new();
    let client = f.client(PERM);
    let old = "TV\u{202e}1";
    f.host.paired.write().unwrap().clients[0].name = old.into();
    let update = |name: &str, perm: u32| {
        handle(
            &f.host,
            "POST",
            "/api/clients/update",
            &json!({"uuid": client.uuid, "name": name, "perm": perm}),
        )
    };
    update(old, butterpollo_core::state::LATER_DEVICE_PERMISSION).unwrap();
    let state = f.host.paired.read().unwrap();
    assert_eq!(
        state.clients[0].perm,
        butterpollo_core::state::LATER_DEVICE_PERMISSION
    );
    assert_eq!(state.clients[0].name, old);
    drop(state);
    assert!(
        update(
            "TV\u{202e}2",
            butterpollo_core::state::LATER_DEVICE_PERMISSION
        )
        .is_err()
    );
    update("TV", butterpollo_core::state::LATER_DEVICE_PERMISSION).unwrap();
}

#[test]
fn device_connection_commands_are_checked_before_they_are_saved() {
    let f = Fixture::new();
    let client = f.client(PERM);
    let update = |change: serde_json::Value| {
        let mut data = change;
        data["uuid"] = json!(client.uuid);
        handle(&f.host, "POST", "/api/clients/update", &data)
    };
    let stored = |key: &str| {
        f.host
            .paired
            .read()
            .unwrap()
            .clients
            .iter()
            .find(|c| c.uuid == client.uuid)
            .and_then(|c| c.extra.get(key).cloned())
    };
    let commands = json!([{"cmd": "lights on", "elevated": true}]);
    update(json!({"do": commands, "undo": [{"cmd": "lights off"}]})).unwrap();
    assert_eq!(stored("do"), Some(commands.clone()));
    for bad in [
        json!({"do": [{"elevated": true}]}),
        json!({"undo": "lights off"}),
        json!({"do": commands, "undo": [{"cmd": null}]}),
    ] {
        assert!(update(bad.clone()).is_err(), "{bad}");
    }
    // A refused update changes nothing.
    assert_eq!(stored("undo"), Some(json!([{"cmd": "lights off"}])));
    update(json!({"do": null, "undo": null})).unwrap();
    assert_eq!((stored("do"), stored("undo")), (None, None));
}

#[test]
fn taking_away_usb_join_co_op_or_a_folder_grant_disconnects_the_device() {
    let f = Fixture::new();
    let client = f.client(PERM);
    let disconnects = |change: serde_json::Value| {
        let mut data = change;
        data["uuid"] = json!(client.uuid);
        let answer = handle(&f.host, "POST", "/api/clients/update", &data).unwrap();
        answer["disconnected"] == json!(true)
    };
    // Granting is not a revocation.
    assert!(!disconnects(json!({"allow_usb": true})));
    assert!(!disconnects(json!({"name": "Renamed"})));
    assert!(disconnects(json!({"allow_usb": false})));
    assert!(!disconnects(json!({"allow_usb": false})));
    assert!(!disconnects(json!({"allow_usb": true})));
    assert!(disconnects(json!({"allow_usb": null})));
    // Join co-op too: a co-op player loses its stream and cannot rejoin.
    assert!(!disconnects(json!({"coop_join": true})));
    assert!(disconnects(json!({"coop_join": false})));
    assert!(!disconnects(json!({"coop_join": null})));
    assert!(!disconnects(
        json!({"shared_folders": {"games": "write", "mods": "read"}})
    ));
    // Another grant, or the same ones again, take nothing away.
    assert!(!disconnects(
        json!({"shared_folders": {"games": "write", "mods": "read", "saves": "read"}})
    ));
    assert!(!disconnects(
        json!({"shared_folders": {"games": "write", "mods": "read", "saves": "read"}})
    ));
    assert!(!disconnects(
        json!({"shared_folders": {"games": "write", "mods": "write", "saves": "read"}})
    ));
    // Write back to read, a folder dropped, and all of them cleared.
    assert!(disconnects(
        json!({"shared_folders": {"games": "read", "mods": "write", "saves": "read"}})
    ));
    assert!(disconnects(
        json!({"shared_folders": {"games": "read", "mods": "write"}})
    ));
    assert!(disconnects(json!({"shared_folders": null})));
    assert!(!disconnects(json!({"shared_folders": null})));
}

#[test]
fn join_co_op_is_granted_only_as_a_boolean() {
    let f = Fixture::new();
    let client = f.client(PERM);
    let update = |value: serde_json::Value| {
        handle(
            &f.host,
            "POST",
            "/api/clients/update",
            &json!({"uuid": client.uuid, "coop_join": value}),
        )
    };
    let allowed = || f.host.paired.read().unwrap().clients[0].allows_coop_join();
    let set = || {
        f.host.paired.read().unwrap().clients[0]
            .extra
            .get("coop_join")
            .cloned()
    };
    // Never set: the device launches apps, so it may join.
    assert_ne!(PERM & butterpollo_core::state::LAUNCH_APPS, 0);
    assert!(allowed());
    let error = update(json!("yes")).unwrap_err();
    assert_eq!(error.to_string(), "coop_join must be true, false or null");
    assert_eq!(set(), None, "a refused value changes nothing");
    // Turned off: an explicit false keeps out a device that launches apps.
    update(json!(false)).unwrap();
    assert!(!allowed());
    update(json!(true)).unwrap();
    assert!(allowed());
    // null clears it: it follows Launch apps again.
    update(json!(false)).unwrap();
    update(serde_json::Value::Null).unwrap();
    assert_eq!(set(), None);
    assert!(allowed());
}

#[test]
fn a_device_cannot_save_a_preset_the_host_does_not_know() {
    let f = Fixture::new();
    let client = f.client(PERM);
    let save = |overrides: serde_json::Value| {
        handle(
            &f.host,
            "POST",
            "/api/clients/update",
            &json!({"uuid": client.uuid, "config_overrides": overrides}),
        )
    };
    let stored = || {
        f.host.paired.read().unwrap().clients[0]
            .extra
            .get("config_overrides")
            .cloned()
    };
    save(json!({"stream_preset": "Lowest-Latency", "max_bitrate": 20000})).unwrap();
    let good = stored();
    for bad in [
        json!("fastest"),
        json!(3),
        json!(true),
        json!(["reference"]),
    ] {
        let error = save(json!({"stream_preset": bad, "max_bitrate": 1})).unwrap_err();
        assert!(error.to_string().contains("stream_preset"), "{error}");
        assert_eq!(stored(), good, "{bad}");
    }
    // Clearing it is fine.
    save(json!({"stream_preset": null})).unwrap();
    save(json!({"stream_preset": ""})).unwrap();
}

fn layout_settings(f: &Fixture) -> (Option<serde_json::Value>, bool, bool) {
    let paired = f.host.paired.read().unwrap();
    let extra = &paired.clients[0].extra;
    (
        extra.get("virtual_display_layout").cloned(),
        extra.contains_key(crate::display_session::SET_BY_USER),
        extra.contains_key(crate::display_session::LEARNED_OVER),
    )
}
fn update(f: &Fixture, data: serde_json::Value) {
    handle(&f.host, "POST", "/api/clients/update", &data).unwrap();
}

#[test]
fn only_touching_the_layout_field_makes_a_learned_layout_the_users() {
    // Issue #40: a layout learned from a stream is not the user's until they
    // pick it, and a rename or another setting does not make it so.
    let f = Fixture::new();
    let client = f.client(PERM);
    let uuid = client.uuid;
    {
        let mut paired = f.host.paired.write().unwrap();
        let extra = &mut paired.clients[0].extra;
        extra.insert("virtual_display_layout".into(), "extended".into());
        extra.insert(
            crate::display_session::LEARNED_OVER.into(),
            "extended_primary".into(),
        );
    }
    update(
        &f,
        json!({"uuid": uuid, "name": "Renamed", "background_stream": true}),
    );
    assert_eq!(
        layout_settings(&f),
        (Some(json!("extended")), false, true),
        "a save that leaves the layout alone keeps it learned"
    );
    // The editor picked the value that was already there.
    update(
        &f,
        json!({"uuid": uuid, crate::display_session::SET_BY_USER: true}),
    );
    assert_eq!(layout_settings(&f), (Some(json!("extended")), true, false));
    // A rename does not take it back.
    update(&f, json!({"uuid": uuid, "name": "Again"}));
    assert_eq!(layout_settings(&f), (Some(json!("extended")), true, false));
    // Back to the host setting: nothing is left to protect or to yield.
    update(
        &f,
        json!({"uuid": uuid, "virtual_display_layout": null, crate::display_session::SET_BY_USER: null}),
    );
    assert_eq!(layout_settings(&f), (None, false, false));
}

#[test]
fn a_layout_changed_through_the_console_api_is_the_users() {
    let f = Fixture::new();
    let client = f.client(PERM);
    update(
        &f,
        json!({"uuid": client.uuid, "virtual_display_layout": "exclusive"}),
    );
    assert_eq!(layout_settings(&f), (Some(json!("exclusive")), true, false));
}

#[test]
fn approve_new_devices_is_granted_only_as_a_boolean_and_alone() {
    let f = Fixture::new();
    let client = f.client(PERM);
    let saved = || f.host.paired.read().unwrap().clients[0].clone();
    for bad in [json!("true"), json!(1)] {
        assert!(
            handle(
                &f.host,
                "POST",
                "/api/clients/update",
                &json!({"uuid": client.uuid, "pairing_approve": bad}),
            )
            .is_err()
        );
    }
    assert!(!saved().extra.contains_key("pairing_approve"));
    update(&f, json!({"uuid": client.uuid, "pairing_approve": true}));
    assert!(saved().allows_pairing_approve());
    assert!(!saved().allows_host_control());
    update(&f, json!({"uuid": client.uuid, "pairing_approve": null}));
    assert!(!saved().extra.contains_key("pairing_approve"));
}

#[test]
fn restart_and_shutdown_is_granted_only_as_a_boolean_and_alone() {
    let f = Fixture::new();
    let client = f.client(PERM);
    let saved = || f.host.paired.read().unwrap().clients[0].clone();
    for bad in [json!("true"), json!(1)] {
        assert!(
            handle(
                &f.host,
                "POST",
                "/api/clients/update",
                &json!({"uuid": client.uuid, "allow_restart_shutdown": bad}),
            )
            .is_err()
        );
    }
    assert!(!saved().extra.contains_key("allow_restart_shutdown"));
    update(
        &f,
        json!({"uuid": client.uuid, "allow_restart_shutdown": true}),
    );
    assert!(saved().allows_restart_shutdown());
    assert!(!saved().allows_sleep() && !saved().allows_host_control());
    update(
        &f,
        json!({"uuid": client.uuid, "allow_restart_shutdown": null}),
    );
    assert!(!saved().extra.contains_key("allow_restart_shutdown"));
}

#[test]
fn the_device_list_says_why_sleep_pc_is_unavailable() {
    let f = Fixture::new();
    f.client(PERM);
    let reason = |states| {
        f.host
            .power
            .set_backend(std::sync::Arc::new(crate::power_action::FixedStates(
                states,
            )));
        handle(&f.host, "GET", "/api/clients/list", &json!({})).unwrap()["sleep_unavailable"]
            .clone()
    };
    let desktop = SleepStates {
        s3: true,
        hibernate: true,
        ..Default::default()
    };
    assert_eq!(reason(desktop), json!(null));
    for hibernate in [true, false] {
        let laptop = SleepStates {
            modern_standby: true,
            hibernate,
            ..Default::default()
        };
        assert_eq!(reason(laptop), json!(laptop.unavailable().unwrap()));
    }
    assert_eq!(
        reason(SleepStates {
            modern_standby: true,
            hibernate: true,
            ..Default::default()
        }),
        json!("this PC only has Modern Standby, so Windows would hibernate it")
    );
}

#[test]
fn the_device_list_names_the_client_and_warns_only_about_listed_builds() {
    let f = Fixture::new();
    let client = f.client(PERM);
    let listed = || handle(&f.host, "GET", "/api/clients/list", &json!({})).unwrap();
    assert_eq!(listed()["clients"][0]["client_agent"], json!(null));
    assert_eq!(listed()["clients"][0]["client_warning"], json!(null));
    f.host
        .client_agents
        .lock()
        .unwrap()
        .insert(client.uuid.clone(), "Nightjar/3.2.0".into());
    assert_eq!(listed()["clients"][0]["client_agent"], "Nightjar/3.2.0");
    // The shipped list names no build, so nothing is flagged.
    assert_eq!(listed()["clients"][0]["client_warning"], json!(null));
    let list = [butterpollo_core::client_warnings::BrokenClient {
        product: "Nightjar",
        from: (3, 2, 0),
        fixed: (3, 2, 1),
        problem: "freezes",
        link: "https://github.com/RamazanKara/Rubylight/issues/0",
    }];
    assert_eq!(
        super::client_warning(&list, Some("Nightjar/3.2.0")),
        json!({"problem": "freezes", "link": "https://github.com/RamazanKara/Rubylight/issues/0"})
    );
    assert_eq!(
        super::client_warning(&list, Some("Nightjar/3.2.1")),
        json!(null)
    );
    assert_eq!(super::client_warning(&list, None), json!(null));
    // What the host saw is not a setting the console can write.
    update(
        &f,
        json!({"uuid": client.uuid, "client_agent": "x", "client_warning": {"problem": "y"}}),
    );
    {
        let paired = f.host.paired.read().unwrap();
        assert!(!paired.clients[0].extra.contains_key("client_agent"));
        assert!(!paired.clients[0].extra.contains_key("client_warning"));
    }
    // An unpaired device's client is forgotten with it.
    handle(
        &f.host,
        "POST",
        "/api/clients/unpair",
        &json!({"uuid": client.uuid}),
    )
    .unwrap();
    assert!(f.host.client_agents.lock().unwrap().is_empty());
}

#[test]
fn unpairing_every_device_gives_up_all_radeon_settings() {
    let f = Fixture::new();
    let client = f.client(PERM);
    f.host.radeon.note_written(&client.uuid, &[]);
    f.host.radeon.note_written("another-device", &[]);
    handle(&f.host, "POST", "/api/clients/unpair-all", &json!({})).unwrap();
    assert!(!f.host.radeon.holds(&client.uuid));
    assert!(!f.host.radeon.holds("another-device"));
}

#[test]
fn the_pairing_qr_code_is_refused_while_turned_off() {
    let f = Fixture::new();
    f.host
        .config
        .write()
        .unwrap()
        .values
        .insert("qr_pairing".into(), "false".into());
    let error = super::pairing_qr(&f.host, "", None, vec!["192.168.1.5".into()]).unwrap_err();
    assert!(error.to_string().contains("off"), "{error}");
    assert!(f.host.otp.lock().unwrap().qr.is_none());
}

#[test]
fn the_pairing_qr_code_holds_a_one_time_pin_and_the_host_certificate() {
    use sha2::Digest;
    let f = Fixture::new();
    f.host
        .config
        .write()
        .unwrap()
        .values
        .insert("qr_pairing".into(), "true".into());
    let addresses = vec!["192.168.1.5".to_owned(), "10.0.0.2".to_owned()];
    let answer = super::pairing_qr(&f.host, "10.0.0.2", None, addresses.clone()).unwrap();
    let codes = answer["codes"].as_array().unwrap();
    // The chosen address comes first.
    assert_eq!(codes[0]["address"], "10.0.0.2");
    assert_eq!(codes[1]["address"], "192.168.1.5");
    assert_eq!(answer["expires_in_seconds"], 180);
    let (pin, passphrase, device_name) = {
        let otp = f.host.otp.lock().unwrap();
        let otp = otp.qr.as_ref().unwrap();
        (
            otp.pin.clone(),
            otp.passphrase.clone(),
            otp.device_name.clone(),
        )
    };
    for code in codes {
        let uri = code["uri"].as_str().unwrap();
        let parsed = butterpollo_core::pairing_uri::PairingUri::parse(uri).unwrap();
        assert_eq!(parsed.host, code["address"].as_str().unwrap());
        assert_eq!(parsed.port, f.host.config.read().unwrap().port().unwrap());
        assert_eq!((&parsed.otp, &parsed.passphrase), (&pin, &passphrase));
        assert_eq!(parsed.otp.len(), 4);
        assert_eq!(parsed.passphrase.len(), 16);
        assert_eq!(
            parsed.cert_sha256,
            <[u8; 32]>::from(sha2::Sha256::digest(&f.host.identity.der))
        );
        assert_eq!(parsed.unique_id, f.host.paired.read().unwrap().unique_id);
        assert!(code["svg"].as_str().unwrap().starts_with("<svg"));
    }
    // The client names itself; the QR code never takes the typed PIN's slot.
    assert_eq!(device_name, "");
    assert!(f.host.otp.lock().unwrap().typed.is_none());

    let error =
        super::pairing_qr(&f.host, "172.16.0.1", None, vec!["10.0.0.2".into()]).unwrap_err();
    assert!(
        error.to_string().contains("not an address of this PC"),
        "{error}"
    );
    assert!(super::pairing_qr(&f.host, "", None, Vec::new()).is_err());
}

fn qr_passphrase(f: &Fixture) -> String {
    f.host
        .otp
        .lock()
        .unwrap()
        .qr
        .as_ref()
        .unwrap()
        .passphrase
        .clone()
}

#[test]
fn a_second_tab_shows_the_live_qr_code_until_its_last_minute() {
    let f = Fixture::new();
    f.host
        .config
        .write()
        .unwrap()
        .values
        .insert("qr_pairing".into(), "true".into());
    let addresses = vec!["192.168.1.5".to_owned()];
    let first = super::pairing_qr(&f.host, "", None, addresses.clone()).unwrap();
    let passphrase = qr_passphrase(&f);
    // Another tab, with 61 s left: the same code and its time left. The
    // latest access choice applies to it.
    f.host.otp.lock().unwrap().qr.as_mut().unwrap().created -= Duration::from_secs(119);
    let play = Some(butterpollo_core::state::PLAY_DEVICE_PERMISSION);
    let again = super::pairing_qr(&f.host, "", play, addresses.clone()).unwrap();
    assert_eq!(again["codes"][0]["uri"], first["codes"][0]["uri"]);
    assert_eq!(again["expires_in_seconds"], 61);
    assert_eq!(qr_passphrase(&f), passphrase);
    assert_eq!(
        f.host.otp.lock().unwrap().qr.as_ref().unwrap().permission,
        play
    );
    // Under a minute left: a new code, for three minutes.
    f.host.otp.lock().unwrap().qr.as_mut().unwrap().created -= Duration::from_secs(2);
    let next = super::pairing_qr(&f.host, "", None, addresses).unwrap();
    assert_ne!(next["codes"][0]["uri"], first["codes"][0]["uri"]);
    assert_eq!(next["expires_in_seconds"], 180);
    assert_ne!(qr_passphrase(&f), passphrase);
    assert_eq!(
        f.host.otp.lock().unwrap().qr.as_ref().unwrap().permission,
        None
    );
}

#[test]
fn the_typed_one_time_pin_leaves_the_qr_code_alone() {
    let f = Fixture::new();
    f.host
        .config
        .write()
        .unwrap()
        .values
        .insert("qr_pairing".into(), "true".into());
    super::pairing_qr(&f.host, "", None, vec!["192.168.1.5".into()]).unwrap();
    let passphrase = qr_passphrase(&f);
    let answer = handle(
        &f.host,
        "POST",
        "/api/otp",
        &json!({"passphrase": "typed phrase", "access": "play"}),
    )
    .unwrap();
    let otp = f.host.otp.lock().unwrap();
    let typed = otp.typed.as_ref().unwrap();
    assert_eq!(typed.pin, answer["otp"].as_str().unwrap());
    assert_eq!(typed.passphrase, "typed phrase");
    assert_eq!(
        typed.permission,
        Some(butterpollo_core::state::PLAY_DEVICE_PERMISSION)
    );
    assert_eq!(otp.qr.as_ref().unwrap().passphrase, passphrase);
    drop(otp);
    let error = handle(
        &f.host,
        "POST",
        "/api/otp",
        &json!({"passphrase": "typed phrase", "access": "admin"}),
    )
    .unwrap_err();
    assert!(error.to_string().contains("access"), "{error}");
}

#[test]
fn the_console_pin_carries_the_chosen_access_to_moonlight_requests_only() {
    let f = Fixture::new();
    let mut receivers = Vec::new();
    for (id, certificate) in [("moonlight", "certificate"), ("tv", "")] {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        f.host.pins.lock().unwrap().insert(
            id.into(),
            crate::state::PendingPin {
                name: id.into(),
                certificate: certificate.into(),
                address: [192, 168, 4, 20].into(),
                created: std::time::Instant::now(),
                sender,
            },
        );
        receivers.push(receiver);
    }
    let pin = |id: &str, access: &str| {
        handle(
            &f.host,
            "POST",
            "/api/pin",
            &json!({"pin": "1234", "uniqueid": id, "access": access}),
        )
    };
    let error = pin("tv", "play").unwrap_err();
    assert!(error.to_string().contains("Moonlight"), "{error}");
    assert!(f.host.pins.lock().unwrap().contains_key("tv"));
    assert!(pin("moonlight", "everything").is_err());
    pin("moonlight", "play").unwrap();
    assert_eq!(
        receivers[0].try_recv().unwrap(),
        (
            "1234".to_owned(),
            "moonlight".to_owned(),
            Some(butterpollo_core::state::PLAY_DEVICE_PERMISSION),
            None
        )
    );
    pin("tv", "").unwrap();
    assert_eq!(receivers[1].try_recv().unwrap().2, None);
}

#[test]
fn a_qr_code_renders_as_dark_squares_on_white() {
    let svg = super::qr_svg("rubylight://pair?v=1").unwrap();
    assert!(svg.starts_with("<svg"), "{svg}");
    assert!(
        svg.contains("fill=\"#fff\"") || svg.contains("fill=\"white\""),
        "{svg}"
    );
    assert!(svg.matches('M').count() > 50);
}
