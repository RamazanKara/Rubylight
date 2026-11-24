//! Property tests over every decoder of peer input in this crate, the host's and the
//! client's: arbitrary bytes never panic, and a message that decodes encodes back to the same
//! value. Inputs are valid encodings of random values, edited, cut short or extended, and plain
//! random bytes. `PROPTEST_CASES` sets the cases per property (256 by default, a few
//! milliseconds each); a local fuzz run raises it to about a minute per target.

use crate::{
    audio_ext, control, controller_identity, cursor_shape, fec_report, frame_warp, keyboard_layout, pairing_uri,
    pen, phase_lock, pipeline_rates, rate_change, roaming, steam_controller, text_input, transport_feedback, usb, usbip,
};
use alloc::{string::String, vec, vec::Vec};
use proptest::collection::vec;
use proptest::prelude::*;
use proptest::sample::{select, Index};

/// Bytes biased towards what the decoders check: small versions, kinds and lengths, printable
/// ASCII for busids, names and tags, and 0xFF at the limits.
fn byte() -> impl Strategy<Value = u8> {
    prop_oneof![0u8..8, 0x20u8..0x7F, Just(0xFF), any::<u8>()]
}

/// A message from `seeds`, valid encodings: as it is, with a few bytes changed, cut short or
/// with bytes after it; or random bytes of about its size.
fn message(seeds: impl Strategy<Value = Vec<u8>>) -> impl Strategy<Value = Vec<u8>> {
    let edits = vec((any::<Index>(), byte()), 0..4);
    let cut = prop_oneof![3 => Just(None), 1 => any::<Index>().prop_map(Some)];
    let tail = prop_oneof![3 => Just(Vec::new()), 1 => vec(byte(), 1..16)];
    let edited = (seeds, edits, cut, tail).prop_map(|(mut m, edits, cut, tail)| {
        for (at, b) in edits {
            if !m.is_empty() {
                let at = at.index(m.len());
                m[at] = b;
            }
        }
        if let Some(cut) = cut {
            m.truncate(cut.index(m.len() + 1));
        }
        m.extend(tail);
        m
    });
    prop_oneof![4 => edited, 1 => vec(any::<u8>(), 0..96)]
}

/// `decode` never panics, and what it accepts survives `encode` and `decode` unchanged.
fn check<T: core::fmt::Debug + PartialEq>(
    bytes: &[u8],
    decode: impl Fn(&[u8]) -> Option<T>,
    encode: impl Fn(&T) -> Option<Vec<u8>>,
) -> Result<(), TestCaseError> {
    if let Some(value) = decode(bytes) {
        let wire = encode(&value);
        prop_assert!(wire.is_some(), "{value:?} decoded but does not encode");
        prop_assert_eq!(decode(&wire.unwrap()), Some(value));
    }
    Ok(())
}

macro_rules! round_trip {
    ($name:ident, $seeds:expr, |$b:ident| $decode:expr, |$v:ident| $encode:expr) => {
        proptest! {
            #[test]
            fn $name(bytes in message($seeds)) {
                check(&bytes, |$b| $decode, |$v| $encode)?;
            }
        }
    };
}

/// A fixed-size view of `b`, for the decoders that take arrays.
fn array<const N: usize>(b: &[u8]) -> Option<[u8; N]> {
    b.try_into().ok()
}

/// Random bytes of a format's size, for formats without a header to get right.
fn raw(len: usize) -> impl Strategy<Value = Vec<u8>> {
    vec(byte(), len)
}


mod extended;

