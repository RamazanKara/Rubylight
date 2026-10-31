use super::*;

fn toggle(id: &'static str, on: Query<bool>) -> FieldRaw {
    FieldRaw {
        id,
        label: id,
        kind: Kind::Toggle,
        unit: None,
        value: Some(on.map(Value::Bool)),
        range: None,
        options: vec![],
        options_queried: false,
        note: None,
    }
}
fn integer(id: &'static str, value: i64, range: Query<Range>) -> FieldRaw {
    FieldRaw {
        id,
        label: id,
        kind: Kind::Integer,
        unit: Some("fps"),
        value: Some(Ok(Value::Int(value))),
        range: Some(range),
        options: vec![],
        options_queried: false,
        note: None,
    }
}
fn choice(id: &'static str, items: EnumItems, raw: i32) -> FieldRaw {
    FieldRaw {
        id,
        label: id,
        kind: Kind::Choice,
        unit: None,
        value: Some(Ok(Value::Choice(enum_name(items, raw)))),
        range: None,
        options: enum_options(items),
        options_queried: false,
        note: None,
    }
}
fn group(group: &'static str, supported: Option<Query<bool>>, fields: Vec<FieldRaw>) -> GroupRaw {
    GroupRaw {
        group,
        interface: "IADLX3DFake",
        obtained: Ok(()),
        supported,
        fields,
    }
}
const RANGE: Range = Range {
    min: 30,
    max: 300,
    step: 1,
};

#[test]
fn supported_toggle_is_configurable_with_its_value() {
    let s = &classify_group(&group(
        "anti_lag",
        Some(Ok(true)),
        vec![toggle("anti_lag.enabled", Ok(true))],
    ))[0];
    assert_eq!(s.state, State::Configurable);
    assert_eq!(s.supported, Some(true));
    assert_eq!(s.readable, Some(true));
    assert_eq!(s.writable, Writable::Yes);
    assert_eq!(s.value, Some(Value::Bool(true)));
}

#[test]
fn unsupported_feature_has_no_value_and_is_not_writable() {
    let s = &classify_group(&group(
        "chill",
        Some(Ok(false)),
        vec![toggle("chill.enabled", Ok(true))],
    ))[0];
    assert_eq!(s.state, State::Unsupported);
    assert_eq!(s.supported, Some(false));
    assert_eq!(s.readable, Some(false));
    assert_eq!(s.writable, Writable::No);
    assert!(s.value.is_none());
}

#[test]
fn missing_interface_and_not_supported_codes_are_unsupported() {
    for fault in [
        Fault::Missing,
        Fault::Code(6),
        Fault::Code(9),
        Fault::Code(12),
    ] {
        let mut g = group(
            "afmf",
            Some(Ok(true)),
            vec![toggle("afmf.enabled", Ok(true))],
        );
        g.obtained = Err(fault);
        assert_eq!(classify_group(&g)[0].state, State::Unsupported, "{fault:?}");
    }
    let g = group(
        "afmf",
        Some(Err(Fault::Code(12))),
        vec![toggle("afmf.enabled", Ok(true))],
    );
    assert_eq!(classify_group(&g)[0].state, State::Unsupported);
}

#[test]
fn other_failures_are_temporarily_unavailable_and_loud() {
    let mut g = group(
        "afmf",
        Some(Ok(true)),
        vec![toggle("afmf.enabled", Ok(true))],
    );
    g.obtained = Err(Fault::Code(3));
    let s = &classify_group(&g)[0];
    assert_eq!(s.state, State::TemporarilyUnavailable);
    assert_eq!(s.supported, None);
    assert!(s.notes.iter().any(|n| n.contains("ADLX_FAIL")));
    let g = group(
        "afmf",
        Some(Ok(true)),
        vec![toggle("afmf.enabled", Err(Fault::Code(17)))],
    );
    let s = &classify_group(&g)[0];
    assert_eq!(s.state, State::TemporarilyUnavailable);
    assert_eq!(s.readable, Some(false));
    assert_eq!(s.adlx["read"], "ADLX_NOT_ACTIVE");
}

