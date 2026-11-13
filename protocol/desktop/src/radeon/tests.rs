use super::*;
use nightjar_core::http::native_api::error::ApiError;

fn radeon(json: &str) -> Radeon {
    serde_json::from_str(json).expect("valid Radeon json")
}

/// The doc's example answer: all five states and a pending restore.
fn fixture(writable: bool, host_control: bool) -> Radeon {
    radeon(&format!(
        r#"{{
              "revision": 7, "state": "configurable", "host_control": {host_control}, "writable_now": {writable},
              "gpus": [{{ "id": -11520, "name": "AMD Radeon RX 7900 XT", "driver": "25.9.1", "settings": [
                {{ "id": "anti_lag.enabled", "group": "anti_lag", "kind": "toggle", "state": "configurable", "value": true }},
                {{ "id": "chill.max_fps", "group": "chill", "kind": "integer", "unit": "fps", "state": "configurable",
                   "value": 144, "range": {{ "min": 30, "max": 300, "step": 1 }}, "restore": {{ "original": 120, "written": 144 }} }},
                {{ "id": "afmf.enabled", "group": "afmf", "kind": "toggle", "state": "needs_external_setup", "value": false,
                   "reason": "external_setup", "message": "Turn on AMD Fluid Motion Frames once in AMD Software." }},
                {{ "id": "rsr.enabled", "group": "rsr", "kind": "toggle", "state": "unsupported" }},
                {{ "id": "image_sharpening.sharpness", "group": "image_sharpening", "kind": "integer", "unit": "%",
                   "state": "temporarily_unavailable", "reason": "driver_busy", "message": "The driver is busy." }},
                {{ "id": "enhanced_sync.enabled", "group": "enhanced_sync", "kind": "toggle", "state": "read_only", "value": true }},
                {{ "id": "vsync.mode", "group": "vsync", "kind": "choice", "state": "configurable", "value": "off",
                   "options": [{{ "value": "off", "label": "Off" }}, {{ "value": "on", "label": "Always on" }}] }},
                {{ "id": "future_thing.gain", "group": "future_thing", "kind": "integer", "unit": "x",
                   "state": "configurable", "value": 3, "range": {{ "min": 1, "max": 9, "step": 2 }} }}
              ] }}] }}"#
    ))
}

fn mode_of(panel: &Panel, id: &str) -> Mode {
    panel.find(id).expect(id).mode
}

#[test]
fn each_of_the_five_states_draws_its_own_way() {
    let panel = Panel::of(&fixture(true, true));
    assert_eq!(panel.notice, Notice::None);
    assert_eq!(mode_of(&panel, "anti_lag.enabled"), Mode::Edit);
    assert_eq!(mode_of(&panel, "chill.max_fps"), Mode::Edit);
    assert_eq!(mode_of(&panel, "enhanced_sync.enabled"), Mode::Value);
    assert_eq!(mode_of(&panel, "afmf.enabled"), Mode::ExternalSetup);
    assert_eq!(mode_of(&panel, "rsr.enabled"), Mode::Unsupported);
    assert_eq!(mode_of(&panel, "image_sharpening.sharpness"), Mode::Unavailable);
    // The notes under the rows that cannot be edited: the host's message, else ours.
    assert_eq!(panel.find("afmf.enabled").unwrap().state_note().unwrap(), "Turn on AMD Fluid Motion Frames once in AMD Software.");
    assert_eq!(panel.find("rsr.enabled").unwrap().state_note().unwrap(), "Not supported on this GPU or driver");
    assert_eq!(panel.find("image_sharpening.sharpness").unwrap().state_note().unwrap(), "The driver is busy.");
    assert_eq!(panel.find("anti_lag.enabled").unwrap().state_note(), None);
    // Values: needs-external-setup shows its value, unsupported and unavailable have none.
    assert_eq!(panel.find("afmf.enabled").unwrap().value_text(), "Off");
    assert_eq!(panel.find("rsr.enabled").unwrap().value_text(), "Not available right now");
}

