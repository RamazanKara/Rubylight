use super::*;
use butterpollo_core::hdr_dynamic::{FrameStats, frame_units, insert_offset};

fn nal(kind: u8, body: &[u8]) -> Vec<u8> {
    let mut unit = vec![0, 0, 0, 1, kind << 1, 0x01];
    unit.extend(body);
    unit
}
fn keyframe() -> Vec<u8> {
    [
        nal(32, &[0x0C]),
        nal(33, &[0x01]),
        nal(34, &[0xC1]),
        nal(19, &[0xAF, 0x12]),
    ]
    .concat()
}
/// A readout as the GPU pass leaves it: ten cells in each bin from `low`
/// to `high`.
fn readout(conversion: u64, low: usize, high: usize) -> HdrStatsReadout {
    let mut histogram = vec![0; crate::compute::HDR_STATS_BINS];
    histogram[low..=high].fill(10);
    let edge = |bin: usize| bin as f32 / crate::compute::HDR_STATS_BINS as f32;
    HdrStatsReadout {
        conversion,
        histogram,
        min_pq: edge(low),
        max_pq: edge(high),
    }
}

#[test]
fn only_hdr_hevc_and_av1_with_the_setting_on_get_dynamic_metadata() {
    let on = butterpollo_core::config::Config::default();
    let off = butterpollo_core::config::Config::parse("hdr_dynamic_metadata = false\n").unwrap();
    let config = |codec, hdr| butterpollo_core::rtsp::Negotiated {
        codec,
        hdr,
        ..Default::default()
    };
    assert_eq!(
        wanted(&config(1, true), &on),
        Some(Wanted::t35(Codec::Hevc))
    );
    assert_eq!(wanted(&config(2, true), &on), Some(Wanted::t35(Codec::Av1)));
    assert_eq!(wanted(&config(1, false), &on), None, "SDR");
    assert_eq!(wanted(&config(0, true), &on), None, "H.264");
    assert_eq!(wanted(&config(3, true), &on), None, "PyroWave");
    for codec in [1, 2] {
        assert_eq!(wanted(&config(codec, true), &off), None, "setting off");
    }
}

#[test]
fn ten_bit_sdr_never_gets_dynamic_metadata() {
    // Hardware lane 4a: a 10-bit SDR launch streamed as HDR switched HDR10+ on.
    let on = butterpollo_core::config::Config::default();
    for codec in [1, 2] {
        // RTSP reads dynamicRangeMode=1 as HDR; hdrMode=0 at launch makes it SDR.
        let mut stream = butterpollo_core::rtsp::Negotiated {
            codec,
            hdr: true,
            ..Default::default()
        };
        assert!(wanted(&stream, &on).is_some(), "an HDR launch keeps it");
        assert!(butterpollo_core::stream_policy::infer_sdr_10bit(
            &mut stream,
            true
        ));
        assert_eq!(wanted(&stream, &on), None, "codec {codec}");
        // Marked SDR but with `hdr` left set: still none.
        stream.hdr = true;
        assert_eq!(wanted(&stream, &on), None, "codec {codec}, hdr left set");
    }
}

#[test]
fn frames_before_the_first_sample_carry_the_static_stand_in() {
    let mut dynamic = Dynamic::new(Codec::Hevc);
    let frame = keyframe();
    let at = insert_offset(Codec::Hevc, &frame).unwrap();
    let with = |metadata: &butterpollo_core::hdr::Metadata| {
        let units = frame_units(Codec::Hevc, &FrameStats::from_static(metadata)).unwrap();
        (
            [&frame[..at], &units, &frame[at..]].concat(),
            units.len() as u64,
        )
    };
    // Frame 0 comes out before its statistics: it carries the stand-in.
    let (expected, added) = with(&Default::default());
    assert_eq!(dynamic.first_unit(&frame), Some(expected));
    assert_eq!(dynamic.cap(Some(5000)), Some(5000 + added));
    // The stream's static metadata rebuilds it.
    let mut metadata = butterpollo_core::hdr::Metadata::display(600., 0.01, 300.);
    metadata.max_cll = 450;
    dynamic.set_metadata(metadata);
    // Nothing finished yet: still the stand-in.
    dynamic.take(Ok(None), Instant::now());
    assert_eq!(dynamic.first_unit(&frame), Some(with(&metadata).0));
    assert_eq!(dynamic.sampled(), 0);
    assert_eq!((dynamic.report.spliced, dynamic.report.stand_in), (2, 2));
    // An invalid first sample keeps it too.
    let mut empty = readout(4, 0, 0);
    empty.histogram.fill(0);
    dynamic.take(Ok(Some(empty)), Instant::now());
    assert_eq!(dynamic.first_unit(&frame), Some(with(&metadata).0));
    // A sample replaces it.
    let sample = readout(5, 200, 600);
    let units = frame_units(Codec::Hevc, &sample.frame_stats().unwrap()).unwrap();
    dynamic.take(Ok(Some(sample)), Instant::now());
    assert_eq!(
        dynamic.first_unit(&frame),
        Some([&frame[..at], &units, &frame[at..]].concat())
    );
    assert_eq!((dynamic.report.spliced, dynamic.report.stand_in), (4, 3));
}