#[test]
fn integer_with_a_usable_range_is_configurable() {
    let s = &classify_group(&group(
        "chill",
        Some(Ok(true)),
        vec![integer("chill.max_fps", 144, Ok(RANGE))],
    ))[0];
    assert_eq!(s.state, State::Configurable);
    assert_eq!(s.range, Some(RANGE));
    assert_eq!(s.writable, Writable::Yes);
}

#[test]
fn empty_or_stepless_range_is_read_only_and_not_writable() {
    for range in [
        Range {
            min: 60,
            max: 60,
            step: 1,
        },
        Range {
            min: 30,
            max: 300,
            step: 0,
        },
        Range {
            min: 300,
            max: 30,
            step: 1,
        },
    ] {
        let s = &classify_group(&group(
            "chill",
            Some(Ok(true)),
            vec![integer("chill.max_fps", 60, Ok(range))],
        ))[0];
        assert_eq!(s.state, State::ReadOnly, "{range:?}");
        assert_eq!(s.writable, Writable::No);
        assert_eq!(s.readable, Some(true));
    }
}

#[test]
fn failed_range_query_leaves_writability_unverified() {
    let s = &classify_group(&group(
        "chill",
        Some(Ok(true)),
        vec![integer("chill.max_fps", 144, Err(Fault::Code(3)))],
    ))[0];
    assert_eq!(s.state, State::ReadOnly);
    assert_eq!(s.writable, Writable::Unverified);
    assert_eq!(s.value, Some(Value::Int(144)));
    assert!(s.range.is_none());
}

#[test]
fn enum_backed_choice_is_unverified_and_read_only_until_a_run_proves_it() {
    let s = &classify_group(&group(
        "sdk",
        Some(Ok(true)),
        vec![choice("sdk.unproven_mode", WAIT_FOR_VERTICAL_REFRESH, 1)],
    ))[0];
    assert_eq!(s.state, State::ReadOnly);
    assert_eq!(s.writable, Writable::Unverified);
    assert_eq!(
        s.value,
        Some(Value::Choice("off_unless_app_specifies".into()))
    );
    assert_eq!(s.options.len(), 4);
    assert!(s.notes.iter().any(|n| n.contains("no query")));
}

#[test]
fn a_choice_proven_on_hardware_is_configurable() {
    let s = &classify_group(&group(
        "vsync",
        Some(Ok(true)),
        vec![choice("vsync.mode", WAIT_FOR_VERTICAL_REFRESH, 1)],
    ))[0];
    assert_eq!(s.state, State::Configurable);
    assert_eq!(s.writable, Writable::Yes);
    assert_eq!(s.options.len(), 4);
}

#[test]
fn afmf_modes_say_they_need_afmf_on() {
    let s = &classify_group(&group(
        "afmf",
        Some(Ok(true)),
        vec![choice("afmf.search_mode", AFMF_SEARCH_MODE, 0)],
    ))[0];
    assert_eq!(s.state, State::Configurable);
    assert!(s.notes.iter().any(|n| n.contains("afmf.enabled")));
}

#[test]
fn levels_that_cannot_be_written_back_are_never_configurable() {
    for (id, items) in [
        ("anti_aliasing.level", AA_LEVEL),
        ("anisotropic_filtering.level", AF_LEVEL),
    ] {
        let s = &classify_group(&group("level", Some(Ok(true)), vec![choice(id, items, 0)]))[0];
        assert_eq!(s.state, State::ReadOnly, "{id}");
        assert_eq!(s.writable, Writable::No, "{id}");
        assert!(s.notes.iter().any(|n| n.contains("could not be restored")));
    }
}

#[test]
fn queried_option_list_makes_a_choice_writable() {
    let mut field = choice("fidelityfx_frame_gen.ratio", FFX_FRAME_GEN_RATIO, 1);
    field.options_queried = true;
    let s = &classify_group(&group("fidelityfx_frame_gen", Some(Ok(true)), vec![field]))[0];
    assert_eq!(s.writable, Writable::Yes);
    assert_eq!(s.state, State::Configurable);
}

