//! The HDR metadata the host writes: the static mastering metadata of HDR mode messages,
//! read back as Moonlight reads it, and Dolby Vision RPUs, checked for their structure
//! (prefix, CRC-32, final byte, NAL header and emulation prevention) for every frame a
//! writer could make. HDR10+ and HDR Vivid go through the independent parsers in
//! `hdr_dynamic/tests.rs`.

use crate::dovi::{self, Frame, Level1, Profile, Writer};
use crate::hdr::Metadata;
use crate::hdr_dynamic::{FrameStats, Smoothed};
use proptest::prelude::*;

prop_compose! {
    fn metadata()(
        primaries in proptest::array::uniform3(proptest::array::uniform2(any::<u16>())),
        white in proptest::array::uniform2(any::<u16>()),
        maximum_nits in any::<u16>(),
        minimum in any::<u16>(),
        max_cll in any::<u16>(),
        max_fall in any::<u16>(),
        full_frame_nits in any::<u16>(),
    ) -> Metadata {
        Metadata { primaries, white, maximum_nits, minimum, max_cll, max_fall, full_frame_nits }
    }
}

/// `SS_HDR_METADATA` after the enabled byte as moonlight-common-c reads it: 13 little-endian
/// words in the order of the struct.
fn read_wire(wire: &[u8; 27]) -> (bool, Metadata) {
    let word = |i: usize| u16::from_le_bytes([wire[1 + 2 * i], wire[2 + 2 * i]]);
    (
        wire[0] == 1,
        Metadata {
            primaries: [[word(0), word(1)], [word(2), word(3)], [word(4), word(5)]],
            white: [word(6), word(7)],
            maximum_nits: word(8),
            minimum: word(9),
            max_cll: word(10),
            max_fall: word(11),
            full_frame_nits: word(12),
        },
    )
}

fn float() -> impl Strategy<Value = f32> {
    prop_oneof![
        any::<f32>(),
        -1f32..20_000.,
        Just(f32::NAN),
        Just(f32::INFINITY),
        Just(f32::NEG_INFINITY),
        Just(0.)
    ]
}

/// Removes HEVC emulation prevention bytes.
fn unescape(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut zeros = 0;
    for &b in data {
        if zeros == 2 && b == 3 {
            zeros = 0;
            continue;
        }
        out.push(b);
        zeros = if b == 0 { zeros + 1 } else { 0 };
    }
    out
}

fn check_rpu(frame: &Frame) -> Result<(), TestCaseError> {
    let rpu = frame.rpu();
    prop_assert_eq!(&rpu[..3], &[0x19, 0x08, 0x09]);
    prop_assert_eq!(rpu.last(), Some(&0x80));
    let crc_at = rpu.len() - 5;
    prop_assert_eq!(
        dovi::crc32_mpeg2(&rpu[1..crc_at]).to_be_bytes(),
        &rpu[crc_at..rpu.len() - 1]
    );
    prop_assert_eq!(
        frame.rpu(),
        rpu.clone(),
        "the same frame writes the same RPU"
    );
    let nal = frame.nal();
    prop_assert_eq!(&nal[..6], &[0, 0, 0, 1, 62 << 1, 0x01]);
    let body = &nal[6..];
    for at in 0..body.len().saturating_sub(2) {
        prop_assert!(
            !(body[at] == 0 && body[at + 1] == 0 && body[at + 2] <= 2),
            "start code prefix 00 00 {:02x} at {} left unescaped",
            body[at + 2],
            at
        );
    }
    prop_assert_eq!(unescape(body), rpu);
    Ok(())
}

prop_compose! {
    fn frame()(
        l1 in proptest::array::uniform3(0u16..=4095),
        target_nits in any::<u16>(),
        metadata in metadata(),
        scene_refresh in any::<bool>(),
        hlg in any::<bool>(),
    ) -> Frame {
        Frame {
            level1: Level1::clamped(l1[0], l1[1], l1[2]),
            target_nits,
            metadata,
            scene_refresh,
            profile: if hlg { Profile::P84 } else { Profile::P81 },
        }
    }
}