round_trip!(
    clock_sync_request,
    any::<(u32, u64)>().prop_map(|(sequence, client_us)| {
        control::ClockSyncRequest {
            sequence,
            client_us,
        }
        .encode()
        .to_vec()
    }),
    |b| control::ClockSyncRequest::decode(b),
    |v| Some(v.encode().to_vec())
);
round_trip!(
    clock_sync_reply,
    any::<(u32, u64, u64, u64)>().prop_map(
        |(sequence, client_us, host_receive_us, host_send_us)| {
            control::ClockSyncReply {
                sequence,
                client_us,
                host_receive_us,
                host_send_us,
            }
            .encode()
            .to_vec()
        }
    ),
    |b| control::ClockSyncReply::decode(b),
    |v| Some(v.encode().to_vec())
);
round_trip!(
    display_caps,
    (
        any::<bool>(),
        0u32..1_100_000,
        0u32..1_100_000,
        0u32..11_000
    )
        .prop_map(|(hdr, max_centinits, average, min_decimillinits)| {
            let max_average_centinits = average.min(max_centinits);
            control::DisplayCaps {
                hdr,
                max_centinits,
                max_average_centinits,
                min_decimillinits,
            }
            .encode()
            .to_vec()
        }),
    |b| control::DisplayCaps::decode(b),
    |v| Some(v.encode().to_vec())
);
round_trip!(
    reconfigure,
    (0u16..8192, 0u16..8192, 0u32..600_000).prop_map(|(width, height, fps_millihz)| {
        control::Reconfigure {
            width,
            height,
            fps_millihz,
        }
        .encode()
        .to_vec()
    }),
    |b| control::Reconfigure::decode(b),
    |v| Some(v.encode().to_vec())
);
round_trip!(
    host_timing_request,
    any::<bool>().prop_map(|enabled| control::HostTimingRequest { enabled }.encode().to_vec()),
    |b| control::HostTimingRequest::decode(b),
    |v| Some(v.encode().to_vec())
);
round_trip!(
    link_notice_request,
    any::<bool>().prop_map(|enabled| control::LinkNoticeRequest { enabled }.encode().to_vec()),
    |b| control::LinkNoticeRequest::decode(b),
    |v| Some(v.encode().to_vec())
);
round_trip!(
    link_notice,
    (
        select(vec![
            control::LinkState::Full,
            control::LinkState::Limited,
            control::LinkState::Floor
        ]),
        select(vec![
            control::LinkReason::None,
            control::LinkReason::Network
        ]),
        any::<(u32, u32)>(),
    )
        .prop_map(|(state, reason, (ceiling_kbps, current_kbps))| {
            control::LinkNotice {
                state,
                reason,
                ceiling_kbps,
                current_kbps,
            }
            .encode()
            .to_vec()
        }),
    |b| control::LinkNotice::decode(b),
    |v| Some(v.encode().to_vec())
);

/// A record whose stages fit its total, and an extra that fits its queue.
fn host_timing() -> impl Strategy<Value = (control::HostTiming, control::HostTimingExtra)> {
    (
        any::<u32>(),
        0u32..20_000,
        0u32..20_000,
        0u32..20_000,
        0u32..1_000,
        any::<u64>(),
        0u32..20_000,
    )
        .prop_map(
            |(frame, queue_us, encode_us, send_us, slack, presented_at_us, capture_us)| {
                let record = control::HostTiming {
                    frame,
                    host_us: queue_us + encode_us + send_us + slack,
                    queue_us,
                    encode_us,
                    send_us,
                };
                let capture_us = capture_us.min(queue_us);
                let extra = control::HostTimingExtra {
                    presented_at_us,
                    capture_us,
                    convert_us: (queue_us - capture_us) / 2,
                };
                (record, extra)
            },
        )
}