#[test]
fn action_has_no_value_and_follows_supported() {
    let action = FieldRaw {
        id: "reset_shader_cache.run",
        label: "reset",
        kind: Kind::Action,
        unit: None,
        value: None,
        range: None,
        options: vec![],
        options_queried: false,
        note: None,
    };
    let s = &classify_group(&group("reset_shader_cache", Some(Ok(true)), vec![action]))[0];
    assert_eq!(s.readable, None);
    assert_eq!(s.state, State::Configurable);
    assert!(s.value.is_none());
}

#[test]
fn out_of_range_value_is_noted_not_hidden() {
    let s = &classify_group(&group(
        "chill",
        Some(Ok(true)),
        vec![integer("chill.max_fps", 400, Ok(RANGE))],
    ))[0];
    assert_eq!(s.value, Some(Value::Int(400)));
    assert!(s.notes.iter().any(|n| n.contains("outside")));
}

#[test]
fn enum_names_cover_known_and_unknown_values() {
    assert_eq!(enum_name(WAIT_FOR_VERTICAL_REFRESH, 3), "always_on");
    assert_eq!(enum_name(ANTILAG_STATE, 1), "anti_lag_next");
    assert_eq!(enum_name(AF_LEVEL, 16), "16x");
    assert_eq!(enum_name(AA_LEVEL, 0), "invalid");
    assert_eq!(enum_name(AA_LEVEL, 99), "unknown(99)");
    assert_eq!(enum_name(TESSELLATION_LEVEL, 99), "unknown(99)");
    assert_eq!(result_name(12), "ADLX_NOT_SUPPORTED");
    assert_eq!(result_name(77), "ADLX_RESULT(77)");
}

#[test]
fn dependencies_report_whether_the_other_side_is_on() {
    let mut settings = vec![];
    settings.extend(classify_group(&group(
        "anti_lag",
        Some(Ok(true)),
        vec![toggle("anti_lag.enabled", Ok(false))],
    )));
    settings.extend(classify_group(&group(
        "chill",
        Some(Ok(true)),
        vec![toggle("chill.enabled", Ok(true))],
    )));
    settings.extend(classify_group(&group(
        "enhanced_sync",
        Some(Ok(true)),
        vec![toggle("enhanced_sync.enabled", Ok(false))],
    )));
    settings.extend(classify_group(&group(
        "vsync",
        Some(Ok(true)),
        vec![choice("vsync.mode", WAIT_FOR_VERTICAL_REFRESH, 3)],
    )));
    apply_dependencies(&mut settings);
    let by_id = |id: &str| settings.iter().find(|s| s.id == id).unwrap();
    let anti_lag = by_id("anti_lag.enabled");
    assert_eq!(anti_lag.depends.len(), 1);
    assert_eq!(anti_lag.depends[0].group, "chill");
    assert_eq!(anti_lag.depends[0].active, Some(true));
    let chill = by_id("chill.enabled");
    let others: Vec<_> = chill.depends.iter().map(|d| (d.group, d.active)).collect();
    assert_eq!(others, vec![("anti_lag", Some(false)), ("boost", None)]);
    assert_eq!(by_id("enhanced_sync.enabled").depends[0].active, Some(true));
    assert_eq!(by_id("vsync.mode").depends[0].active, Some(false));
}

#[test]
fn json_uses_snake_case_states_and_omits_absent_fields() {
    let s = classify_group(&group(
        "chill",
        Some(Ok(false)),
        vec![toggle("chill.enabled", Ok(true))],
    ));
    let json = serde_json::to_value(&s[0]).unwrap();
    assert_eq!(json["state"], "unsupported");
    assert_eq!(json["writable"], "no");
    assert!(json.get("value").is_none());
    assert_eq!(json["adlx"]["is_supported"], "ADLX_OK");
}

#[test]
fn markdown_rows_carry_state_value_range_and_notes() {
    let mut settings = classify_group(&group(
        "chill",
        Some(Ok(true)),
        vec![integer("chill.max_fps", 144, Ok(RANGE))],
    ));
    apply_dependencies(&mut settings);
    let table = markdown_table(&settings);
    let row = table.lines().last().unwrap();
    assert!(row.starts_with("| `chill.max_fps` |"), "{row}");
    assert!(
        row.contains("| yes | yes | yes | configurable | 144 fps | 30..300 step 1 |"),
        "{row}"
    );
    assert!(row.contains("excludes anti_lag"), "{row}");
}