#[test]
fn a_sample_is_spliced_into_every_frame_until_the_next() {
    let mut dynamic = Dynamic::new(Codec::Hevc);
    let first = readout(7, 200, 600);
    let stats = first.frame_stats().unwrap();
    dynamic.take(Ok(Some(first)), Instant::now());
    assert_eq!(dynamic.sampled(), 7, "the next readout must be newer");
    let units = frame_units(Codec::Hevc, &stats).unwrap();
    let frame = keyframe();
    let at = insert_offset(Codec::Hevc, &frame).unwrap();
    let expected = [&frame[..at], &units, &frame[at..]].concat();
    for _ in 0..3 {
        assert_eq!(dynamic.first_unit(&frame).as_ref(), Some(&expected));
        assert_eq!(dynamic.cap(Some(5000)), Some(5000 + units.len() as u64));
        assert_eq!(dynamic.cap(Some(0)), Some(0), "unrestricted stays so");
        assert_eq!(dynamic.cap(None), None);
    }
    // A frame without an insertion point goes out as it is, its cap too.
    assert_eq!(dynamic.first_unit(&nal(32, &[0x0C])), None);
    assert_eq!(dynamic.cap(Some(5000)), Some(5000));
    assert_eq!(dynamic.report.no_insertion, 1);
    // An empty output is no frame to report.
    assert_eq!(dynamic.first_unit(&[]), None);
    assert_eq!(dynamic.cap(Some(5000)), Some(5000));
    assert_eq!(dynamic.report.no_insertion, 1);
    // A cut to a dark scene is taken as it is.
    let dark = readout(9, 10, 80);
    let dark_stats = dark.frame_stats().unwrap();
    dynamic.take(Ok(Some(dark)), Instant::now());
    let units = frame_units(Codec::Hevc, &dark_stats).unwrap();
    assert_eq!(
        dynamic.first_unit(&frame),
        Some([&frame[..at], &units, &frame[at..]].concat())
    );
    assert_eq!(dynamic.report.scene_cuts, 2);
}

#[test]
fn a_failed_readout_keeps_the_previous_units() {
    let mut dynamic = Dynamic::new(Codec::Av1);
    dynamic.take(Ok(Some(readout(3, 100, 900))), Instant::now());
    let frame = [
        vec![0x12, 0x00],
        vec![0x0A, 0x01, 0x00],
        vec![0x32, 0x01, 0x10],
    ]
    .concat();
    let before = dynamic.first_unit(&frame).unwrap();
    dynamic.take(Err(anyhow::anyhow!("inconsistent")), Instant::now());
    let mut empty = readout(4, 0, 0);
    empty.histogram.fill(0);
    dynamic.take(Ok(Some(empty)), Instant::now());
    assert_eq!(dynamic.first_unit(&frame), Some(before));
    assert_eq!(
        (dynamic.report.readout_errors, dynamic.report.invalid),
        (1, 1)
    );
    assert_eq!(dynamic.sampled(), 4, "an invalid sample is not read again");
}

/// A statistics pass that fails to submit on call `fail_at`.
#[derive(Default)]
struct FakeStats {
    enables: u32,
    enable_fails: bool,
    submits: u32,
    fail_at: Option<u32>,
    stopped: bool,
    readout: Option<HdrStatsReadout>,
}
impl Stats for FakeStats {
    fn enable(&mut self) -> anyhow::Result<()> {
        self.enables += 1;
        if self.enable_fails {
            anyhow::bail!("root signature rejected");
        }
        Ok(())
    }
    fn submit(&mut self) -> anyhow::Result<()> {
        self.submits += 1;
        if self.fail_at == Some(self.submits) {
            anyhow::bail!("command allocator reset failed");
        }
        Ok(())
    }
    fn latest(&self, newer_than: u64) -> anyhow::Result<Option<HdrStatsReadout>> {
        Ok(self
            .readout
            .clone()
            .filter(|readout| readout.conversion > newer_than))
    }
    fn stop(&mut self) {
        self.stopped = true;
    }
}