#[test]
fn nothing_is_editable_unless_the_host_says_writable_now() {
    // Host control but no owned session: configurable settings show as values, with the reason.
    let panel = Panel::of(&fixture(false, true));
    assert_eq!(panel.notice, Notice::NeedsSession);
    assert!(!panel.can_write());
    assert!(panel.gpus[0].rows.iter().all(|r| r.mode != Mode::Edit));
    assert_eq!(mode_of(&panel, "anti_lag.enabled"), Mode::Value);
    // Not even host control: the notice says to turn it on.
    let panel = Panel::of(&fixture(false, false));
    assert_eq!(panel.notice, Notice::NeedsHostControl);
    assert!(panel.gpus[0].rows.iter().all(|r| r.mode != Mode::Edit));
}

#[test]
fn a_whole_panel_state_overrides_the_rows() {
    let mut read_only = fixture(true, true);
    read_only.state = ControlState::ReadOnly;
    let panel = Panel::of(&read_only);
    assert_eq!(panel.notice, Notice::ReadOnly);
    assert!(panel.gpus[0].rows.iter().all(|r| r.mode != Mode::Edit), "read only panel offers no control");

    let mut adlx = fixture(false, true);
    adlx.state = ControlState::TemporarilyUnavailable;
    adlx.reason = Some("adlx_unavailable".into());
    adlx.gpus.clear();
    let panel = Panel::of(&adlx);
    assert_eq!(panel.notice, Notice::AdlxUnavailable);
    assert!(panel.gpus.is_empty());
    adlx.reason = Some("driver_busy".into());
    assert_eq!(Panel::of(&adlx).notice, Notice::Unavailable);

    let mut external = fixture(true, true);
    external.state = ControlState::NeedsExternalSetup;
    assert_eq!(Panel::of(&external).notice, Notice::NeedsExternalSetup);
    let mut unsupported = fixture(true, true);
    unsupported.state = ControlState::Unsupported;
    assert_eq!(Panel::of(&unsupported).notice, Notice::Unsupported);
    // A state from a newer host reads as unsupported.
    unsupported.state = ControlState::Unknown;
    assert_eq!(Panel::of(&unsupported).notice, Notice::Unsupported);
    assert!(Panel::of(&unsupported).gpus[0].rows.iter().all(|r| r.mode != Mode::Edit));
}

#[test]
fn a_control_needs_a_value_and_room_to_move() {
    let mut data = fixture(true, true);
    let settings = &mut data.gpus[0].settings;
    settings[0].value = None; // a toggle with no value
    settings[1].range = Some(Range { min: 5, max: 5, step: 1 }); // nothing to slide
    settings[6].options.clear(); // a choice with no options
    let panel = Panel::of(&data);
    assert_eq!(mode_of(&panel, "anti_lag.enabled"), Mode::Value);
    assert_eq!(mode_of(&panel, "chill.max_fps"), Mode::Value);
    assert_eq!(mode_of(&panel, "vsync.mode"), Mode::Value);
    // A kind from a newer host is shown, never edited.
    let mut data = fixture(true, true);
    data.gpus[0].settings[0].kind = Kind::Unknown;
    assert_eq!(mode_of(&Panel::of(&data), "anti_lag.enabled"), Mode::Value);
}

#[test]
fn names_come_from_known_ids_and_from_the_words_of_unknown_ones() {
    let panel = Panel::of(&fixture(true, true));
    assert_eq!(panel.find("anti_lag.enabled").unwrap().title(), "Radeon Anti-Lag");
    assert_eq!(panel.find("chill.max_fps").unwrap().title(), "Radeon Chill: Maximum FPS");
    assert_eq!(panel.find("vsync.mode").unwrap().title(), "Vertical sync");
    assert_eq!(panel.find("future_thing.gain").unwrap().title(), "Future thing: Gain");
    let mut data = fixture(true, true);
    data.gpus[0].settings[7].id = "future_thing.enabled".into();
    assert_eq!(Panel::of(&data).find("future_thing.enabled").unwrap().title(), "Future thing");
}

