//! Property tests for rate changes, roaming, Steam controllers, text input, USB/USBIP
//! and pairing URIs.

use super::*;

fn outcome() -> impl Strategy<Value = rate_change::Outcome> {
    use rate_change::{Reason, Status};
    (
        select(vec![
            Status::Unchanged,
            Status::Applied,
            Status::Clamped,
            Status::Refused,
        ]),
        select(vec![
            Reason::None,
            Reason::Invalid,
            Reason::Disabled,
            Reason::Codec,
            Reason::HostLimit,
            Reason::EncoderLimit,
            Reason::EncoderFailed,
            Reason::Superseded,
            Reason::Other,
        ]),
    )
        .prop_map(|(status, reason)| rate_change::Outcome { status, reason })
}

round_trip!(
    rate_change_result,
    (any::<(u32, u32, u32)>(), outcome(), outcome()).prop_map(
        |((serial, bitrate_kbps, fps_millihz), bitrate, fps)| {
            rate_change::RateChangeResult {
                serial,
                bitrate_kbps,
                fps_millihz,
                bitrate,
                fps,
            }
            .encode()
            .to_vec()
        }
    ),
    |b| rate_change::RateChangeResult::decode(b),
    |v| Some(v.encode().to_vec())
);
round_trip!(
    roaming_datagram,
    (
        1u8..=4,
        any::<[u8; roaming::SESSION_TAG_BYTES]>(),
        any::<u32>(),
        any::<[u8; 16]>(),
        any::<[u8; 16]>()
    )
        .prop_map(|(kind, session_tag, seq, gcm_tag, ciphertext)| {
            let header = roaming::Header {
                kind: roaming::Kind::from_u8(kind).unwrap(),
                session_tag,
                seq,
            };
            roaming::Datagram {
                header,
                gcm_tag,
                ciphertext,
            }
            .encode()
            .to_vec()
        }),
    |b| roaming::Datagram::decode(b).ok(),
    |v| Some(v.encode().to_vec())
);
round_trip!(
    steam_haptic,
    (
        any::<u16>(),
        vec(any::<u8>(), 0..=steam_controller::STEAM_HAPTIC_REPORT_MAX)
    )
        .prop_map(|(controller, report)| {
            steam_controller::SteamHaptic::new(controller, &report)
                .map(|h| h.encode().to_vec())
                .unwrap_or_default()
        }),
    |b| steam_controller::SteamHaptic::decode(b).ok(),
    |v| Some(v.encode().to_vec())
);
round_trip!(
    steam_settings,
    (
        any::<u16>(),
        prop_oneof![
            Just(steam_controller::STEAM_SETTINGS_SET_VALUES),
            Just(steam_controller::STEAM_SETTINGS_LOAD_DEFAULTS),
            any::<u8>(),
        ],
        prop_oneof![
            Just(Vec::new()),
            vec(
                any::<[u8; 3]>(),
                0..=steam_controller::STEAM_SETTINGS_TRIPLES_MAX
            )
        ],
    )
        .prop_map(|(controller, command, triples)| {
            steam_controller::SteamSettings::new(controller, command, &triples)
                .map(|s| s.encode().to_vec())
                .unwrap_or_default()
        }),
    |b| steam_controller::SteamSettings::decode(b).ok(),
    |v| Some(v.encode().to_vec())
);
round_trip!(
    steam_attributes,
    (
        any::<(u8, u8, u8)>(),
        any::<[u32; 6]>(),
        any::<[[u8; 16]; 3]>()
    )
        .prop_map(
            |(
                (controller, transport, valid_mask),
                words,
                [build_id, unit_serial, board_serial],
            )| {
                steam_controller::SteamAttributes {
                    controller,
                    transport,
                    valid_mask,
                    firmware_build_time: words[0],
                    bootloader_build_time: words[1],
                    board_revision: words[2],
                    capabilities: words[3],
                    radio_firmware_build_time: words[4],
                    dongle_firmware_build_time: words[5],
                    build_id,
                    unit_serial,
                    board_serial,
                }
                .encode()
                .to_vec()
            }
        ),
    |b| steam_controller::SteamAttributes::decode(b).ok(),
    |v| Some(v.encode().to_vec())
);
round_trip!(
    controller_identity,
    (0u8..16, any::<u16>(), any::<u16>()).prop_map(|(controller, vendor, product)| {
        controller_identity::ControllerIdentity {
            controller,
            vendor,
            product,
        }
        .encode()
        .to_vec()
    }),
    |b| controller_identity::ControllerIdentity::decode(b).ok(),
    |v| Some(v.encode().to_vec())
);
round_trip!(
    text_input_request,
    (any::<[bool; 3]>(), 0u16..5000).prop_map(|(flags, max_text)| {
        text_input::TextInputRequest {
            notices: flags[0],
            surrounding_text: flags[1],
            element_ids: flags[2],
            max_text,
        }
        .encode()
        .to_vec()
    }),
    |b| text_input::TextInputRequest::decode(b),
    |v| Some(v.encode().to_vec())
);
round_trip!(
    text_input_notice,
    (
        select(vec![
            text_input::NoticeKind::Lower,
            text_input::NoticeKind::Raise,
            text_input::NoticeKind::Update
        ]),
        select(vec![
            text_input::InputType::Text,
            text_input::InputType::Number,
            text_input::InputType::Decimal,
            text_input::InputType::Phone,
            text_input::InputType::Email,
            text_input::InputType::Url,
            text_input::InputType::Password,
            text_input::InputType::Search,
            text_input::InputType::Multiline,
        ]),
        any::<[bool; 4]>(),
        // The element id: 0 keeps the notice at version 1.
        (any::<u32>(), prop_oneof![Just(0u32), any::<u32>()]),
        (0u16..40, 0u16..40),
        "\\PC{0,24}",
    )
        .prop_map(
            |(
                kind,
                input_type,
                flags,
                (serial, element),
                (selection_start, selection_end),
                text,
            )| {
                text_input::TextInputNotice {
                    kind,
                    input_type,
                    sensitive: flags[0],
                    no_autocorrect: flags[1],
                    no_autocapitalize: flags[2],
                    enter_action: flags[3],
                    serial,
                    selection_start,
                    selection_end,
                    text,
                    element,
                }
                .encode()
                .unwrap_or_default()
            }
        ),
    |b| text_input::TextInputNotice::decode(b),
    |v| v.encode()
);
round_trip!(
    audio_chunk_header,
    raw(audio_ext::CHUNK_HEADER_BYTES),
    |b| audio_ext::ChunkHeader::decode(b),
    |v| Some(v.encode().to_vec())
);
round_trip!(
    usb_message,
    prop_oneof![
        (
            any::<u32>(),
            any::<(u16, u16, u8, u8)>(),
            "[!-~]{1,31}",
            "\\PC{0,20}"
        )
            .prop_map(|(request_id, (vid, pid, class, speed), busid, name)| {
                usb::UsbMessage::AttachRequest {
                    request_id,
                    vid,
                    pid,
                    class,
                    speed,
                    busid,
                    name,
                }
            }),
        (any::<u32>(), any::<[u8; usb::TOKEN_BYTES]>())
            .prop_map(|(request_id, token)| usb::UsbMessage::TunnelReady { request_id, token }),
        any::<u32>().prop_map(|request_id| usb::UsbMessage::Detach { request_id }),
        (
            any::<u32>(),
            select(vec![
                usb::UsbState::Attached,
                usb::UsbState::Detached,
                usb::UsbState::Refused,
                usb::UsbState::Failed,
            ]),
            select(vec![
                usb::UsbReason::None,
                usb::UsbReason::Permission,
                usb::UsbReason::Disabled,
                usb::UsbReason::Limit,
                usb::UsbReason::Driver,
                usb::UsbReason::TunnelTimeout,
                usb::UsbReason::DeviceError,
                usb::UsbReason::SessionEnd,
            ]),
        )
            .prop_map(|(request_id, state, reason)| usb::UsbMessage::Status {
                request_id,
                state,
                reason
            }),
    ]
    .prop_map(|message| message.encode().unwrap_or_default()),
    |b| usb::UsbMessage::decode(b),
    |v| v.encode()
);
round_trip!(
    usbip_op_header,
    raw(usbip::OP_HEADER_BYTES),
    |b| array(b).map(|a| usbip::OpHeader::decode(&a)),
    |v| Some(v.encode().to_vec())
);
round_trip!(
    usbip_import_request,
    "[0-9a-z.:_-]{1,31}".prop_map(|busid| usbip::import_request(&busid)
        .map(|a| a.to_vec())
        .unwrap_or_default()),
    |b| array(b).and_then(|a| usbip::decode_import_request(&a)),
    |v| usbip::import_request(v).map(|a| a.to_vec())
);
round_trip!(
    usbip_device,
    (
        "[ -~]{0,40}",
        "[0-9a-z.:_-]{1,31}",
        any::<[u32; 3]>(),
        any::<[u16; 3]>(),
        any::<[u8; 6]>()
    )
        .prop_map(
            |(path, busid, [busnum, devnum, speed], [vendor, product, bcd_device], b)| {
                usbip::Device {
                    path,
                    busid,
                    busnum,
                    devnum,
                    speed,
                    vendor,
                    product,
                    bcd_device,
                    class: b[0],
                    subclass: b[1],
                    protocol: b[2],
                    configuration: b[3],
                    configurations: b[4],
                    interfaces: b[5],
                }
                .encode()
                .map(|a| a.to_vec())
                .unwrap_or_default()
            }
        ),
    |b| array(b).and_then(|a| usbip::Device::decode(&a)),
    |v| v.encode().map(|a| a.to_vec())
);
round_trip!(
    usbip_header,
    (1u32..=4, raw(44)).prop_map(|(command, rest)| [&command.to_be_bytes()[..], &rest].concat()),
    |b| array(b).and_then(|a| usbip::Header::decode(&a)),
    |v| Some(v.encode().to_vec())
);
round_trip!(
    usbip_iso_packet,
    raw(16),
    |b| array(b).map(|a| usbip::IsoPacket::decode(&a)),
    |v| Some(v.encode().to_vec())
);