#[test]
fn a_failed_statistics_pass_turns_dynamic_metadata_off_loudly() {
    let warnings = Warnings::default();
    let mut stats = FakeStats {
        fail_at: Some(2),
        readout: Some(readout(5, 200, 600)),
        ..Default::default()
    };
    let mut slot = Some(Box::new(Dynamic::new(Codec::Hevc)));
    sample(&mut slot, Some(&mut stats), &warnings);
    assert_eq!(slot.as_deref().map(Dynamic::sampled), Some(5));
    assert!(warnings.snapshot().is_empty());
    // The second pass fails: no error reaches the encode, which keeps
    // its frames; the stream says why it lost its dynamic metadata.
    sample(&mut slot, Some(&mut stats), &warnings);
    assert!(slot.is_none(), "dynamic metadata stayed on");
    assert!(stats.stopped, "the converter kept queueing passes");
    let shown = warnings.snapshot();
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].code, "hdr_dynamic_metadata");
    assert!(
        shown[0]
            .message
            .contains("GPU statistics pass failed (command allocator reset failed)"),
        "{}",
        shown[0].message
    );
    // Off for the rest of the stream: no more passes.
    sample(&mut slot, Some(&mut stats), &warnings);
    assert_eq!(stats.submits, 2);
}

#[test]
fn a_stream_without_the_statistics_pass_says_why_it_carries_static_hdr10_only() {
    let shown = |stats: Option<&mut FakeStats>| {
        let warnings = Warnings::default();
        let dynamic = start(Wanted::t35(Codec::Hevc), false, stats, &warnings);
        assert!(dynamic.is_none());
        let shown = warnings.snapshot();
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].code, "hdr_dynamic_metadata");
        assert!(
            shown[0]
                .message
                .starts_with("HDR10+ and HDR Vivid are off for this stream: ")
                && shown[0]
                    .message
                    .ends_with(". It carries static HDR10 metadata only"),
            "{}",
            shown[0].message
        );
        shown[0].message.clone()
    };
    assert!(shown(None::<&mut FakeStats>).contains("no GPU statistics pass"));
    let mut failing = FakeStats {
        enable_fails: true,
        ..Default::default()
    };
    assert!(
        shown(Some(&mut failing))
            .contains("statistics pass did not start (root signature rejected)")
    );
    // With the pass, dynamic metadata is on and nothing is shown.
    let mut stats = FakeStats::default();
    for codec in [Codec::Av1, Codec::Hevc] {
        let warnings = Warnings::default();
        let dynamic = start(Wanted::t35(codec), false, Some(&mut stats), &warnings);
        assert!(dynamic.is_some_and(|dynamic| dynamic.measuring));
        assert!(warnings.snapshot().is_empty());
    }
    assert_eq!(stats.enables, 2);
}

#[test]
fn packed_444_runs_no_statistics_pass_for_dynamic_metadata() {
    // Its bitstream says SDR and the device unpacks it in its own
    // shader, with the control channel's HDR10 metadata: no decoder
    // would read HDR10+ or HDR Vivid, so the GPU never measures for it.
    let warnings = Warnings::default();
    let mut stats = FakeStats::default();
    for codec in [Codec::Hevc, Codec::Av1] {
        assert!(start(Wanted::t35(codec), true, Some(&mut stats), &warnings).is_none());
    }
    assert_eq!(stats.enables, 0);
    assert!(warnings.snapshot().is_empty());
}

#[test]
fn packed_444_streams_ask_with_their_own_config_not_the_coded_one() {
    // amf.rs codes a packed stream as SDR; `wanted` gets the stream's
    // config, so an HDR packed stream reaches `start` and says why it
    // carries no dynamic metadata.
    let mut config = butterpollo_core::rtsp::Negotiated {
        codec: 1,
        hdr: true,
        chroma_packing: 1,
        ..Default::default()
    };
    let on = butterpollo_core::config::Config::default();
    assert_eq!(wanted(&config, &on), Some(Wanted::t35(Codec::Hevc)));
    config.hdr = false;
    assert_eq!(wanted(&config, &on), None);
}

fn dolby_vision(t35: bool) -> Wanted {
    Wanted {
        codec: Codec::Hevc,
        t35,
        dolby_vision: true,
        hlg: false,
    }
}

