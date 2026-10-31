use super::*;
fn client_with(perm: u32, extra: Value) -> Client {
    let mut document = json!({"name":"Phone","cert":"c","uuid":"u","perm":perm});
    document
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    serde_json::from_value(document).unwrap()
}
#[test]
fn host_control_is_off_by_default_and_for_a_full_permission_value() {
    assert!(!client_with(0x071f1f00, json!({})).allows_host_control());
    // Imports and fixtures use an all-ones permission value.
    assert!(!client_with(u32::MAX, json!({})).allows_host_control());
    assert!(!client_with(u32::MAX, json!({"allow_client_commands":true})).allows_host_control());
}
#[test]
fn host_control_reads_only_true_and_the_string_true() {
    assert!(client_with(0, json!({"host_control":true})).allows_host_control());
    assert!(client_with(0, json!({"host_control":"true"})).allows_host_control());
    for value in [
        json!("yes"),
        json!("1"),
        json!(1),
        json!("TRUE"),
        json!(false),
        json!(null),
        json!([true]),
    ] {
        assert!(
            !client_with(u32::MAX, json!({"host_control":value})).allows_host_control(),
            "{value}"
        );
    }
    let mut disabled = client_with(0, json!({"host_control":true}));
    disabled.enabled = false;
    assert!(!disabled.allows_host_control());
}
#[test]
fn imported_vibepollo_state_starts_without_host_control() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("state.json");
    let identity = crypto::Identity::generate().unwrap();
    write_json(
        &p,
        &json!({"root":{"named_devices":[
            {"name":"Old","uuid":"old","cert":identity.certificate,"perm":u32::MAX,
             "allow_client_commands":true}
        ]}}),
    )
    .unwrap();
    let s = PairedState::load(&p).unwrap();
    assert!(!s.clients[0].allows_host_control());
    // Saving and loading again doesn't add it either.
    s.save(&p).unwrap();
    let reloaded = PairedState::load(&p).unwrap();
    assert!(!reloaded.clients[0].allows_host_control());
    assert!(!reloaded.clients[0].extra.contains_key("host_control"));
}
#[test]
fn sleep_is_off_by_default_and_reads_only_a_json_true() {
    assert!(!client_with(0x071f1f00, json!({})).allows_sleep());
    assert!(!client_with(u32::MAX, json!({"host_control":true})).allows_sleep());
    assert!(client_with(0, json!({"allow_sleep":true})).allows_sleep());
    for value in [
        json!("true"),
        json!(1),
        json!("1"),
        json!(false),
        json!(null),
        json!([true]),
    ] {
        assert!(
            !client_with(u32::MAX, json!({"allow_sleep":value})).allows_sleep(),
            "{value}"
        );
    }
    let mut disabled = client_with(0, json!({"allow_sleep":true}));
    disabled.enabled = false;
    assert!(!disabled.allows_sleep());
}
#[test]
fn usb_is_off_by_default_and_reads_only_a_json_true() {
    assert!(!client_with(0x071f1f00, json!({})).allows_usb());
    assert!(!client_with(u32::MAX, json!({})).allows_usb());
    assert!(!client_with(u32::MAX, json!({"host_control":true,"allow_sleep":true})).allows_usb());
    assert!(client_with(0, json!({"allow_usb":true})).allows_usb());
    for value in [
        json!("true"),
        json!(1),
        json!("1"),
        json!(false),
        json!(null),
        json!([true]),
    ] {
        assert!(
            !client_with(u32::MAX, json!({"allow_usb":value})).allows_usb(),
            "{value}"
        );
    }
    let mut disabled = client_with(0, json!({"allow_usb":true}));
    disabled.enabled = false;
    assert!(!disabled.allows_usb());
}
#[test]
fn coop_join_never_set_follows_launch_apps_and_false_refuses() {
    // Never set (or cleared with null): a device that launches apps may join, one
    // that only watches may not, and Host control has nothing to do with it.
    assert!(client_with(PLAY_DEVICE_PERMISSION, json!({})).allows_coop_join());
    assert!(client_with(LAUNCH_APPS, json!({"coop_join":null})).allows_coop_join());
    assert!(!client_with(LATER_DEVICE_PERMISSION, json!({})).allows_coop_join());
    assert!(!client_with(!LAUNCH_APPS, json!({"host_control":true})).allows_coop_join());
    // Joining launches the co-op entry, so without Launch apps even a stored true
    // keeps the device out: the console's switch and invite list say the same.
    assert!(!client_with(0, json!({"coop_join":true})).allows_coop_join());
    assert!(!client_with(LATER_DEVICE_PERMISSION, json!({"coop_join":"true"})).allows_coop_join());
    assert!(client_with(LAUNCH_APPS, json!({"coop_join":"true"})).allows_coop_join());
    // Set to anything but true, it keeps out a device that launches apps.
    for value in [json!(false), json!(1), json!("yes")] {
        assert!(
            !client_with(u32::MAX, json!({"coop_join":value})).allows_coop_join(),
            "{value}"
        );
    }
    let mut disabled = client_with(PLAY_DEVICE_PERMISSION, json!({"coop_join":true}));
    disabled.enabled = false;
    assert!(!disabled.allows_coop_join());
}
#[test]
fn folder_grants_read_only_read_and_write() {
    use rubylight_native_api::folders::FolderAccess;
    let client = client_with(
        u32::MAX,
        json!({"shared_folders":{"games":"read","mods":"write","bad":"Write","num":1}}),
    );
    assert_eq!(client.folder_access("games"), Some(FolderAccess::Read));
    assert_eq!(client.folder_access("mods"), Some(FolderAccess::Write));
    assert_eq!(client.folder_access("bad"), None);
    assert_eq!(client.folder_access("num"), None);
    assert_eq!(client.folder_access("other"), None);
    assert!(client.has_folders());
    assert!(!client_with(u32::MAX, json!({})).has_folders());
    assert!(!client_with(u32::MAX, json!({"shared_folders":{}})).has_folders());
    assert!(!client_with(u32::MAX, json!({"shared_folders":{"a":"none"}})).has_folders());
    assert!(!client_with(u32::MAX, json!({"shared_folders":["games"]})).has_folders());
    assert!(!client_with(u32::MAX, json!({"shared_folders":null})).has_folders());
    let mut disabled = client_with(0, json!({"shared_folders":{"games":"write"}}));
    disabled.enabled = false;
    assert_eq!(disabled.folder_access("games"), None);
    assert!(!disabled.has_folders());
}
#[test]
fn a_hand_edited_folder_id_the_console_refuses_grants_nothing() {
    for id in ["../x", "", "a/b", "a b", "games.", &"x".repeat(65)] {
        let client = client_with(u32::MAX, json!({"shared_folders":{id:"write"}}));
        assert_eq!(client.folder_access(id), None, "{id}");
        assert!(!client.has_folders(), "{id}");
    }
    // Next to a good one, only the good one counts.
    let client = client_with(
        u32::MAX,
        json!({"shared_folders":{"../x":"write","games":"read"}}),
    );
    assert_eq!(client.folder_access("../x"), None);
    assert!(client.has_folders());
}
#[test]
fn imported_vibepollo_sleep_opt_in_is_honoured() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("state.json");
    let identity = crypto::Identity::generate().unwrap();
    let other = crypto::Identity::generate().unwrap();
    write_json(
        &p,
        &json!({"root":{"named_devices":[
            {"name":"Opted in","uuid":"in","cert":identity.certificate,"perm":u32::MAX,
             "allow_sleep":true},
            {"name":"Not","uuid":"out","cert":other.certificate,"perm":u32::MAX,
             "allow_sleep":"true"}
        ]}}),
    )
    .unwrap();
    let s = PairedState::load(&p).unwrap();
    assert!(s.clients[0].allows_sleep());
    assert!(!s.clients[1].allows_sleep());
    s.save(&p).unwrap();
    let reloaded = PairedState::load(&p).unwrap();
    assert!(reloaded.clients[0].allows_sleep());
    assert!(!reloaded.clients[1].allows_sleep());
}
#[test]
fn paired_saves_keep_credentials_changed_or_removed_on_disk() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("state.json");
    let old = Credentials::new("old".into(), "old-password").unwrap();
    write_json(&p, &old).unwrap();
    let mut s = PairedState::load(&p).unwrap();
    let new = Credentials::new("new".into(), "new-password").unwrap();
    write_json(&p, &new).unwrap();
    s.document["root"]["last_seen"] = 123.into();
    s.save(&p).unwrap();
    assert!(
        Credentials::load(&p)
            .unwrap()
            .unwrap()
            .verifies("new", "new-password")
    );
    assert!(
        !Credentials::load(&p)
            .unwrap()
            .unwrap()
            .verifies("old", "old-password")
    );
    assert_eq!(
        load_json(&p, Value::Null).unwrap()["root"]["last_seen"],
        123
    );
    let mut document = load_json(&p, Value::Null).unwrap();
    for key in ["username", "password", "salt"] {
        document.as_object_mut().unwrap().remove(key);
    }
    write_json(&p, &document).unwrap();
    s.save(&p).unwrap();
    assert!(Credentials::load(&p).unwrap().is_none());
}
fn webrtc_client(n: u8, name: &str) -> Client {
    let id = format!("{n:02x}").repeat(16);
    let secret = format!("{n:02x}").repeat(32);
    serde_json::from_value(json!({
        "name": name, "cert": "", "uuid": format!("web-{n}"), "perm": 0x071f1f00u32,
        "webrtc": {"clientId": id, "secret": secret},
    }))
    .unwrap()
}
#[test]
fn an_empty_certificate_never_matches_a_tls_peer() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("state.json");
    let mut s = PairedState::load(&p).unwrap();
    s.add_webrtc(&p, webrtc_client(1, "TV"), 32).unwrap();
    // A client record with no WebRTC credentials and no certificate, as a
    // hand edit could leave, matches nothing either.
    let mut blank = webrtc_client(2, "Blank");
    blank.extra.clear();
    s.clients.push(blank);
    for der in [&b""[..], b"\0", b"abc"] {
        assert!(s.client_by_certificate(der).is_none());
    }
    let identity = crypto::Identity::generate().unwrap();
    let der = crypto::certificate_der(&identity.certificate).unwrap();
    assert!(s.client_by_certificate(&der).is_none());
    // A real certificate still matches next to them.
    s.clients.push(
        serde_json::from_value(json!({"name":"Phone","uuid":"phone","cert":identity.certificate}))
            .unwrap(),
    );
    assert_eq!(s.client_by_certificate(&der).unwrap().uuid, "phone");
}
#[test]
fn webrtc_devices_survive_a_reload_and_the_oldest_beyond_the_limit_goes() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("state.json");
    let mut s = PairedState::load(&p).unwrap();
    assert!(
        s.add_webrtc(&p, webrtc_client(1, "A"), 2)
            .unwrap()
            .is_empty()
    );
    s.add_webrtc(&p, webrtc_client(2, "B"), 2).unwrap();
    let dropped = s.add_webrtc(&p, webrtc_client(3, "C"), 2).unwrap();
    assert_eq!(dropped, ["web-1"]);
    let reloaded = PairedState::load(&p).unwrap();
    let names: Vec<_> = reloaded.clients.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["B", "C"]);
    assert!(reloaded.clients.iter().all(Client::is_webrtc));
    assert!(reloaded.webrtc_client(&"03".repeat(16)).is_some());
    assert!(reloaded.webrtc_client(&"01".repeat(16)).is_none());
    // The same client ID or UUID can't be added twice.
    assert!(s.add_webrtc(&p, webrtc_client(3, "C"), 2).is_err());
}
fn phone(name: &str) -> Client {
    let identity = crypto::Identity::generate().unwrap();
    serde_json::from_value(
            json!({"name":name,"uuid":format!("phone-{name}"),"cert":identity.certificate,"perm":FIRST_DEVICE_PERMISSION}),
        )
        .unwrap()
}
fn perm_of(s: &PairedState, name: &str) -> u32 {
    s.clients.iter().find(|c| c.name == name).unwrap().perm
}
#[test]
fn the_first_device_of_either_kind_gets_everything_and_later_ones_view_only() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("state.json");
    // A TV first, then a phone: the phone is a later device.
    let mut s = PairedState::load(&p).unwrap();
    assert_eq!(s.new_device_permission(), FIRST_DEVICE_PERMISSION);
    s.add_webrtc(&p, webrtc_client(1, "TV"), 32).unwrap();
    assert_eq!(perm_of(&s, "TV"), FIRST_DEVICE_PERMISSION);
    assert_eq!(s.new_device_permission(), LATER_DEVICE_PERMISSION);
    // A phone first, then a TV.
    let p = d.path().join("other.json");
    let mut s = PairedState::load(&p).unwrap();
    s.add(&p, phone("Phone")).unwrap();
    s.add_webrtc(&p, webrtc_client(2, "TV"), 32).unwrap();
    assert_eq!(perm_of(&s, "TV"), LATER_DEVICE_PERMISSION);
    // Two TVs: the first of them may do everything, the second may not,
    // whatever permissions the record arrived with.
    let p = d.path().join("tvs.json");
    let mut s = PairedState::load(&p).unwrap();
    s.add_webrtc(&p, webrtc_client(3, "One"), 32).unwrap();
    s.add_webrtc(&p, webrtc_client(4, "Two"), 32).unwrap();
    assert_eq!(perm_of(&s, "One"), FIRST_DEVICE_PERMISSION);
    assert_eq!(perm_of(&s, "Two"), LATER_DEVICE_PERMISSION);
    let reloaded = PairedState::load(&p).unwrap();
    assert_eq!(perm_of(&reloaded, "Two"), LATER_DEVICE_PERMISSION);
}
#[test]
fn pairing_access_applies_to_later_devices_and_never_to_the_first() {
    assert_eq!(PLAY_DEVICE_PERMISSION, 0x07001f00);
    assert_eq!(PLAY_DEVICE_PERMISSION & !FIRST_DEVICE_PERMISSION, 0);
    assert_eq!(pairing_access("").unwrap(), None);
    assert_eq!(
        pairing_access("play").unwrap(),
        Some(PLAY_DEVICE_PERMISSION)
    );
    assert_eq!(
        pairing_access("view").unwrap(),
        Some(LATER_DEVICE_PERMISSION)
    );
    assert_eq!(
        pairing_access("all").unwrap(),
        Some(FIRST_DEVICE_PERMISSION)
    );
    assert!(pairing_access("Play").is_err());
    // Play games and Everything may join co-op too; view only may not.
    for (perm, joins) in [
        (PLAY_DEVICE_PERMISSION, true),
        (FIRST_DEVICE_PERMISSION, true),
        (LATER_DEVICE_PERMISSION, false),
    ] {
        let mut client = client_with(perm, json!({}));
        client.extra = PairedState::paired_device_extra(perm);
        assert_eq!(client.allows_coop_join(), joins, "{perm:#x}");
    }
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("state.json");
    let mut s = PairedState::load(&p).unwrap();
    assert_eq!(
        s.paired_device_permission(Some(LATER_DEVICE_PERMISSION)),
        FIRST_DEVICE_PERMISSION
    );
    s.add(&p, phone("Phone")).unwrap();
    assert_eq!(
        s.paired_device_permission(Some(PLAY_DEVICE_PERMISSION)),
        PLAY_DEVICE_PERMISSION
    );
    assert_eq!(s.paired_device_permission(None), LATER_DEVICE_PERMISSION);
}
const FILE_BITS: u32 = crate::native::FILE_UPLOAD | crate::native::FILE_DOWNLOAD;
#[test]
fn file_transfer_is_granted_on_its_own_never_with_everything() {
    assert_eq!(FIRST_DEVICE_PERMISSION & FILE_BITS, 0);
    assert_eq!(LATER_DEVICE_PERMISSION & FILE_BITS, 0);
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("state.json");
    let mut s = PairedState::load(&p).unwrap();
    s.add(&p, phone("Phone")).unwrap();
    assert_eq!(perm_of(&s, "Phone") & FILE_BITS, 0);
}
#[test]
fn file_transfer_given_by_a_test_build_is_taken_back_once() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("state.json");
    let mut tested = phone("Tested");
    tested.perm = 0x071f1f00;
    write_json(&p, &json!({"root":{"named_devices":[tested]}})).unwrap();
    let mut s = PairedState::load(&p).unwrap();
    assert_eq!(perm_of(&s, "Tested"), 0x07131f00);
    // Ticked again in the console, the bits stay.
    s.clients[0].perm |= FILE_BITS;
    s.save(&p).unwrap();
    let saved = load_json(&p, Value::Null).unwrap();
    assert_eq!(saved["root"]["rubylight_state_version"], 1);
    let reloaded = PairedState::load(&p).unwrap();
    assert_eq!(perm_of(&reloaded, "Tested"), 0x071f1f00);
    // A later version's file is left as it is.
    let mut later = saved;
    later["root"]["rubylight_state_version"] = json!(7);
    write_json(&p, &later).unwrap();
    let s = PairedState::load(&p).unwrap();
    assert_eq!(perm_of(&s, "Tested"), 0x071f1f00);
    assert_eq!(s.document["root"]["rubylight_state_version"], 7);
}
fn console_client(n: u8, username: &str) -> Client {
    let id = format!("{n:02x}").repeat(16);
    let secret = format!("{n:02x}").repeat(32);
    Client::console_over_webrtc(format!("Console ({username})"), username, &id, &secret)
}
#[test]
fn the_consoles_own_device_gets_everything_and_leaves_the_first_device_its_permissions() {
    let d = tempfile::tempdir().unwrap();
    // The console streams first; the first phone is still the first device.
    let p = d.path().join("phone.json");
    let mut s = PairedState::load(&p).unwrap();
    s.add_webrtc(&p, console_client(1, "admin"), 32).unwrap();
    assert_eq!(s.clients[0].console(), Some("admin"));
    assert_eq!(perm_of(&s, "Console (admin)"), FIRST_DEVICE_PERMISSION);
    assert_eq!(s.new_device_permission(), FIRST_DEVICE_PERMISSION);
    // GameStream pairing gives the phone what the host offers before it adds it.
    s.add(&p, phone("Phone")).unwrap();
    assert_eq!(s.new_device_permission(), LATER_DEVICE_PERMISSION);
    // The same for a TV.
    let p = d.path().join("tv.json");
    let mut s = PairedState::load(&p).unwrap();
    s.add_webrtc(&p, console_client(1, "admin"), 32).unwrap();
    s.add_webrtc(&p, webrtc_client(2, "TV"), 32).unwrap();
    assert_eq!(perm_of(&s, "TV"), FIRST_DEVICE_PERMISSION);
    s.add_webrtc(&p, webrtc_client(3, "Second TV"), 32).unwrap();
    assert_eq!(perm_of(&s, "Second TV"), LATER_DEVICE_PERMISSION);
    // A console device added after other devices still gets everything, in one save.
    s.add_webrtc(&p, console_client(4, "other"), 32).unwrap();
    let reloaded = PairedState::load(&p).unwrap();
    assert_eq!(
        perm_of(&reloaded, "Console (other)"),
        FIRST_DEVICE_PERMISSION
    );
    assert_eq!(reloaded.clients.last().unwrap().console(), Some("other"));
    // A device with a certificate is never a console's, whatever its record says.
    let mut tls = phone("Marked");
    tls.extra
        .insert("webrtc".into(), json!({"console": "admin"}));
    assert_eq!(tls.console(), None);
}
#[test]
fn the_consoles_own_devices_neither_count_towards_the_webrtc_limit_nor_go() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("state.json");
    let mut s = PairedState::load(&p).unwrap();
    s.add_webrtc(&p, console_client(1, "admin"), 2).unwrap();
    s.add_webrtc(&p, webrtc_client(2, "A"), 2).unwrap();
    s.add_webrtc(&p, webrtc_client(3, "B"), 2).unwrap();
    // At the limit, the console's device unpairs nothing.
    assert!(
        s.add_webrtc(&p, console_client(4, "other"), 2)
            .unwrap()
            .is_empty()
    );
    // A third TV unpairs the oldest TV, never a console's device.
    assert_eq!(
        s.add_webrtc(&p, webrtc_client(5, "C"), 2).unwrap(),
        ["web-2"]
    );
    let names: Vec<_> = PairedState::load(&p)
        .unwrap()
        .clients
        .iter()
        .map(|c| c.name.clone())
        .collect();
    assert_eq!(names, ["Console (admin)", "B", "Console (other)", "C"]);
}
#[test]
fn a_repeated_webrtc_client_id_is_skipped_but_kept_in_the_file() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("state.json");
    let first = serde_json::to_value(webrtc_client(1, "First")).unwrap();
    let mut second = serde_json::to_value(webrtc_client(1, "Second")).unwrap();
    second["uuid"] = json!("web-other");
    write_json(
        &p,
        &json!({"root":{"named_devices":[first, second.clone()]}}),
    )
    .unwrap();
    let s = PairedState::load(&p).unwrap();
    assert_eq!(s.clients.len(), 1);
    assert_eq!(s.clients[0].name, "First");
    s.save(&p).unwrap();
    let saved = load_json(&p, Value::Null).unwrap();
    assert!(
        saved["root"]["named_devices"]
            .as_array()
            .unwrap()
            .contains(&second)
    );
}
#[test]
fn a_webrtc_devices_retired_overrides_are_replaced_on_load() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("state.json");
    let mut tv = serde_json::to_value(webrtc_client(1, "TV")).unwrap();
    tv["config_overrides"] = json!({"gamepad": "ds4"});
    write_json(&p, &json!({"root":{"named_devices":[tv]}})).unwrap();
    let s = PairedState::load(&p).unwrap();
    assert_eq!(
        s.clients[0].extra["config_overrides"],
        json!({"gamepad": "vhf_ds4"})
    );
}
#[test]
fn a_device_without_a_certificate_or_valid_webrtc_credentials_is_still_skipped() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("state.json");
    let short =
        json!({"name":"Short","uuid":"s","cert":"","webrtc":{"clientId":"ab","secret":"cd"}});
    let none = json!({"name":"None","uuid":"n","cert":""});
    write_json(&p, &json!({"root":{"named_devices":[short, none]}})).unwrap();
    let s = PairedState::load(&p).unwrap();
    assert!(s.clients.is_empty());
    s.save(&p).unwrap();
    let saved = load_json(&p, Value::Null).unwrap();
    let kept: Vec<_> = saved["root"]["named_devices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["uuid"].as_str().unwrap())
        .collect();
    assert_eq!(kept, ["s", "n"]);
}
#[test]
fn malformed_devices_are_preserved_without_blocking_usable_pairings() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("state.json");
    let identity = crypto::Identity::generate().unwrap();
    let malformed = json!({"name":"Malformed","uuid":"broken","perm":[]});
    let unreadable =
        json!({"name":"Unreadable","uuid":"bad-cert","cert":"bad","perm":1,"enabled":true});
    write_json(
        &p,
        &json!({"root":{"named_devices":[malformed, unreadable,
            {"name":"Phone","uuid":"valid","cert":identity.certificate}
        ]}}),
    )
    .unwrap();
    let s = PairedState::load(&p).unwrap();
    assert_eq!(s.clients.len(), 1);
    assert_eq!(s.clients[0].uuid, "valid");
    s.save(&p).unwrap();
    let saved = load_json(&p, Value::Null).unwrap();
    assert_eq!(saved["root"]["named_devices"][0], malformed);
    assert_eq!(saved["root"]["named_devices"][1], unreadable);
    let mut reloaded = PairedState::load(&p).unwrap();
    reloaded.remove(&p, "valid").unwrap();
    assert_eq!(
        load_json(&p, Value::Null).unwrap()["root"]["named_devices"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(PairedState::load(&p).unwrap().clients.is_empty());
    reloaded.clear();
    reloaded.save(&p).unwrap();
    assert_eq!(
        load_json(&p, Value::Null).unwrap()["root"]["named_devices"],
        json!([])
    );
}
#[test]
fn preparation_commands_keep_unknown_fields() {
    let original = json!({"do":"before","undo":"after","elevated":false,"future":{"value":42}});
    let command: PrepCommand = serde_json::from_value(original.clone()).unwrap();
    assert_eq!(serde_json::to_value(command).unwrap(), original);
}
#[test]
fn bad_existing_state_is_not_replaced() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("state.json");
    std::fs::write(&p, b"broken").unwrap();
    assert!(PairedState::load(&p).is_err());
    assert_eq!(std::fs::read(&p).unwrap(), b"broken");
}
#[test]
fn a_byte_order_mark_is_skipped_and_a_blank_file_reads_as_missing() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("state.json");
    std::fs::write(&p, b"\xef\xbb\xbf{\"root\":{\"uniqueid\":\"same-id\"}}").unwrap();
    assert_eq!(
        load_json(&p, json!(null)).unwrap(),
        json!({"root":{"uniqueid":"same-id"}})
    );
    assert_eq!(PairedState::load(&p).unwrap().unique_id, "same-id");
    for blank in [&b""[..], b"\xef\xbb\xbf", b" \r\n\t"] {
        std::fs::write(&p, blank).unwrap();
        assert_eq!(
            load_json(&p, json!({"apps":[]})).unwrap(),
            json!({"apps":[]})
        );
        assert!(PairedState::load(&p).unwrap().clients.is_empty());
    }
    std::fs::write(&p, b"\xef\xbb\xbf broken").unwrap();
    assert!(load_json(&p, json!(null)).is_err());
}
#[test]
fn atomic_state_replacement_and_unknown_fields() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("state.json");
    write_json(
        &p,
        &json!({"root":{"uniqueid":"same-id","named_devices":[],"other":[1,2]}}),
    )
    .unwrap();
    let s = PairedState::load(&p).unwrap();
    s.save(&p).unwrap();
    let n = load_json(&p, json!(null)).unwrap();
    assert_eq!(n["root"]["other"], json!([1, 2]));
    assert_eq!(n["root"]["uniqueid"], "same-id");
}
#[test]
fn pairing_a_known_certificate_again_keeps_the_device_and_duplicates_load() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("state.json");
    let identity = crypto::Identity::generate().unwrap();
    let client = |name: &str, uuid: &str| Client {
        name: name.into(),
        cert: identity.certificate.clone(),
        uuid: uuid.into(),
        perm: 0x0300_0000,
        enabled: true,
        extra: BTreeMap::new(),
    };
    let mut s = PairedState::load(&p).unwrap();
    s.add(&p, client("Phone", "a")).unwrap();
    s.clients[0].perm = 0x071f_1f00;
    s.add(&p, client("Phone again", "b")).unwrap();
    assert_eq!(s.clients.len(), 1);
    assert_eq!(s.clients[0].uuid, "a");
    assert_eq!(s.clients[0].perm, 0x071f_1f00);
    assert_eq!(s.clients[0].name, "Phone again");
    // A Vibepollo profile that lists the certificate twice still loads.
    let mut doubled = s.clone();
    doubled.clients.push(client("Phone", "a"));
    doubled.save(&p).unwrap();
    let loaded = PairedState::load(&p).unwrap();
    assert_eq!(loaded.clients.len(), 1);
    let der = loaded.clients[0].der().unwrap();
    assert!(loaded.client_by_certificate(&der).is_some());
}
#[test]
fn a_device_with_a_vigembus_controller_type_gets_its_vhf_pad() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("state.json");
    let identity = crypto::Identity::generate().unwrap();
    let mut s = PairedState::load(&p).unwrap();
    s.add(
        &p,
        Client {
            name: "Phone".into(),
            cert: identity.certificate.clone(),
            uuid: "a".into(),
            perm: 0x071f_1f00,
            enabled: true,
            extra: BTreeMap::from([(
                "config_overrides".into(),
                json!({"gamepad": "x360", "keyboard": "true"}),
            )]),
        },
    )
    .unwrap();
    let loaded = PairedState::load(&p).unwrap();
    assert_eq!(
        loaded.clients[0].extra["config_overrides"],
        json!({"gamepad": "vhf_xbox_one", "keyboard": "true"})
    );
}
#[test]
fn property_tree_empty_lists_and_unreadable_certificates_load() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("state.json");
    let write = |document: Value| std::fs::write(&p, document.to_string()).unwrap();
    write(json!({"root":{"uniqueid":"same-id","named_devices":"","devices":""}}));
    let s = PairedState::load(&p).unwrap();
    assert!(s.clients.is_empty());
    assert_eq!(s.unique_id, "same-id");
    let identity = crypto::Identity::generate().unwrap();
    write(json!({"root":{"uniqueid":"same-id","named_devices":[
            {"name":"Broken","cert":"not a certificate","uuid":"a"},
            {"name":"Phone","cert":identity.certificate,"uuid":"b",
             "do":"","undo":"","config_overrides":"","other":""}
        ],"devices":[{"certs":""}]}}));
    let s = PairedState::load(&p).unwrap();
    assert_eq!(s.clients.len(), 1);
    let phone = &s.clients[0];
    assert_eq!(phone.name, "Phone");
    assert_eq!(phone.extra["do"], json!([]));
    assert_eq!(phone.extra["undo"], json!([]));
    assert_eq!(phone.extra["config_overrides"], json!({}));
    assert_eq!(phone.extra["other"], "");
}
#[test]
fn existing_credentials_format() {
    let c = Credentials {
        username: "user".into(),
        salt: "abc".into(),
        password: hex::encode(crypto::hash(b"passwordabc")).to_uppercase(),
    };
    assert!(c.verifies("user", "password"));
    assert!(c.verifies("User", "password"));
    assert!(!c.verifies("other", "password"));
    assert!(!c.verifies("user", "wrong"));
    // Independent SHA-256/password+salt vector in C++ util::hex byte order.
    let previous = Credentials {
        password: "F88D961F52F30D505CC0CBB98D01B38D0D789C075812B3C38748CEEAFFB73367".into(),
        ..c
    };
    assert!(previous.verifies("USER", "password"));
    assert!(!previous.verifies("user", "wrong"));
    assert!(!previous.verifies("other", "password"));
    let current = Credentials::new("user".into(), "new-password").unwrap();
    assert_eq!(
        current.password,
        crypto::legacy_hash(format!("new-password{}", current.salt).as_bytes())
    );
    assert!(current.verifies("user", "new-password"));
}