#[derive(Debug, Clone)]
enum Step {
    Metadata(Metadata),
    Target(u16),
    Sample(FrameStats, bool),
    Forget,
    Frame(bool),
}

fn steps() -> impl Strategy<Value = Vec<Step>> {
    let stats = (
        proptest::array::uniform3(prop_oneof![0f32..=1., Just(1.5f32), Just(f32::NAN)]),
        0f32..=10_000.,
        proptest::array::uniform9(0f32..=1.),
    )
        .prop_map(|(pq, mean_nits, percentiles_pq)| FrameStats {
            min_pq: pq[0],
            mean_pq: pq[1],
            max_pq: pq[2],
            mean_nits,
            percentiles_pq,
        });
    let step = prop_oneof![
        1 => metadata().prop_map(Step::Metadata),
        1 => any::<u16>().prop_map(Step::Target),
        2 => (stats, any::<bool>()).prop_map(|(s, cut)| Step::Sample(s, cut)),
        1 => Just(Step::Forget),
        4 => any::<bool>().prop_map(Step::Frame),
    ];
    proptest::collection::vec(step, 1..24)
}

proptest! {
    /// Moonlight reads back every field the host writes, in place.
    #[test]
    fn static_metadata_round_trips(metadata in metadata(), enabled in any::<bool>()) {
        prop_assert_eq!(read_wire(&metadata.wire(enabled)), (enabled, metadata));
    }

    /// The display's luminance, whatever Windows reports, becomes bounded wire values.
    #[test]
    fn display_luminance_is_always_in_range(maximum in float(), minimum in float(), full in float()) {
        let m = Metadata::display(maximum, minimum, full);
        prop_assert!(m.maximum_nits >= 1);
        prop_assert!(m.full_frame_nits <= m.maximum_nits);
        prop_assert_eq!(read_wire(&m.wire(true)).1, m);
    }

    /// L1 stays within CM v2.9's limits for any input.
    #[test]
    fn level1_is_always_within_its_limits(min in any::<u16>(), avg in any::<u16>(), max in any::<u16>()) {
        let l1 = Level1::clamped(min, avg, max);
        prop_assert!(l1.min_pq <= 12);
        prop_assert!((2081..=4095).contains(&l1.max_pq));
        prop_assert!(l1.avg_pq >= 819 && l1.avg_pq < l1.max_pq);
    }

    #[test]
    fn every_rpu_is_well_formed(frame in frame()) {
        check_rpu(&frame)?;
    }

    /// The RPU carries the frame's statistics and profile: frames that differ in them write
    /// different RPUs.
    #[test]
    fn rpus_differ_with_their_statistics(a in frame(), b in frame()) {
        let b = Frame { target_nits: a.target_nits, metadata: a.metadata, scene_refresh: a.scene_refresh, ..b };
        if (a.level1, a.profile) != (b.level1, b.profile) {
            prop_assert_ne!(a.rpu(), b.rpu());
        }
    }

    /// Whatever a stream does to its writer, every frame's RPU is well formed, keyframes
    /// refresh the scene, and the writer keeps its profile.
    #[test]
    fn the_writer_makes_only_well_formed_rpus(target in any::<u16>(), hlg in any::<bool>(), steps in steps()) {
        let profile = if hlg { Profile::P84 } else { Profile::P81 };
        let mut writer = Writer::new(target).with_profile(profile);
        for step in steps {
            match step {
                Step::Metadata(m) => writer.set_metadata(m),
                Step::Target(t) => writer.set_target_nits(t),
                Step::Sample(stats, scene_cut) => writer.sample(&Smoothed { stats, scene_cut }),
                Step::Forget => writer.forget_measurements(),
                Step::Frame(keyframe) => {
                    let frame = writer.frame(keyframe);
                    prop_assert_eq!(frame.profile, profile);
                    prop_assert!(!keyframe || frame.scene_refresh);
                    prop_assert_eq!(frame.target_nits, writer.target_nits());
                    check_rpu(&frame)?;
                }
            }
        }
    }
}