/// The RPU a frame ends with: its NAL unit from the last start code on.
fn ending_rpu(bytes: &[u8]) -> &[u8] {
    let at = bytes
        .windows(6)
        .rposition(|window| window[..4] == [0, 0, 0, 1] && window[4] == 62 << 1)
        .expect("no RPU");
    &bytes[at..]
}

#[test]
fn dolby_vision_is_wanted_for_hevc_streams_that_picked_it() {
    let on = butterpollo_core::config::Config::default();
    let off = butterpollo_core::config::Config::parse("hdr_dynamic_metadata = false\n").unwrap();
    let config = |codec, hdr_format| butterpollo_core::rtsp::Negotiated {
        codec,
        hdr: true,
        hdr_format,
        ..Default::default()
    };
    assert_eq!(wanted(&config(1, 4), &on), Some(dolby_vision(true)));
    // Its RPUs do not need HDR10+.
    assert_eq!(wanted(&config(1, 4), &off), Some(dolby_vision(false)));
    // A stock pick (none), PQ, profile 8 on AV1 and 10.1 on HEVC carry
    // none.
    assert_eq!(wanted(&config(1, 0), &off), None);
    assert_eq!(wanted(&config(1, 1), &off), None);
    assert_eq!(wanted(&config(2, 4), &off), None);
    assert_eq!(wanted(&config(1, 16), &off), None);
    // AV1 that picked 10.1, with HDR10+ beside it while the setting is on.
    assert_eq!(wanted(&config(2, 16), &on), Some(av1_dolby_vision(true)));
    assert_eq!(wanted(&config(2, 16), &off), Some(av1_dolby_vision(false)));
    let (writer, rpus) = av1_dolby_vision(true).writers(1000);
    assert!(!writer.hlg());
    assert_eq!(writer.codec(), Codec::Av1);
    assert_eq!(rpus.unwrap().profile(), dovi::Profile::P101);
}

fn av1_dolby_vision(t35: bool) -> Wanted {
    Wanted {
        codec: Codec::Av1,
        ..dolby_vision(t35)
    }
}

/// An AV1 temporal unit as AMF writes it: a temporal delimiter, a
/// sequence header and one frame OBU, each with a size field.
fn av1_frame(tile: u8) -> Vec<u8> {
    [
        vec![0x12, 0x00],
        vec![0x0A, 0x01, 0x00],
        vec![0x32, 0x01, tile],
    ]
    .concat()
}

#[test]
fn an_av1_dolby_vision_frame_carries_its_rpu_obu_before_the_frame_after_hdr10_plus() {
    let warnings = Warnings::default();
    let mut stats = FakeStats {
        readout: Some(readout(6, 100, 700)),
        ..Default::default()
    };
    let mut slot = start(av1_dolby_vision(true), false, Some(&mut stats), &warnings);
    sample(&mut slot, Some(&mut stats), &warnings);
    let dynamic = slot.as_deref_mut().unwrap();
    let measured = readout(6, 100, 700).frame_stats().unwrap();
    let units = frame_units(Codec::Av1, &measured).unwrap();
    let mut rpus = dovi::Writer::new(1000).with_profile(dovi::Profile::P101);
    rpus.sample(
        &hdr_dynamic::StatsSmoother::default()
            .update(&measured)
            .unwrap(),
    );
    let frame = av1_frame(0x10);
    let at = insert_offset(Codec::Av1, &frame).unwrap();
    assert_eq!(at, 5, "after the temporal delimiter and sequence header");
    // A whole frame: HDR10+, then the RPU, then the frame OBU; nothing after.
    let rpu = rpus.frame(true).obu();
    let (bytes, cap) = output(Some(&mut *dynamic), Unit::Whole, &frame, Some(5000), true);
    assert_eq!(
        bytes,
        Some([&frame[..at], &units, &rpu, &frame[at..]].concat())
    );
    assert_eq!(cap, Some(5000 + (units.len() + rpu.len()) as u64));
    // Tiles streamed as units: the first takes both, the last nothing.
    let rpu = rpus.frame(false).obu();
    let grown = Some(5000 + (units.len() + rpu.len()) as u64);
    let mut sequencer = super::super::units::Sequencer::default();
    let tiles = vec![(1, av1_frame(0x21)), (2, vec![0x22, 0x01, 0x22])];
    let sent = deliver(&mut sequencer, Some(&mut *dynamic), 2, STREAMED, &tiles);
    let first = av1_frame(0x21);
    assert_eq!(
        sent,
        [
            (
                Unit::Slice { index: 0 },
                [&first[..at], &units, &rpu, &first[at..]].concat(),
                grown
            ),
            (Unit::Last { index: 1 }, vec![0x22, 0x01, 0x22], grown),
        ]
    );
    assert_eq!(
        (dynamic.report.rpus, dynamic.report.unmeasured_rpus),
        (2, 0)
    );
    assert!(warnings.snapshot().is_empty());
}