proptest! {
    /// A report with its extension: the base records always round-trip, and the extras do
    /// when every record kept one.
    #[test]
    fn host_timing_extended_report(bytes in message(vec(host_timing(), 0..6).prop_map(|records| {
        control::HostTiming::encode_report_with(&records)
    }))) {
        if let Some(records) = control::HostTiming::decode_report_with(&bytes) {
            let base: Vec<_> = records.iter().map(|(r, _)| *r).collect();
            prop_assert_eq!(control::HostTiming::decode_report(&bytes), Some(base));
            let extras: Option<Vec<_>> = records.iter().map(|(r, x)| x.map(|x| (*r, x))).collect();
            if let Some(extras) = extras {
                let wire = control::HostTiming::encode_report_with(&extras);
                prop_assert_eq!(control::HostTiming::decode_report_with(&wire), Some(records));
            }
        }
    }

    /// The sealed payload of each roaming kind.
    #[test]
    fn roaming_payload(kind in 1u8..=4, plaintext in prop_oneof![
        any::<[u8; roaming::PAYLOAD_BYTES]>(),
        (any::<[u8; 12]>()).prop_map(|head| {
            let mut plaintext = [0; roaming::PAYLOAD_BYTES];
            plaintext[..12].copy_from_slice(&head);
            plaintext
        }),
    ]) {
        let kind = roaming::Kind::from_u8(kind).unwrap();
        if let Some(payload) = roaming::Payload::decode(kind, &plaintext) {
            prop_assert_eq!(payload.kind(), kind);
            prop_assert_eq!(roaming::Payload::decode(kind, &payload.encode()), Some(payload));
        }
    }

    /// The transfer sizes a submit and its reply announce never overflow, and the descriptors
    /// they promise are read only from inside the payload.
    #[test]
    fn usbip_transfer_sizes(
        direction in 0u32..3,
        asked in any::<u32>(),
        asked_packets in prop_oneof![0u32..64, Just(u32::MAX), any::<u32>()],
        length in any::<u32>(),
        packets in prop_oneof![0u32..64, Just(u32::MAX), any::<u32>()],
        payload in vec(any::<u8>(), 0..1024),
    ) {
        let _ = usbip::submit_payload(direction, length, packets);
        let _ = usbip::reply_payload(direction, asked, asked_packets, length, packets);
        if let Some(descriptors) = usbip::iso_descriptors(&payload, packets) {
            let replied: Vec<_> = descriptors.collect();
            prop_assert!(replied.len() * 16 <= payload.len());
            let _ = usbip::iso_reply_fits(&replied, replied.iter().copied(), length);
        }
    }

    /// Text values from SDP and launch parameters.
    #[test]
    fn text_values(text in "[0-9a-z,]{0,48}|\\PC{0,48}") {
        let _ = audio_ext::Layout::parse(&text);
        let _ = fec_report::FecChoice::parse(&text);
        let _ = pairing_uri::PairingUri::parse(&text);
        let _ = pairing_uri::PairingUri::parse(&[pairing_uri::PREFIX, &text].concat());
    }

    /// A pairing link reads back as the invitation it was written from.
    #[test]
    fn pairing_uri_round_trip(
        host in "[a-zA-Z0-9._-]{1,24}|\\[[0-9a-f:.]{0,16}:%[a-z0-9]{1,6}\\]",
        port in 1u16..,
        otp in "\\PC{1,8}",
        passphrase in "\\PC{0,16}",
        unique_id in "\\PC{0,16}",
        cert_sha256 in any::<[u8; 32]>(),
        name in "\\PC{0,24}",
    ) {
        let uri = pairing_uri::PairingUri { host, port, otp, passphrase, unique_id, cert_sha256, name };
        prop_assert_eq!(pairing_uri::PairingUri::parse(&uri.to_uri()), Some(uri));
    }

    /// Whatever a link carries, parsing it again after writing it out changes nothing.
    #[test]
    fn pairing_uri_reparse(query in vec(prop_oneof![
        Just(String::from("v=1")), Just(String::from("host=h")), Just(String::from("port=47989")),
        Just(String::from("otp=1234")), Just(["fp=", &"0f".repeat(32)].concat()), "[a-z]{1,4}=[%0-9a-fA-F\\PC]{0,12}",
    ], 0..10)) {
        let text = [pairing_uri::PREFIX, &query.join("&")].concat();
        if let Some(uri) = pairing_uri::PairingUri::parse(&text) {
            prop_assert_eq!(pairing_uri::PairingUri::parse(&uri.to_uri()), Some(uri));
        }
    }
}
