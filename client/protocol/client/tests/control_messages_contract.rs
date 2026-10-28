//! The 3.0 control message types `MoonBridge.java` names, from
//! `app/src/test/resources/protocol/control-messages.txt`, checked against `rubylight_protocol`.
//! The JUnit `MoonBridgeControlMessagesTest` checks the same file against the Java constants, so
//! the two cannot drift apart unnoticed.

use rubylight_protocol::ids::{CONTROL_IDS, RUBYLIGHT_3_CONTROL_IDS};
use rubylight_protocol::{
    cursor_shape, fec_report, keyboard_layout, pipeline_rates, rate_change, text_input, usb,
};
use std::collections::BTreeMap;

const FIXTURE: &str = include_str!("../../../app/src/test/resources/protocol/control-messages.txt");

/// Field name to type, as the file lists them.
fn named() -> BTreeMap<&'static str, u16> {
    let mut named = BTreeMap::new();
    for line in FIXTURE
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
    {
        let (name, id) = line.split_once(' ').unwrap_or_else(|| panic!("{line:?}"));
        let hex = id.strip_prefix("0x").unwrap_or_else(|| panic!("{line:?}"));
        let id = u16::from_str_radix(hex, 16).unwrap_or_else(|e| panic!("{line:?}: {e}"));
        assert!(named.insert(name, id).is_none(), "{name} is listed twice");
    }
    named
}

#[test]
fn moonbridge_names_the_3_0_control_messages_as_rubylight_protocol_defines_them() {
    let named = named();
    assert_eq!(
        named,
        BTreeMap::from([
            (
                "CONTROL_MESSAGE_FEC_REPORT",
                fec_report::FEC_REPORT_MESSAGE_TYPE
            ),
            ("CONTROL_MESSAGE_USB", usb::USB_MESSAGE_TYPE),
            (
                "CONTROL_MESSAGE_TEXT_INPUT",
                text_input::TEXT_INPUT_MESSAGE_TYPE
            ),
            (
                "CONTROL_MESSAGE_RATE_CHANGE",
                rate_change::RATE_CHANGE_MESSAGE_TYPE
            ),
            (
                "CONTROL_MESSAGE_PIPELINE_RATES",
                pipeline_rates::PIPELINE_RATES_MESSAGE_TYPE
            ),
            (
                "CONTROL_MESSAGE_KEYBOARD_LAYOUT",
                keyboard_layout::KEYBOARD_LAYOUT_MESSAGE_TYPE
            ),
            (
                "CONTROL_MESSAGE_CURSOR_SHAPE_REQUEST",
                cursor_shape::CURSOR_SHAPE_REQUEST_MESSAGE_TYPE
            ),
        ])
    );
    // Every 3.0 type has a Java name but the host's cursor image, which MoonBridge does not name.
    let mut listed: Vec<u16> = named.into_values().collect();
    listed.sort_unstable();
    let mut defined: Vec<u16> = CONTROL_IDS
        .iter()
        .map(|(id, _)| *id)
        .filter(|id| {
            RUBYLIGHT_3_CONTROL_IDS.contains(id) && *id != cursor_shape::CURSOR_SHAPE_MESSAGE_TYPE
        })
        .collect();
    defined.sort_unstable();
    assert_eq!(listed, defined);
}