#[test]
fn an_av1_dolby_vision_frame_without_hdr10_plus_or_a_place_says_so() {
    // HDR10+ off: the RPU alone before the frame.
    let mut dynamic = Dynamic::with(av1_dolby_vision(false), false);
    let mut rpus = dovi::Writer::new(1000).with_profile(dovi::Profile::P101);
    let frame = av1_frame(0x10);
    let rpu = rpus.frame(false).obu();
    let (bytes, cap) = output(Some(&mut dynamic), Unit::Whole, &frame, Some(5000), false);
    assert_eq!(bytes, Some([&frame[..5], &rpu, &frame[5..]].concat()));
    assert_eq!(cap, Some(5000 + rpu.len() as u64));
    assert_eq!((dynamic.report.spliced, dynamic.report.stand_in), (1, 0));
    // A temporal unit with no frame OBU has no place: it goes out as AMF
    // wrote it, counted, and the RPU is not appended after it.
    let (bytes, cap) = output(
        Some(&mut dynamic),
        Unit::Whole,
        &frame[..5],
        Some(5000),
        false,
    );
    assert_eq!((bytes, cap), (None, Some(5000)));
    assert_eq!(dynamic.report.no_insertion, 1);
    assert!(dynamic.rpu.is_empty());
}

#[test]
fn hlg_streams_want_hdr_vivid_on_hevc_and_dolby_vision_8_4() {
    let on = butterpollo_core::config::Config::default();
    let off = butterpollo_core::config::Config::parse("hdr_dynamic_metadata = false\n").unwrap();
    let config = |codec, hdr_format| butterpollo_core::rtsp::Negotiated {
        codec,
        hdr: true,
        hdr_format,
        ..Default::default()
    };
    let hlg = |t35, dolby_vision| Wanted {
        codec: Codec::Hevc,
        t35,
        dolby_vision,
        hlg: true,
    };
    assert_eq!(wanted(&config(1, 2), &on), Some(hlg(true, false)));
    assert_eq!(wanted(&config(1, 2), &off), None);
    assert_eq!(wanted(&config(1, 8), &on), Some(hlg(true, true)));
    assert_eq!(wanted(&config(1, 8), &off), Some(hlg(false, true)));
    // AV1 HLG: HDR Vivid has no AV1 carriage and HDR10+ is PQ only.
    assert_eq!(wanted(&config(2, 2), &on), None);
    // The writers follow: HLG units and 8.4 RPUs.
    let (writer, rpus) = hlg(true, true).writers(1000);
    assert!(writer.hlg());
    assert_eq!(rpus.unwrap().profile(), dovi::Profile::P84);
    let (writer, rpus) = dolby_vision(true).writers(1000);
    assert!(!writer.hlg());
    assert_eq!(rpus.unwrap().profile(), dovi::Profile::P81);
}

#[test]
fn a_dolby_vision_8_4_frame_carries_hdr_vivid_for_hlg_and_an_8_4_rpu() {
    let warnings = Warnings::default();
    let mut stats = FakeStats {
        readout: Some(readout(6, 100, 700)),
        ..Default::default()
    };
    let wanted = Wanted {
        hlg: true,
        ..dolby_vision(true)
    };
    let mut slot = start(wanted, false, Some(&mut stats), &warnings);
    sample(&mut slot, Some(&mut stats), &warnings);
    let dynamic = slot.as_deref_mut().unwrap();
    let measured = readout(6, 100, 700).frame_stats().unwrap();
    let units = hdr_dynamic::hlg_frame_units(Codec::Hevc, &measured).unwrap();
    let mut rpus = dovi::Writer::new(1000).with_profile(dovi::Profile::P84);
    rpus.sample(
        &hdr_dynamic::StatsSmoother::default()
            .update(&measured)
            .unwrap(),
    );
    let rpu = rpus.frame(false).nal();
    let frame = keyframe();
    let at = insert_offset(Codec::Hevc, &frame).unwrap();
    let (bytes, cap) = output(Some(dynamic), Unit::Whole, &frame, Some(5000), false);
    assert_eq!(
        bytes,
        Some([&frame[..at], &units, &frame[at..], &rpu].concat())
    );
    assert_eq!(cap, Some(5000 + (units.len() + rpu.len()) as u64));
    assert!(warnings.snapshot().is_empty());
}