#[test]
fn nvidia_settings_are_named_by_their_group_alone() {
    let setting = |id: &str, kind: &str, value: &str| {
        let group = id.split('.').next().unwrap();
        format!(r#"{{ "id": "{id}", "group": "{group}", "kind": "{kind}", "state": "configurable", "value": {value} }}"#)
    };
    let settings = [
        ("nvidia_low_latency.mode", "choice", r#""ultra""#, "Low Latency Mode"),
        ("nvidia_frame_rate_limit.fps", "integer", "0", "Max frame rate"),
        ("nvidia_vsync.mode", "choice", r#""fast""#, "Vertical sync"),
        ("nvidia_power.mode", "choice", r#""adaptive""#, "Power management mode"),
        ("nvidia_texture_filtering.quality", "choice", r#""quality""#, "Texture filtering quality"),
        ("nvidia_anisotropic_filtering.mode", "choice", r#""16x""#, "Anisotropic filtering"),
        ("nvidia_smooth_motion.enabled", "toggle", "true", "Smooth Motion"),
    ];
    let rows: Vec<String> = settings.iter().map(|(id, kind, value, _)| setting(id, kind, value)).collect();
    let data = radeon(&format!(
        r#"{{ "revision": 1, "state": "configurable", "host_control": true, "writable_now": true,
                 "gpus": [{{ "id": 1, "name": "NVIDIA GeForce RTX 4080", "driver": "581.57", "settings": [{}] }}] }}"#,
        rows.join(",")
    ));
    let panel = Panel::of(&data);
    for (id, _, _, title) in settings {
        assert_eq!(panel.find(id).unwrap().title(), title, "{id}");
    }
}

#[test]
fn a_mode_field_of_an_unknown_group_leaves_the_name_alone() {
    let mut data = fixture(true, true);
    data.gpus[0].settings[7].id = "future_thing.mode".into();
    assert_eq!(Panel::of(&data).find("future_thing.mode").unwrap().title(), "Future thing");
}

#[test]
fn names_follow_the_language() {
    let panel = Panel::of(&fixture(true, true));
    crate::strings::set(crate::strings::Lang::De);
    assert_eq!(panel.find("chill.max_fps").unwrap().title(), "Radeon Chill: Maximale FPS");
    assert_eq!(panel.find("vsync.mode").unwrap().title(), "Vertikale Synchronisierung");
    // A name made from an id this client does not know stays as the host sent it.
    assert_eq!(panel.find("future_thing.gain").unwrap().title(), "Future thing: Gain");
}

#[test]
fn an_unknown_id_draws_from_its_kind_range_and_unit() {
    let panel = Panel::of(&fixture(true, true));
    let row = panel.find("future_thing.gain").unwrap();
    assert_eq!(row.mode, Mode::Edit);
    assert_eq!(row.value_text(), "3 x");
    // The host's step of 2 is kept on the way up and clamped to the range at the end.
    assert_eq!(row.step(1), Some(Value::Integer(5)));
    let mut top = row.clone();
    top.value = Some(Value::Integer(9));
    assert_eq!(top.step(1), Some(Value::Integer(9)));
}

#[test]
fn steps_stay_in_range_and_cycle_choices() {
    let panel = Panel::of(&fixture(true, true));
    let fps = panel.find("chill.max_fps").unwrap();
    // 30..300 has 270 values: 20 presses cross it, so one press is 13.
    assert_eq!(fps.unit_step(), Some(13));
    assert_eq!(fps.step(1), Some(Value::Integer(157)));
    assert_eq!(fps.step(-1), Some(Value::Integer(131)));
    let mut low = fps.clone();
    low.value = Some(Value::Integer(31));
    assert_eq!(low.step(-1), Some(Value::Integer(30)));
    assert_eq!(panel.find("anti_lag.enabled").unwrap().step(1), Some(Value::Bool(false)));
    let sync = panel.find("vsync.mode").unwrap();
    assert_eq!(sync.step(1), Some(Value::Choice("on".into())));
    let mut last = sync.clone();
    last.value = Some(Value::Choice("on".into()));
    assert_eq!(last.step(1), Some(Value::Choice("off".into())), "a choice wraps around");
    assert_eq!(sync.value_text(), "Off");
    assert_eq!(last.value_text(), "Always on");
    // A row with no value has no step.
    assert_eq!(panel.find("rsr.enabled").unwrap().step(1), None);
}

#[test]
fn a_range_that_overflows_is_not_editable_and_never_panics() {
    let mut data = fixture(true, true);
    data.gpus[0].settings[1].range = Some(Range { min: i64::MIN, max: i64::MAX, step: 1 });
    let panel = Panel::of(&data);
    let row = panel.find("chill.max_fps").unwrap();
    assert_eq!(row.mode, Mode::Value, "a span that does not fit i64 is shown, not edited");
    assert_eq!(row.unit_step(), None);
    assert_eq!(row.step(1), None);
    // Arithmetic at the edges of a range that is allowed gives a value or None, never a wrap.
    let mut edge = Panel::of(&fixture(true, true)).find("chill.max_fps").unwrap().clone();
    edge.range = Some(Range { min: i64::MAX - 100, max: i64::MAX, step: 7 });
    edge.value = Some(Value::Integer(i64::MAX));
    assert_eq!(edge.step(1), None, "one step past i64::MAX overflows");
    assert_eq!(edge.step(-1), Some(Value::Integer(i64::MAX - 100 + 13 * 7)));
    edge.value = Some(Value::Integer(i64::MIN));
    edge.range = Some(Range { min: 0, max: 100, step: 1 });
    assert_eq!(edge.step(-1), None);
}

#[test]
fn a_pending_change_says_what_the_session_end_restores() {
    let panel = Panel::of(&fixture(true, true));
    assert_eq!(panel.find("chill.max_fps").unwrap().restore_hint().unwrap(), "Goes back to 120 fps when you end the session");
    assert_eq!(panel.find("anti_lag.enabled").unwrap().restore_hint(), None);
    let mut data = fixture(true, true);
    data.gpus[0].settings[0].restore = Some(Restore { original: Value::Bool(false), written: Value::Bool(true) });
    data.gpus[0].settings[6].restore =
        Some(Restore { original: Value::Choice("on".into()), written: Value::Choice("off".into()) });
    let panel = Panel::of(&data);
    assert_eq!(panel.find("anti_lag.enabled").unwrap().restore_hint().unwrap(), "Goes back to Off when you end the session");
    assert_eq!(panel.find("vsync.mode").unwrap().restore_hint().unwrap(), "Goes back to Always on when you end the session");
}

fn api_error(status: u16, code: ErrorCode, setting: Option<&str>) -> Error {
    Error::Api {
        status,
        error: ApiError { code, message: "host words".into(), setting: setting.map(str::to_owned), retry_after_ms: None },
    }
}

#[test]
fn failures_follow_the_hosts_code_and_name_the_setting() {
    let failure = Failure::of(&api_error(422, ErrorCode::InvalidValue, Some("chill.max_fps")), Some("anti_lag.enabled"));
    assert_eq!(failure.kind, FailureKind::InvalidValue);
    assert_eq!(failure.setting.as_deref(), Some("chill.max_fps"), "the host's setting wins");
    assert_eq!(failure.text(), "The PC does not accept that value.");
    let failure = Failure::of(&api_error(409, ErrorCode::DriverRejected, None), Some("chill.max_fps"));
    assert_eq!(failure.setting.as_deref(), Some("chill.max_fps"), "else the one that was written");
    assert_eq!(failure.text(), "The graphics driver rejected this change.");
    assert_eq!(Failure::of(&api_error(403, ErrorCode::HostControlRequired, None), None).kind, FailureKind::HostControl);
    assert_eq!(Failure::of(&api_error(409, ErrorCode::NotSessionOwner, None), None).kind, FailureKind::NoSession);
    assert_eq!(Failure::of(&api_error(409, ErrorCode::SessionBusy, None), None).kind, FailureKind::Busy);
    assert_eq!(Failure::of(&api_error(503, ErrorCode::Unknown, None), None).kind, FailureKind::Busy);
    let generic = Failure::of(&api_error(500, ErrorCode::Internal, None), None);
    assert_eq!(generic.text(), "host words", "an error we have no text for shows the host's");
    let unreachable = Failure::of(&Error::Io(std::io::Error::from(std::io::ErrorKind::TimedOut)), None);
    assert_eq!(unreachable.text(), "Cannot reach the PC.");
}

#[test]
fn a_host_without_radeon_endpoints_is_told_apart() {
    assert!(is_missing(&Error::NotFound("radeon".into())));
    assert!(is_missing(&api_error(404, ErrorCode::NotFound, None)));
    assert!(!is_missing(&api_error(409, ErrorCode::NoSession, None)));
    assert!(!is_missing(&Error::Io(std::io::Error::from(std::io::ErrorKind::TimedOut))));
}
