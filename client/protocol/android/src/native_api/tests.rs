use super::*;
use nightjar_core::http::native_api::error::{ApiError, ErrorCode};
use nightjar_core::http::PairingRequest;
use std::sync::Mutex;

#[derive(Default)]
struct Fake {
    calls: Mutex<Vec<String>>,
    fail: Option<ErrorCode>,
    /// The host's GET /capabilities body, when not the default one.
    capabilities: Option<&'static str>,
}

impl Fake {
    fn note(&self, call: &str) {
        self.calls.lock().unwrap().push(call.to_string());
    }

    fn session(&self) -> Result<Session, Error> {
        if let Some(code) = self.fail {
            return Err(Error::Api {
                status: code.status(),
                error: ApiError {
                    code,
                    message: "Turn on Host control for this device.".into(),
                    setting: Some("chill.enabled".into()),
                    retry_after_ms: Some(250),
                },
            });
        }
        Ok(serde_json::from_str(
            r#"{"revision":4,"state":"suspended","owner":"you","app":null,"game_running":true,"actions":[{"id":"end","allowed":true}]}"#,
        )
        .unwrap())
    }
}

impl Host for Fake {
    fn capabilities_with_files(&self) -> Result<NativeCapabilities, Error> {
        self.note("capabilities");
        Ok(serde_json::from_str(self.capabilities.unwrap_or(
            r#"{"api_version":1,"host":{"name":"PC","version":"3.0.0","unique_id":"U"},"features":["files.upload","files.download"],"permissions":{"list":true,"view":true,"launch":true,"host_control":true,"file_upload":true,"file_download":false,"usb":"yes","shared_folders":true,"pairing_approve":true}}"#,
        ))
        .unwrap())
    }
    fn library(&self) -> Result<NativeLibrary, Error> {
        self.note("library");
        Err(Error::NotFound("/api/native/v1/library".into()))
    }
    fn session(&self) -> Result<Session, Error> {
        self.note("session");
        Fake::session(self)
    }
    fn session_action(&self, action: ActionId) -> Result<Session, Error> {
        self.note(&format!("action {action:?}"));
        Fake::session(self)
    }
    fn stream_move(&self) -> Result<StreamMove, Error> {
        self.note("stream move");
        Ok(serde_json::from_str(r#"{"moved_to":{"device":"TV","since_unix_ms":5},"owns_game":true,"press_failed":true}"#).unwrap())
    }
    fn continue_here(&self) -> Result<Session, Error> {
        self.note("continue here");
        Fake::session(self)
    }
    fn radeon(&self) -> Result<Radeon, Error> {
        self.note("radeon");
        Err(Error::Cancelled)
    }
    fn write_radeon(&self, write: &SettingsWrite) -> Result<Radeon, Error> {
        self.note(&format!("write {} {}", write.gpu, write.changes.len()));
        Err(Error::Malformed("no".into()))
    }
    fn restore_radeon(&self) -> Result<Radeon, Error> {
        self.note("restore");
        Err(Error::NotPaired)
    }
    fn couch_menu(&self, open: bool) -> Result<CouchMenu, Error> {
        self.note(&format!("couch menu {open}"));
        if let Some(code) = self.fail {
            return Err(Error::Api {
                status: code.status(),
                error: ApiError {
                    code,
                    message: "Start a stream first.".into(),
                    setting: None,
                    retry_after_ms: None,
                },
            });
        }
        Ok(CouchMenu { open })
    }
    fn wake(&self) -> Result<Wake, Error> {
        self.note("wake");
        Ok(serde_json::from_str(
            r#"{"revision":1,"primary":"02:7c:16:b4:13:4f","targets":[{"mac":"02:7c:16:b4:13:4f","kind":"ethernet","connected":true,"route":true,"broadcasts":["192.168.4.255"],"state":"ready","checks":[]}],"system":{"sleep":"s3","modern_standby":false,"hibernate":false,"fast_startup":"inactive","shutdown_wake":"firmware_unknown"},"idle_sleep":{"state":"off","minutes":0,"reason":null}}"#,
        )
        .unwrap())
    }
    fn coop(&self) -> Result<Coop, Error> {
        self.note("coop");
        Ok(serde_json::from_str(
            r#"{"available":true,"reason":null,"games":[{"app_id":7,"title":"Portal 2 (Co-op)","game":"g","max_players":2}],"sessions":[{"id":3,"app_id":7,"title":"Portal 2","game":"g","phase":"running","end":null,"players":[{"slot":0,"name":"Fold7","you":false,"away":false},{"slot":1,"name":null,"you":false,"away":false}],"free_slots":1,"joinable":false,"refusal":"This device may not join co-op sessions"}]}"#,
        )
        .unwrap())
    }
    fn join_coop(&self, session: u32) -> Result<CoopJoined, Error> {
        self.note(&format!("join {session}"));
        if let Some(code) = self.fail {
            return Err(Error::Api {
                status: code.status(),
                error: ApiError {
                    code,
                    message: "Turn on “Can join co-op games” for this device in the Rubylight console."
                        .into(),
                    setting: None,
                    retry_after_ms: None,
                },
            });
        }
        Ok(CoopJoined {
            session,
            slot: 1,
            app_id: 7,
        })
    }
    fn power_capabilities(&self) -> Result<PowerReply, PowerError> {
        self.note("power");
        Ok(power_reply(200, "", Some(TOKEN)))
    }
    fn sleep(&self, token: &str) -> Result<PowerReply, PowerError> {
        self.note(&format!("sleep {token}"));
        match token {
            TOKEN => Ok(power_reply(202, "Sleep request accepted", None)),
            "" => Err(PowerError::Host(Error::InvalidArgument("no token".into()))),
            _ => Err(PowerError::Refused {
                refusal: Refusal::Conflict,
                reply: power_reply(409, "Sleep token expired or already used", None),
            }),
        }
    }
    fn pc_power(&self) -> Result<PcPowerOffer, Error> {
        self.note("pc power");
        Ok(serde_json::from_str(&format!(
            r#"{{"restart":{{"available":true,"reason":null}},"shutdown":{{"available":true,"reason":null}},"token":"{TOKEN}","token_lifetime_s":30,"wake_from_off":false,"last_refusal":null}}"#
        ))
        .unwrap())
    }
    fn power_pc(&self, action: PcPower, token: &str) -> Result<(), Error> {
        self.note(&format!("{} {token}", action.path()));
        if token == TOKEN {
            return Ok(());
        }
        Err(Error::Api {
            status: 409,
            error: serde_json::from_str(
                r#"{"code":"host_in_use","message":"Living room TV is streaming from this PC.","setting":null,"retry_after_ms":null}"#,
            )
            .unwrap(),
        })
    }
    fn host_update(&self) -> Result<HostUpdate, Error> {
        self.note("update");
        Ok(serde_json::from_str(
            r#"{"current_version":"3.3.0","latest_version":"v3.4.0","phase":"idle","install_supported":true,"error":null}"#,
        )
        .unwrap())
    }
    fn pairing_requests(&self) -> Result<PairingRequests, Error> {
        self.note("pairing requests");
        Ok(PairingRequests {
            requests: vec![PairingRequest {
                id: "a1".into(),
                name: "TV".into(),
                address: "192.168.4.116".into(),
                age_seconds: 12,
            }],
        })
    }
    fn approve_pairing(&self, id: &str, pin: &str) -> Result<PairingApproved, Error> {
        self.note(&format!("approve {id} {pin}"));
        if pin == "0000" {
            return Err(Error::Api {
                status: 422,
                error: ApiError {
                    code: ErrorCode::Unknown,
                    message: "That is not the PIN TV shows.".into(),
                    setting: None,
                    retry_after_ms: None,
                },
            });
        }
        Ok(PairingApproved { name: "TV".into() })
    }
    fn deny_pairing(&self, id: &str) -> Result<PairingRequests, Error> {
        self.note(&format!("deny {id}"));
        Ok(PairingRequests { requests: Vec::new() })
    }
    fn session_guests(&self) -> Result<SessionGuests, Error> {
        self.note("guests");
        Ok(serde_json::from_str(GUESTS).unwrap())
    }
    fn guest_action(&self, action: &GuestAction) -> Result<SessionGuests, Error> {
        self.note(&format!("guest {}", action.request().0));
        Ok(serde_json::from_str(GUESTS).unwrap())
    }
    fn install_host_update(&self) -> Result<HostUpdate, Error> {
        self.note("update install");
        Ok(serde_json::from_str(
            r#"{"current_version":"3.3.0","latest_version":"v3.4.0","phase":"downloading","install_supported":true,"error":null}"#,
        )
        .unwrap())
    }
    fn audio(&self) -> Result<Audio, Error> {
        self.note("audio");
        Ok(serde_json::from_str(AUDIO).unwrap())
    }
    fn write_audio(&self, write: &AudioWrite) -> Result<Audio, Error> {
        self.note(&format!("write audio {}", serde_json::to_string(write).unwrap()));
        if let Some(code) = self.fail {
            return Err(Error::Api {
                status: code.status(),
                error: ApiError {
                    code,
                    message: "This session belongs to another device.".into(),
                    setting: None,
                    retry_after_ms: None,
                },
            });
        }
        Ok(serde_json::from_str(AUDIO).unwrap())
    }
}

const AUDIO: &str = r#"{"revision":3,"settings":[{"id":"audio.night_mode","group":"sound","kind":"toggle","state":"configurable","value":true}]}"#;

#[test]
fn audio_reads_and_writes_reach_the_host_and_bad_writes_do_not() {
    let fake = Fake::default();
    let read = parse(&execute(&fake, "audio", ""));
    assert_eq!(read["ok"]["settings"][0]["id"], "audio.night_mode");
    assert_eq!(read["ok"]["settings"][0]["value"], true);
    let write = r#"{"changes":[{"id":"audio.night_mode","value":false}]}"#;
    assert_eq!(parse(&execute(&fake, "audio_write", write))["ok"]["revision"], 3);
    let written = format!("write audio {write}");
    assert_eq!(
        fake.calls.lock().unwrap().clone(),
        ["audio", written.as_str()]
    );
    for bad in ["", "{}", r#"{"changes":[{"id":"audio.night_mode","value":true,"scope":"host"}]}"#] {
        let reply = parse(&execute(&fake, "audio_write", bad));
        assert_eq!(reply["error"]["kind"], "invalid_argument", "{bad}");
    }
    assert_eq!(fake.calls.lock().unwrap().len(), 2);

    let refused = Fake {
        fail: Some(ErrorCode::NotSessionOwner),
        ..Fake::default()
    };
    let reply = parse(&execute(&refused, "audio_write", write));
    assert_eq!(reply["error"]["code"], "not_session_owner");
    assert_eq!(reply["error"]["http_status"], 409);
}

const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn power_reply(status: u16, message: &str, token: Option<&str>) -> PowerReply {
    PowerReply {
        status,
        message: message.into(),
        version: 1,
        sleep_supported: true,
        sleep_allowed: true,
        sleep_available: token.is_some(),
        active_session_policy: "idle-only".into(),
        token: token.map(str::to_owned),
        token_lifetime_seconds: 30,
        accepted: status == 202,
    }
}

fn parse(reply: &str) -> Value {
    serde_json::from_str(reply).unwrap()
}

#[test]
fn answers_carry_the_apis_json_under_ok() {
    let fake = Fake::default();
    let reply = parse(&execute(&fake, "capabilities", ""));
    assert_eq!(reply["ok"]["api_version"], 1);
    assert_eq!(reply["ok"]["permissions"]["host_control"], true);
    let reply = parse(&execute(&fake, "session", ""));
    assert_eq!(reply["ok"]["state"], "suspended");
    assert_eq!(reply["ok"]["actions"][0]["id"], "end");
    assert!(reply.get("error").is_none());
}

#[test]
fn file_capabilities_keep_the_permissions_the_pinned_type_drops() {
    let fake = Fake::default();
    let reply = parse(&execute(&fake, "files.capabilities", ""));
    let permissions = &reply["ok"]["permissions"];
    assert_eq!(permissions["file_upload"], true);
    assert_eq!(permissions["file_download"], false);
    // Anything but a bool reads as refused, as the Rust client reads it.
    assert_eq!(permissions["usb"], false);
    assert_eq!(permissions["shared_folders"], true);
    assert_eq!(permissions["restart_shutdown"], false);
    assert_eq!(permissions["host_control"], true);
    assert_eq!(permissions["pairing_approve"], true);
    assert_eq!(reply["ok"]["features"][0], "files.upload");
    assert_eq!(fake.calls.lock().unwrap().last().unwrap(), "capabilities");
    // The plain capabilities answer the same.
    let plain = parse(&execute(&fake, "capabilities", ""));
    assert_eq!(plain, reply);
}

/// The stream menu's USB row reads `permissions.usb` from the plain `capabilities`: the
/// pinned `Capabilities` dropped it, so a host that offered USB never got the row.
#[test]
fn plain_capabilities_keep_the_usb_permission() {
    let granted = Fake {
        capabilities: Some(
            r#"{"api_version":1,"host":{"name":"PC","version":"3.0.0","unique_id":"U"},"features":["library","usb"],"permissions":{"list":true,"view":true,"launch":true,"host_control":false,"usb":true}}"#,
        ),
        ..Fake::default()
    };
    let reply = parse(&execute(&granted, "capabilities", ""));
    assert_eq!(reply["ok"]["permissions"]["usb"], true);
    assert_eq!(reply["ok"]["features"][1], "usb");
    // A host without the permission, or an older one without the field, refuses it.
    let refused = Fake {
        capabilities: Some(
            r#"{"api_version":1,"host":{"name":"PC","version":"3.0.0","unique_id":"U"},"features":["usb"],"permissions":{"list":true,"view":true,"launch":true,"host_control":false}}"#,
        ),
        ..Fake::default()
    };
    let reply = parse(&execute(&refused, "capabilities", ""));
    assert_eq!(reply["ok"]["permissions"]["usb"], false);
}

#[test]
fn coop_lists_and_joins_by_session_id_and_keeps_the_refusal() {
    let fake = Fake::default();
    let reply = parse(&execute(&fake, "coop", ""));
    assert_eq!(reply["ok"]["sessions"][0]["id"], 3);
    assert_eq!(reply["ok"]["sessions"][0]["joinable"], false);
    assert_eq!(
        reply["ok"]["sessions"][0]["refusal"],
        "This device may not join co-op sessions"
    );
    assert_eq!(
        reply["ok"]["sessions"][0]["players"][1]["name"],
        Value::Null
    );
    let reply = parse(&execute(&fake, "coop.join", "3"));
    assert_eq!(reply["ok"], json!({"session": 3, "slot": 1, "app_id": 7}));
    assert_eq!(fake.calls.lock().unwrap().last().unwrap(), "join 3");
    let calls = fake.calls.lock().unwrap().len();
    for argument in ["", "-1", "three", "4294967296"] {
        let reply = parse(&execute(&fake, "coop.join", argument));
        assert_eq!(reply["error"]["kind"], "invalid_argument", "{argument}");
    }
    assert_eq!(
        fake.calls.lock().unwrap().len(),
        calls,
        "a bad id never reaches the host"
    );
    let refused = Fake {
        fail: Some(ErrorCode::PermissionRequired),
        ..Fake::default()
    };
    let reply = parse(&execute(&refused, "coop.join", "3"));
    assert_eq!(reply["error"]["http_status"], 403);
    assert_eq!(reply["error"]["code"], "permission_required");
    assert_eq!(
        reply["error"]["message"],
        "Turn on “Can join co-op games” for this device in the Rubylight console."
    );
}

#[test]
fn session_actions_map_by_name_and_resume_is_refused() {
    let fake = Fake::default();
    for (argument, called) in [
        ("disconnect", "action Disconnect"),
        ("end", "action End"),
        ("close_game", "action CloseGame"),
    ] {
        let reply = parse(&execute(&fake, "session_action", argument));
        assert_eq!(reply["ok"]["revision"], 4, "{argument}");
        assert_eq!(fake.calls.lock().unwrap().last().unwrap(), called);
    }
    let calls = fake.calls.lock().unwrap().len();
    for argument in ["resume", "", "Close_Game"] {
        let reply = parse(&execute(&fake, "session_action", argument));
        assert_eq!(reply["error"]["kind"], "invalid_argument", "{argument}");
    }
    // Nothing reached the host for the refused ones.
    assert_eq!(fake.calls.lock().unwrap().len(), calls);
}

#[test]
fn continue_here_and_the_move_note_reach_java() {
    let fake = Fake::default();
    let reply = parse(&execute(&fake, "session_move", ""));
    assert_eq!(reply["ok"]["moved_to"]["device"], "TV");
    assert_eq!(reply["ok"]["owns_game"], true);
    assert_eq!(reply["ok"]["press_failed"], true);
    let reply = parse(&execute(&fake, "session_continue_here", ""));
    assert_eq!(reply["ok"]["revision"], 4);
    assert_eq!(fake.calls.lock().unwrap().last().unwrap(), "continue here");
}

#[test]
fn api_errors_keep_their_code_status_and_setting() {
    let fake = Fake {
        fail: Some(ErrorCode::HostControlRequired),
        ..Fake::default()
    };
    let reply = parse(&execute(&fake, "session_action", "close_game"));
    let error = &reply["error"];
    assert_eq!(error["kind"], "api");
    assert_eq!(error["code"], "host_control_required");
    assert_eq!(error["http_status"], 403);
    assert_eq!(error["setting"], "chill.enabled");
    assert_eq!(error["retry_after_ms"], 250);
    assert_eq!(error["message"], "Turn on Host control for this device.");
    assert!(reply.get("ok").is_none());
}

#[test]
fn other_errors_have_a_kind_and_the_status_when_the_host_answered() {
    let fake = Fake::default();
    let missing = parse(&execute(&fake, "library", ""));
    assert_eq!(missing["error"]["kind"], "not_found");
    assert_eq!(missing["error"]["http_status"], 404);
    assert!(missing["error"].get("code").is_none());
    assert_eq!(
        parse(&execute(&fake, "radeon", ""))["error"]["kind"],
        "cancelled"
    );
    assert_eq!(
        parse(&execute(&fake, "radeon_restore", ""))["error"]["kind"],
        "not_paired"
    );
    let status = parse(&error_envelope(&Error::Status {
        code: 502,
        message: "Bad Gateway".into(),
    }));
    assert_eq!(
        (
            status["error"]["kind"].as_str(),
            status["error"]["http_status"].as_u64()
        ),
        (Some("status"), Some(502))
    );
    let io = parse(&error_envelope(&Error::Io(std::io::Error::other(
        "refused",
    ))));
    assert_eq!(io["error"]["kind"], "network");
    assert!(io["error"].get("http_status").is_none());
}

#[test]
fn radeon_writes_parse_strictly_before_they_reach_the_host() {
    let fake = Fake::default();
    let write = r#"{"gpu":11520,"changes":[{"id":"chill.enabled","value":true},{"id":"chill.max_fps","value":144}]}"#;
    let reply = parse(&execute(&fake, "radeon_write", write));
    assert_eq!(reply["error"]["kind"], "malformed");
    assert_eq!(fake.calls.lock().unwrap().last().unwrap(), "write 11520 2");

    let calls = fake.calls.lock().unwrap().len();
    for bad in ["", "{}", r#"{"gpu":1,"changes":[],"extra":1}"#] {
        let reply = parse(&execute(&fake, "radeon_write", bad));
        assert_eq!(reply["error"]["kind"], "invalid_argument", "{bad}");
    }
    assert_eq!(fake.calls.lock().unwrap().len(), calls);
}

#[test]
fn power_ops_carry_the_reply_and_refusals_keep_it() {
    let fake = Fake::default();
    let offer = parse(&execute(&fake, "power.capabilities", ""));
    assert_eq!(offer["ok"]["token"], TOKEN);
    assert_eq!(
        (
            offer["ok"]["status"].as_u64(),
            offer["ok"]["sleep_available"].as_bool()
        ),
        (Some(200), Some(true))
    );
    let accepted = parse(&execute(&fake, "power.sleep", TOKEN));
    assert_eq!(
        (
            accepted["ok"]["accepted"].as_bool(),
            &accepted["ok"]["token"]
        ),
        (Some(true), &Value::Null)
    );
    assert_eq!(
        fake.calls.lock().unwrap().last().unwrap(),
        &format!("sleep {TOKEN}")
    );

    let refused = parse(&execute(&fake, "power.sleep", "spent"));
    let error = &refused["error"];
    assert_eq!(
        (error["kind"].as_str(), error["refusal"].as_str()),
        (Some("power_refused"), Some("conflict"))
    );
    assert_eq!(
        (error["http_status"].as_u64(), error["message"].as_str()),
        (Some(409), Some("Sleep token expired or already used"))
    );
    assert_eq!(error["power"]["accepted"], false);
    assert!(refused.get("ok").is_none());
    let invalid = parse(&execute(&fake, "power.sleep", ""));
    assert_eq!(invalid["error"]["kind"], "invalid_argument");
}

const GUESTS: &str = r#"{"policy":"ask","default_permission":"controller","manage":true,"you":null,"owner":"Fold7","max_guests":4,"version":2,"keyboard_mouse":null,"guests":[],"waiting":[{"id":3,"name":"TV","seconds_left":40}]}"#;

#[test]
fn guests_reach_the_host_and_bad_actions_do_not() {
    let fake = Fake::default();
    assert_eq!(parse(&execute(&fake, "guests", ""))["ok"]["waiting"][0]["name"], "TV");
    let approved = parse(&execute(&fake, "guests.action", r#"{"action":"approve","id":3}"#));
    assert_eq!(approved["ok"]["owner"], "Fold7");
    let bad = parse(&execute(&fake, "guests.action", r#"{"action":"permission","id":3,"permission":"admin"}"#));
    assert_eq!(bad["error"]["kind"], "invalid_argument");
    assert_eq!(*fake.calls.lock().unwrap(), ["guests", "guest session/guests/approve"]);
}

#[test]
fn pairing_requests_reach_the_host_and_a_bad_pin_never_leaves_this_device() {
    let fake = Fake::default();
    let list = parse(&execute(&fake, "pairing.requests", ""));
    assert_eq!(list["ok"]["requests"][0]["name"], "TV");
    assert_eq!(list["ok"]["requests"][0].get("certificate"), None);
    let approve = |pin: &str| json!({ "id": "a1", "pin": pin }).to_string();
    assert_eq!(parse(&execute(&fake, "pairing.approve", &approve("1234")))["ok"]["name"], "TV");
    let wrong = parse(&execute(&fake, "pairing.approve", &approve("0000")));
    assert_eq!(wrong["error"]["message"], "That is not the PIN TV shows.");
    assert_eq!(wrong["error"]["http_status"], 422);
    for bad in ["123", "12345", "12a4", ""] {
        let refused = parse(&execute(&fake, "pairing.approve", &approve(bad)));
        assert_eq!(refused["error"]["kind"], "invalid_argument", "{bad}");
    }
    assert_eq!(
        parse(&execute(&fake, "pairing.approve", "nonsense"))["error"]["kind"],
        "invalid_argument"
    );
    assert_eq!(parse(&execute(&fake, "pairing.deny", "a1"))["ok"]["requests"], json!([]));
    assert_eq!(
        *fake.calls.lock().unwrap(),
        ["pairing requests", "approve a1 1234", "approve a1 0000", "deny a1"]
    );
}

#[test]
fn restart_shut_down_and_update_reach_the_host_and_refusals_keep_its_words() {
    let fake = Fake::default();
    let offer = parse(&execute(&fake, "pc_power", ""));
    assert_eq!(offer["ok"]["token"], TOKEN);
    assert_eq!(offer["ok"]["restart"]["available"], true);
    assert_eq!(offer["ok"]["wake_from_off"], false);
    assert_eq!(offer["ok"]["last_refusal_unconfirmed"], false);
    assert_eq!(parse(&execute(&fake, "pc_power.restart", TOKEN)), json!({ "ok": {} }));
    assert_eq!(parse(&execute(&fake, "pc_power.shutdown", TOKEN)), json!({ "ok": {} }));
    let refused = parse(&execute(&fake, "pc_power.shutdown", "spent"));
    let error = &refused["error"];
    assert_eq!(
        (error["kind"].as_str(), error["http_status"].as_u64()),
        (Some("api"), Some(409))
    );
    assert_eq!(error["message"], "Living room TV is streaming from this PC.");
    let update = parse(&execute(&fake, "update", ""));
    assert_eq!(update["ok"]["latest_version"], "v3.4.0");
    // The shared rules' verdicts ride along, so Java decides as desktop does.
    assert_eq!(
        (&update["ok"]["offered"], &update["ok"]["installing_now"]),
        (&json!("3.4.0"), &json!(false))
    );
    assert_eq!(
        (&update["ok"]["failed"], &update["ok"]["failure"]),
        (&json!(false), &Value::Null)
    );
    assert!(update["ok"].get("gave_up").is_none() && update["ok"].get("arrived").is_none());
    let following = parse(&execute(&fake, "update", "3.4.0"));
    assert_eq!((&following["ok"]["gave_up"], &following["ok"]["arrived"]), (&json!(true), &json!(false)));
    // Back on the target, by the shared rule: trimmed, one leading v.
    let back = parse(&execute(&fake, "update", " v3.3.0 "));
    assert_eq!((&back["ok"]["gave_up"], &back["ok"]["arrived"]), (&json!(false), &json!(true)));
    let install = parse(&execute(&fake, "update.install", ""));
    assert_eq!(
        (&install["ok"]["phase"], &install["ok"]["in_progress"]),
        (&json!("downloading"), &json!(true))
    );
    assert_eq!(
        fake.calls.lock().unwrap().clone(),
        [
            "pc power".to_owned(),
            format!("power/restart {TOKEN}"),
            format!("power/shutdown {TOKEN}"),
            "power/shutdown spent".to_owned(),
            "update".to_owned(),
            "update".to_owned(),
            "update".to_owned(),
            "update install".to_owned(),
        ]
    );
}

#[test]
fn the_couch_menu_parses_strictly_and_keeps_api_errors() {
    let fake = Fake::default();
    let reply = parse(&execute(&fake, "couch_menu", r#"{"open":true}"#));
    assert_eq!(reply["ok"]["open"], true);
    assert_eq!(
        fake.calls.lock().unwrap().last().unwrap(),
        "couch menu true"
    );

    let calls = fake.calls.lock().unwrap().len();
    for bad in ["", "{}", "true", r#"{"open":true,"page":"radeon"}"#] {
        let reply = parse(&execute(&fake, "couch_menu", bad));
        assert_eq!(reply["error"]["kind"], "invalid_argument", "{bad}");
    }
    assert_eq!(fake.calls.lock().unwrap().len(), calls);

    let fake = Fake {
        fail: Some(ErrorCode::NoSession),
        ..Fake::default()
    };
    let reply = parse(&execute(&fake, "couch_menu", r#"{"open":true}"#));
    assert_eq!(reply["error"]["kind"], "api");
    assert_eq!(reply["error"]["code"], "no_session");
    assert_eq!(reply["error"]["http_status"], ErrorCode::NoSession.status());
    assert_eq!(reply["error"]["message"], "Start a stream first.");
}

#[test]
fn wake_answers_with_the_targets() {
    let fake = Fake::default();
    let reply = parse(&execute(&fake, "wake", ""));
    assert_eq!(reply["ok"]["primary"], "02:7c:16:b4:13:4f");
    assert_eq!(reply["ok"]["targets"][0]["broadcasts"][0], "192.168.4.255");
    assert_eq!(fake.calls.lock().unwrap().last().unwrap(), "wake");
}

#[test]
fn an_unknown_operation_is_an_argument_error() {
    let reply = parse(&execute(&Fake::default(), "reboot", ""));
    assert_eq!(reply["error"]["kind"], "invalid_argument");
}

#[test]
fn an_unusable_identity_or_certificate_is_reported_not_thrown() {
    let endpoint = Endpoint {
        address: "192.0.2.1".into(),
        http_port: 47989,
        https_port: 47984,
        server_certificate_pem: String::new(),
        unique_id: "0123456789ABCDEF".into(),
        client_certificate_der: vec![1, 2, 3],
        client_private_key_pkcs8: vec![4, 5, 6],
    };
    // Bytes that are no certificate come back as an error envelope, not a panic.
    let reply = parse(&request(&endpoint, "session", ""));
    assert!(reply.get("ok").is_none());
    assert!(["malformed", "identity"].contains(&reply["error"]["kind"].as_str().unwrap()));
    let reply = parse(&request(
        &Endpoint {
            unique_id: "not valid!".into(),
            ..endpoint
        },
        "session",
        "",
    ));
    assert_eq!(reply["error"]["kind"], "identity");
}

#[test]
fn a_generated_identity_reaches_the_certificate_check() {
    let identity = ClientIdentity::generate().unwrap();
    let endpoint = Endpoint {
        address: "192.0.2.1".into(),
        http_port: 47989,
        https_port: 47984,
        server_certificate_pem: "not a certificate".into(),
        unique_id: identity.unique_id().to_string(),
        client_certificate_der: identity.certificate_der().to_vec(),
        client_private_key_pkcs8: identity.private_key_pkcs8().to_vec(),
    };
    let reply = parse(&request(&endpoint, "library", ""));
    assert_ne!(reply["error"]["kind"], Value::Null);
    assert!(reply.get("ok").is_none());
}