#[test]
fn every_dolby_vision_frame_ends_with_an_rpu_counted_in_its_cap() {
    let mut dynamic = Dynamic::with(dolby_vision(false), false);
    let mut metadata = butterpollo_core::hdr::Metadata::display(1000., 0.005, 400.);
    metadata.max_fall = 150;
    dynamic.set_metadata(metadata);
    dynamic.set_target(600);
    let mut rpus = dovi::Writer::new(600);
    rpus.set_metadata(metadata);
    // A keyframe, whole: the RPU after its last slice.
    let frame = keyframe();
    let expected = rpus.frame(true).nal();
    let (bytes, cap) = output(Some(&mut dynamic), Unit::Whole, &frame, Some(5000), true);
    assert_eq!(bytes, Some([frame.clone(), expected.clone()].concat()));
    assert_eq!(cap, Some(5000 + expected.len() as u64));
    // A streamed frame in slices: the first and middle units go out as
    // they are, the last takes the RPU, and all count it.
    let expected = rpus.frame(false).nal();
    let grown = Some(5000 + expected.len() as u64);
    let mut sequencer = super::super::units::Sequencer::default();
    let [streamed, ..] = frames();
    let sent = deliver(
        &mut sequencer,
        Some(&mut dynamic),
        1,
        streamed.0,
        &streamed.1,
    );
    assert_eq!(
        sent,
        [
            (
                Unit::Slice { index: 0 },
                [aud(), nal(1, &[0xA1])].concat(),
                grown
            ),
            (Unit::Slice { index: 1 }, nal(1, &[0xA2]), grown),
            (
                Unit::Last { index: 2 },
                [nal(1, &[0xA3]), expected].concat(),
                grown
            ),
        ]
    );
    // Static metadata only: the stream's light levels as L1.
    let parsed = ending_rpu(&sent[2].1);
    assert_eq!(parsed, &rpus.frame(false).nal()[..]);
    assert_eq!(dynamic.report.unmeasured_rpus, 2);
    // An empty output gets no RPU.
    assert_eq!(
        output(Some(&mut dynamic), Unit::Whole, &[], Some(5000), false),
        (None, Some(5000))
    );
}

#[test]
fn dolby_vision_takes_each_sample_and_hdr10_plus_goes_first() {
    let warnings = Warnings::default();
    let mut stats = FakeStats {
        readout: Some(readout(6, 100, 700)),
        ..Default::default()
    };
    let mut slot = start(dolby_vision(true), false, Some(&mut stats), &warnings);
    sample(&mut slot, Some(&mut stats), &warnings);
    let dynamic = slot.as_deref_mut().unwrap();
    let measured = readout(6, 100, 700).frame_stats().unwrap();
    let units = frame_units(Codec::Hevc, &measured).unwrap();
    let mut rpus = dovi::Writer::new(1000);
    rpus.sample(
        &hdr_dynamic::StatsSmoother::default()
            .update(&measured)
            .unwrap(),
    );
    let rpu = rpus.frame(false).nal();
    let frame = keyframe();
    let at = insert_offset(Codec::Hevc, &frame).unwrap();
    let (bytes, cap) = output(Some(dynamic), Unit::Whole, &frame, Some(5000), false);
    assert_eq!(
        bytes,
        Some([&frame[..at], &units, &frame[at..], &rpu].concat())
    );
    assert_eq!(cap, Some(5000 + (units.len() + rpu.len()) as u64));
    assert_eq!(dynamic.report.unmeasured_rpus, 0);
    assert!(warnings.snapshot().is_empty());
}