round_trip!(
    host_timing_report,
    vec(host_timing(), 0..6).prop_map(|records| {
        let records: Vec<_> = records.into_iter().map(|(r, _)| r).collect();
        control::HostTiming::encode_report(&records)
    }),
    |b| control::HostTiming::decode_report(b),
    |v| Some(control::HostTiming::encode_report(v))
);
round_trip!(
    cursor_shape_request,
    (any::<bool>(), 0u16..200).prop_map(|(enabled, max_edge)| {
        cursor_shape::CursorShapeRequest { enabled, max_edge }
            .encode()
            .to_vec()
    }),
    |b| cursor_shape::CursorShapeRequest::decode(b),
    |v| Some(v.encode().to_vec())
);
round_trip!(
    cursor_shape,
    prop_oneof![
        Just(cursor_shape::CursorShape::Hide),
        Just(cursor_shape::CursorShape::Default),
        any::<u32>().prop_map(|id| cursor_shape::CursorShape::Show { id }),
        (any::<u32>(), 1u16..6, 1u16..6, 0u16..6, 0u16..6, 90u16..410).prop_flat_map(
            |(id, width, height, hotspot_x, hotspot_y, scale)| {
                vec(any::<u8>(), usize::from(width) * usize::from(height) * 4).prop_map(
                    move |pixels| {
                        let image = cursor_shape::CursorImage {
                            width,
                            height,
                            hotspot_x,
                            hotspot_y,
                            scale,
                            pixels,
                        };
                        cursor_shape::CursorShape::Image { id, image }
                    },
                )
            }
        ),
    ]
    .prop_map(|shape| shape.encode().unwrap_or_default()),
    |b| cursor_shape::CursorShape::decode(b),
    |v| v.encode()
);
round_trip!(
    frame_warp_request,
    (any::<bool>(), any::<u8>(), 0u16..2100, any::<bool>()).prop_map(|(enabled, controller, mask_max_width, lens)| {
        frame_warp::FrameWarpRequest {
            enabled,
            controller: (controller < 20).then_some(controller),
            mask_max_width,
            lens,
        }
        .encode()
        .to_vec()
    }),
    |b| frame_warp::FrameWarpRequest::decode(b),
    |v| Some(v.encode().to_vec())
);
round_trip!(
    frame_warp_tags,
    (any::<u16>(), vec(any::<(u32, i32, i32, u32, u32)>(), 1..5)).prop_map(|(generation, tags)| {
        frame_warp::Tags {
            model_generation: generation,
            tags: tags
                .into_iter()
                .map(|(frame, mouse_x, mouse_y, controller_seq, controller_held_us)| {
                    frame_warp::Tag {
                        frame,
                        mouse_x,
                        mouse_y,
                        controller_seq,
                        controller_held_us,
                    }
                })
                .collect(),
        }
        .encode()
        .unwrap_or_default()
    }),
    |b| frame_warp::Tags::decode(b),
    |v| v.encode()
);
round_trip!(
    frame_warp_model,
    (
        any::<(u16, bool, bool, u16, u16)>(),
        any::<[f32; 4]>(),
        0.0f32..0.5,
        0usize..5,
        any::<([f32; 2], u32)>(),
        (0.0f32..=1.0, prop_oneof![Just(0.0f32), 1.0f32..=1_000_000.0]),
    )
        .prop_map(
            |((generation, mouse_locked, stick_locked, w, h), gains, deadzone, exponent, (max_offset, lag_us), (confidence, focal))| {
                frame_warp::Model {
                    generation,
                    mouse_locked,
                    stick_locked,
                    picture_width: w,
                    picture_height: h,
                    mouse_gain: [gains[0], gains[1]],
                    stick_gain: [gains[2], gains[3]],
                    stick_deadzone: deadzone,
                    stick_exponent: frame_warp::STICK_EXPONENTS[exponent],
                    max_offset,
                    lag_us,
                    confidence,
                    focal,
                }
                .encode()
                .map(|m| m.to_vec())
                .unwrap_or_default()
            }
        ),
    |b| frame_warp::Model::decode(b),
    |v| v.encode().map(|m| m.to_vec())
);
round_trip!(
    frame_warp_mask,
    (any::<u16>(), 0u16..40, 0u16..6, vec(any::<u32>(), 0..12)).prop_map(
        |(generation, width, height, words)| {
            let mut mask = frame_warp::Mask::new(generation, width, height);
            if width == 0 || height == 0 {
                mask = frame_warp::Mask { generation, ..Default::default() };
            }
            for (slot, word) in mask.words.iter_mut().zip(words) {
                *slot = word;
            }
            // Bits past a row's width are not on the wire.
            let stride = frame_warp::Mask::stride(width);
            if width % 32 != 0 {
                for row in mask.words.chunks_mut(stride.max(1)) {
                    if let Some(last) = row.last_mut() {
                        *last &= (1u32 << (width % 32)) - 1;
                    }
                }
            }
            mask.encode().unwrap_or_default()
        }
    ),
    |b| frame_warp::Mask::decode(b),
    |v| v.encode()
);
round_trip!(
    fec_report,
    (
        any::<u32>(),
        0u32..64,
        0u32..64,
        0u32..64,
        0u16..64,
        0u16..6000
    )
        .prop_map(
            |(seq, data_shards, parity_shards, missing_shards, unrecovered_blocks, interval_ms)| {
                fec_report::FecReport {
                    seq,
                    data_shards,
                    parity_shards,
                    missing_shards,
                    unrecovered_blocks,
                    interval_ms,
                }
                .encode()
                .to_vec()
            }
        ),
    |b| fec_report::FecReport::decode(b),
    |v| Some(v.encode().to_vec())
);
round_trip!(
    transport_feedback,
    (
        any::<u8>(),
        any::<u16>(),
        0u16..70,
        any::<u32>(),
        any::<u32>(),
        any::<u16>(),
        vec(
            prop_oneof![any::<u16>(), Just(transport_feedback::NOT_RECEIVED)],
            0..20
        )
    )
        .prop_map(
            |(flags, base, stride, seq, base_arrival_us, late, entries)| {
                transport_feedback::TransportFeedback {
                    flags,
                    base,
                    stride,
                    seq,
                    base_arrival_us,
                    late,
                    entries,
                }
                .encode()
                .unwrap_or_default()
            }
        ),
    |b| transport_feedback::TransportFeedback::decode(b),
    |v| v.encode()
);
round_trip!(
    keyboard_layout,
    (
        select(vec![
            keyboard_layout::LayoutSource::Unknown,
            keyboard_layout::LayoutSource::Klid,
            keyboard_layout::LayoutSource::Bcp47,
            keyboard_layout::LayoutSource::Xkb,
        ]),
        any::<u32>(),
        "[a-zA-Z0-9_+-]{0,16}|\\PC{0,8}",
    )
        .prop_map(|(source, klid, tag)| {
            keyboard_layout::KeyboardLayout { source, klid, tag }
                .encode()
                .unwrap_or_default()
        }),
    |b| keyboard_layout::KeyboardLayout::decode(b),
    |v| v.encode()
);
round_trip!(
    pen_barrel_roll,
    (
        raw(pen::PEN_BYTES - 8),
        prop_oneof![0u16..400, any::<u16>()]
    )
        .prop_map(|(axes, roll)| {
            let declared = (pen::PEN_BYTES as u32 - 4).to_be_bytes();
            let pen = [&declared[..], &pen::PEN_MAGIC.to_le_bytes(), &axes].concat();
            pen::PenBarrelRoll::new(&pen, roll)
                .map(|p| p.encode().to_vec())
                .unwrap_or_default()
        }),
    |b| pen::PenBarrelRoll::decode(b),
    |v| Some(v.encode().to_vec())
);
round_trip!(
    phase_lock_report,
    (
        0u16..512,
        1_900_000u32..42_000_000,
        -42_000_000i32..42_000_000,
        0u32..42_000_000
    )
        .prop_map(|(frames, period_ns, lead_ns, spread_ns)| {
            let lead_ns = lead_ns % period_ns as i32;
            let spread_ns = spread_ns % period_ns;
            phase_lock::Report {
                frames,
                period_ns,
                lead_ns,
                spread_ns,
            }
            .encode()
            .to_vec()
        }),
    |b| phase_lock::Report::decode(b),
    |v| Some(v.encode().to_vec())
);
round_trip!(
    pipeline_rates_request,
    (any::<bool>(), 0u16..12_000).prop_map(|(enabled, interval_ms)| {
        pipeline_rates::PipelineRatesRequest {
            enabled,
            interval_ms,
        }
        .encode()
        .to_vec()
    }),
    |b| pipeline_rates::PipelineRatesRequest::decode(b),
    |v| Some(v.encode().to_vec())
);
round_trip!(
    pipeline_rates_report,
    raw(pipeline_rates::PIPELINE_RATES_REPORT_BYTES),
    |b| pipeline_rates::PipelineRatesReport::decode(b),
    |v| Some(v.encode().to_vec())
);
round_trip!(
    rate_change_request,
    raw(rate_change::RATE_CHANGE_REQUEST_BYTES),
    |b| rate_change::RateChangeRequest::decode(b),
    |v| Some(v.encode().to_vec())
);
