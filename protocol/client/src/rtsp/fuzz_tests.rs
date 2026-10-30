//! Property tests over RTSP responses and DESCRIBE answers. See `crate::fuzz_tests`.

use proptest::collection::vec;
use proptest::prelude::*;
use proptest::sample::{select, Index};

use super::describe::{self, DescribeInfo};
use super::message::{parse_response, RtspResponse};
use crate::control::negotiation;
use crate::fuzz_tests::{mutated, text};

fn response() -> impl Strategy<Value = RtspResponse> {
    (
        0i32..1000,
        "[ -~]{0,16}",
        vec(("[!-9;-~]{1,12}", "[ -~]{0,24}"), 0..6),
        prop_oneof![Just(None), vec(any::<u8>(), 1..64).prop_map(Some)],
    )
        .prop_map(
            |(status_code, status_text, options, payload)| RtspResponse {
                status_code,
                status_text,
                options,
                payload,
            },
        )
}

fn response_bytes(r: &RtspResponse) -> Vec<u8> {
    let mut out = format!("RTSP/1.0 {} {}\r\n", r.status_code, r.status_text);
    for (name, value) in &r.options {
        out.push_str(&format!("{name}: {value}\r\n"));
    }
    out.push_str("\r\n");
    let mut out = out.into_bytes();
    out.extend(r.payload.iter().flatten());
    out
}

const DESCRIBE: &str = "v=0\r\no=android 0 14 IN IPv4 0.0.0.0\r\ns=NVIDIA Streaming Client\r\n\
    a=x-ss-general.featureFlags:3\r\na=x-ss-general.encryptionSupported:15\r\n\
    a=x-ss-general.encryptionRequested:9\r\n\
    a=x-rl-control:0x5530,0x5531,0x5532,0x5533,0x5534,0x5535,0x5536,0x5537\r\n\
    a=x-rl-roaming:1\r\na=x-rl-mic:48014\r\n\
    sprop-parameter-sets=AAAAAU\r\na=rtpmap:98 AV1/90000\r\n\
    a=rtpmap:99 PYROWAVE/90000\r\na=x-ss-pyrowave.bitstream:186f0393\r\n\
    a=fmtp:97 surround-params=21101\r\na=fmtp:97 surround-params=642014523\r\n\
    a=fmtp:97 surround-params=85301456723\r\na=fmtp:97 surround-params=21101\r\n\
    a=fmtp:97 surround-params=660014523\r\na=fmtp:97 surround-params=88001456723\r\n\
    a=x-nv-video[0].refPicInvalidation:1\r\n\
    m=audio 48012 RTP/AVP 96\r\na=rtpmap:96 opus/48000/1\r\na=fmtp:96 minptime=10;useinbandfec=1\r\n";

/// The DESCRIBE answer with a few of its lines dropped, repeated or given other values.
fn describe_answer() -> impl Strategy<Value = String> {
    let lines: Vec<&str> = DESCRIBE.split("\r\n").filter(|l| !l.is_empty()).collect();
    (
        vec(any::<Index>(), 0..4),
        vec((any::<Index>(), "[ -~]{0,24}"), 0..4),
        vec(any::<Index>(), 0..3),
    )
        .prop_map(move |(dropped, values, repeated)| {
            let mut lines: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
            for (at, value) in values {
                let at = at.index(lines.len());
                let line = &mut lines[at];
                let keep = line.find([':', '=']).map_or(0, |i| i + 1);
                line.truncate(keep);
                line.push_str(&value);
            }
            for at in repeated {
                let at = at.index(lines.len());
                let line = lines[at].clone();
                lines.push(line);
            }
            for at in dropped {
                if !lines.is_empty() {
                    let at = at.index(lines.len());
                    lines.remove(at);
                }
            }
            lines.join("\r\n") + "\r\n"
        })
}

proptest! {
    #[test]
    fn rtsp_response_round_trips(r in response()) {
        prop_assert_eq!(parse_response(&response_bytes(&r)).ok(), Some(r));
    }

    #[test]
    fn rtsp_response_reads_anything(raw in mutated(response_bytes(&RtspResponse {
        status_code: 200,
        status_text: "OK".into(),
        options: vec![("CSeq".into(), "3".into()), ("Content-Length".into(), "12".into())],
        payload: Some(DESCRIBE.as_bytes()[..12].to_vec()),
    }))) {
        if let Ok(r) = parse_response(&raw) {
            let _ = (r.sequence(), r.payload_text());
        }
    }

    #[test]
    fn describe_reads_any_answer(
        sdp in prop_oneof![describe_answer(), text(DESCRIBE)],
        channels in prop_oneof![select(vec![2u8, 6, 8, 12]), any::<u8>()],
    ) {
        let info = DescribeInfo::parse(&sdp);
        prop_assert!(info.control_messages.len() <= negotiation::MAX_CONTROL_MESSAGES);
        prop_assert!(describe::parse_control_messages(&sdp).len() <= negotiation::MAX_CONTROL_MESSAGES);
        prop_assert!(negotiation::parse_control_messages(&sdp).len() <= negotiation::MAX_CONTROL_MESSAGES);
        let _ = describe::sdp_attribute_u32(&sdp, "x-ss-general.featureFlags");
        let _ = super::parse_app_version(&sdp);
        if let Ok(configs) = describe::parse_opus_configs(&sdp, channels) {
            prop_assert!(usize::from(channels) <= describe::MAX_CHANNELS);
            for config in [Some(configs.normal), configs.high_quality].into_iter().flatten() {
                prop_assert_eq!(config.channel_count, channels);
            }
        }
    }

    /// Control ids read back as listed, whatever their form.
    #[test]
    fn describe_control_ids_round_trip(ids in vec(any::<u16>(), 0..40), hex in vec(any::<bool>(), 40)) {
        let listed: Vec<String> = ids
            .iter()
            .zip(&hex)
            .map(|(id, &hex)| if hex { format!("0x{id:04x}") } else { id.to_string() })
            .collect();
        let sdp = format!("a=x-rl-control:{}\r\n", listed.join(","));
        let mut expected = Vec::new();
        for id in ids {
            if !expected.contains(&id) && expected.len() < negotiation::MAX_CONTROL_MESSAGES {
                expected.push(id);
            }
        }
        prop_assert_eq!(&describe::parse_control_messages(&sdp), &expected);
        prop_assert_eq!(&negotiation::parse_control_messages(&sdp), &expected);
    }
}