#[test]
fn dolby_vision_without_statistics_goes_on_from_the_static_metadata_and_says_so() {
    // An input without the statistics pass: HDR10+ off, RPUs from the
    // static values.
    let warnings = Warnings::default();
    let mut slot = start(dolby_vision(true), false, None::<&mut FakeStats>, &warnings);
    let dynamic = slot.as_deref_mut().expect("Dolby Vision went off");
    assert!(!dynamic.measuring && !dynamic.wanted.t35);
    let mut codes: Vec<_> = warnings.snapshot().into_iter().map(|w| w.code).collect();
    codes.sort();
    assert_eq!(codes, ["hdr_dolby_vision", "hdr_dynamic_metadata"]);
    let frame = keyframe();
    let (bytes, _) = output(Some(dynamic), Unit::Whole, &frame, None, false);
    assert_eq!(&bytes.unwrap()[..frame.len()], &frame[..], "HDR10+ went in");

    // A pass that fails mid-stream: the RPUs go back to the static values.
    let warnings = Warnings::default();
    let mut stats = FakeStats {
        fail_at: Some(2),
        readout: Some(readout(5, 200, 600)),
        ..Default::default()
    };
    let mut slot = start(dolby_vision(true), false, Some(&mut stats), &warnings);
    sample(&mut slot, Some(&mut stats), &warnings);
    assert!(slot.as_ref().unwrap().dovi.as_ref().unwrap().measured());
    sample(&mut slot, Some(&mut stats), &warnings);
    let dynamic = slot.as_deref_mut().expect("Dolby Vision went off");
    assert!(stats.stopped && !dynamic.measuring && !dynamic.wanted.t35);
    assert!(!dynamic.dovi.as_ref().unwrap().measured());
    let shown = warnings.snapshot();
    assert_eq!(shown.len(), 2);
    assert!(
        shown
            .iter()
            .all(|w| w.message.contains("the rest of this stream")),
        "{shown:?}"
    );
    let (bytes, _) = output(Some(dynamic), Unit::Whole, &frame, None, false);
    let bytes = bytes.unwrap();
    assert_eq!(&bytes[..frame.len()], &frame[..]);
    let mut rpus = dovi::Writer::new(1000);
    assert_eq!(ending_rpu(&bytes), &rpus.frame(false).nal()[..]);
    sample(&mut slot, Some(&mut stats), &warnings);
    assert_eq!(stats.submits, 2, "a pass after the failure");
}

#[test]
fn packed_444_never_carries_dolby_vision() {
    let mut stats = FakeStats::default();
    for t35 in [false, true] {
        let warnings = Warnings::default();
        let slot = start(dolby_vision(t35), true, Some(&mut stats), &warnings);
        assert!(slot.is_none());
        let shown = warnings.snapshot();
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].code, "hdr_dolby_vision");
        assert!(shown[0].message.contains("packed 4:4:4"));
    }
    assert_eq!(stats.enables, 0);
}

/// What the sender gets for one frame AMF returned as `units`
/// (`OutputBufferType`, bytes), as `Encoder::poll_once` hands them on.
fn deliver(
    sequencer: &mut super::super::units::Sequencer,
    mut dynamic: Option<&mut Dynamic>,
    pts: i64,
    meta: super::super::units::Meta,
    units: &[(i64, Vec<u8>)],
) -> Vec<(Unit, Vec<u8>, Option<u64>)> {
    use super::super::units::Step;
    let mut sent = vec![];
    for (kind, bytes) in units {
        let (unit, bytes) = match sequencer.push(pts, Some(*kind), bytes, || meta).step {
            Step::Hold => continue,
            Step::Unit(unit, bytes) => (unit, bytes),
            Step::Whole(bytes) => (Unit::Whole, bytes),
        };
        let (spliced, cap) = output(dynamic.as_deref_mut(), unit, &bytes, Some(5000), meta.idr);
        sent.push((unit, spliced.unwrap_or(bytes), cap));
    }
    sent
}
const STREAMED: super::super::units::Meta = super::super::units::Meta {
    streamed: true,
    idr: false,
};
const KEYFRAME: super::super::units::Meta = super::super::units::Meta {
    streamed: false,
    idr: true,
};
fn aud() -> Vec<u8> {
    nal(35, &[0x50])
}
/// A frame's metadata and its `(pts, bytes)` slices.
type Frame = (super::super::units::Meta, Vec<(i64, Vec<u8>)>);
/// A streamed frame in three slices, the first behind an AUD; a
/// one-slice frame; and a keyframe in three slices, joined.
fn frames() -> [Frame; 3] {
    [
        (
            STREAMED,
            vec![
                (1, [aud(), nal(1, &[0xA1])].concat()),
                (1, nal(1, &[0xA2])),
                (2, nal(1, &[0xA3])),
            ],
        ),
        (STREAMED, vec![(2, [aud(), nal(1, &[0xB1])].concat())]),
        (
            KEYFRAME,
            vec![
                (
                    1,
                    [
                        aud(),
                        nal(32, &[0x0C]),
                        nal(33, &[0x01]),
                        nal(34, &[0xC1]),
                        nal(19, &[0xC2]),
                    ]
                    .concat(),
                ),
                (1, nal(19, &[0xC3])),
                (2, nal(19, &[0xC4])),
            ],
        ),
    ]
}

#[test]
fn slice_output_carries_dynamic_metadata_on_each_frames_first_unit() {
    let mut dynamic = Dynamic::new(Codec::Hevc);
    let sample = readout(2, 150, 650);
    let units = frame_units(Codec::Hevc, &sample.frame_stats().unwrap()).unwrap();
    dynamic.take(Ok(Some(sample)), Instant::now());
    let grown = Some(5000 + units.len() as u64);
    let mut sequencer = super::super::units::Sequencer::default();
    let [streamed, single, keyframe] = frames();
    // Slice 0 alone gets the units, after its AUD; the frame's every
    // unit gets the cap it gained.
    assert_eq!(
        deliver(
            &mut sequencer,
            Some(&mut dynamic),
            1,
            streamed.0,
            &streamed.1
        ),
        [
            (
                Unit::Slice { index: 0 },
                [aud(), units.clone(), nal(1, &[0xA1])].concat(),
                grown
            ),
            (Unit::Slice { index: 1 }, nal(1, &[0xA2]), grown),
            (Unit::Last { index: 2 }, nal(1, &[0xA3]), grown),
        ]
    );
    assert_eq!(
        deliver(&mut sequencer, Some(&mut dynamic), 2, single.0, &single.1),
        [(
            Unit::Last { index: 0 },
            [aud(), units.clone(), nal(1, &[0xB1])].concat(),
            grown
        )]
    );
    // A keyframe is joined: the units go after its AUD and parameter
    // sets, before its first IDR slice.
    assert_eq!(
        deliver(
            &mut sequencer,
            Some(&mut dynamic),
            3,
            keyframe.0,
            &keyframe.1
        ),
        [(
            Unit::Whole,
            [
                aud(),
                nal(32, &[0x0C]),
                nal(33, &[0x01]),
                nal(34, &[0xC1]),
                units.clone(),
                nal(19, &[0xC2]),
                nal(19, &[0xC3]),
                nal(19, &[0xC4]),
            ]
            .concat(),
            grown
        )]
    );
    // A first slice with no place for the units: none of its frame's
    // units count any.
    let bare = [(1, aud()), (2, nal(1, &[0xD1]))];
    assert_eq!(
        deliver(&mut sequencer, Some(&mut dynamic), 4, STREAMED, &bare),
        [
            (Unit::Slice { index: 0 }, aud(), Some(5000)),
            (Unit::Last { index: 1 }, nal(1, &[0xD1]), Some(5000)),
        ]
    );
}

#[test]
fn without_dynamic_metadata_every_output_byte_is_amfs() {
    let off = butterpollo_core::config::Config::parse("hdr_dynamic_metadata = false\n").unwrap();
    let config = butterpollo_core::rtsp::Negotiated {
        codec: 1,
        hdr: true,
        ..Default::default()
    };
    assert_eq!(wanted(&config, &off), None, "the setting is off");
    // Off: what AMF wrote, unspliced.
    let mut sequencer = super::super::units::Sequencer::default();
    for (pts, (meta, units)) in frames().into_iter().enumerate() {
        let sent = deliver(&mut sequencer, None, pts as i64, meta, &units);
        let input: Vec<u8> = units.iter().flat_map(|(_, bytes)| bytes.clone()).collect();
        let bytes: Vec<u8> = sent
            .iter()
            .flat_map(|(_, bytes, _)| bytes.clone())
            .collect();
        assert_eq!(bytes, input);
        assert!(sent.iter().all(|&(_, _, cap)| cap == Some(5000)));
    }
    assert_eq!(
        output(None, Unit::Whole, &keyframe(), None, true),
        (None, None)
    );
}

#[test]
fn the_receivers_display_peak_leaves_the_units() {
    let mut dynamic = Dynamic::new(Codec::Hevc);
    let sample = readout(1, 300, 700);
    let stats: FrameStats = sample.frame_stats().unwrap();
    dynamic.take(Ok(Some(sample)), Instant::now());
    dynamic.set_target(2600);
    assert_eq!(dynamic.display_peak, 2600);
    let frame = keyframe();
    let at = insert_offset(Codec::Hevc, &frame).unwrap();
    let units = frame_units(Codec::Hevc, &stats).unwrap();
    assert_eq!(
        dynamic.first_unit(&frame),
        Some([&frame[..at], &units, &frame[at..]].concat())
    );
}
