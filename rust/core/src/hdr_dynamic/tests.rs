use super::*;

/// MSB-first reader for the independent parsers below, written from the syntax tables
/// (ST 2094-40 / CTA-861 as FFmpeg parses it, CUVA 005.1, H.265 7.3, AV1 5.3 and 5.8).
struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, at: 0 }
    }

    fn bits(&mut self, count: usize) -> u32 {
        assert!(self.at + count <= self.data.len() * 8, "read past the end");
        let mut value = 0;
        for _ in 0..count {
            let bit = (self.data[self.at / 8] >> (7 - self.at % 8)) & 1;
            value = (value << 1) | u32::from(bit);
            self.at += 1;
        }
        value
    }

    /// Only zero padding to the next byte boundary may remain.
    fn finish(mut self) {
        let left = self.data.len() * 8 - self.at;
        assert!(left < 8, "{left} bits left over");
        assert_eq!(self.bits(left), 0, "padding must be zero");
    }
}

#[derive(Debug, Default)]
struct ParsedHdr10Plus {
    application_version: u32,
    num_windows: u32,
    target: u32,
    maxscl: [u32; 3],
    average_maxrgb: u32,
    distribution: Vec<(u32, u32)>,
    fraction_bright_pixels: u32,
    tone_mapping: bool,
    saturation_mapping: bool,
}

fn parse_hdr10plus(t35: &[u8]) -> ParsedHdr10Plus {
    let mut r = Reader::new(t35);
    assert_eq!(r.bits(8), 0xB5, "itu_t_t35_country_code");
    assert_eq!(r.bits(16), 0x003C, "itu_t_t35_terminal_provider_code");
    assert_eq!(r.bits(16), 0x0001, "terminal_provider_oriented_code");
    assert_eq!(r.bits(8), 4, "application_identifier");
    let mut parsed = ParsedHdr10Plus {
        application_version: r.bits(8),
        num_windows: r.bits(2),
        ..Default::default()
    };
    assert_eq!(parsed.num_windows, 1, "this parser covers one window");
    parsed.target = r.bits(27);
    if r.bits(1) == 1 {
        let (rows, columns) = (r.bits(5), r.bits(5));
        for _ in 0..rows * columns {
            r.bits(4);
        }
    }
    for component in &mut parsed.maxscl {
        *component = r.bits(17);
    }
    parsed.average_maxrgb = r.bits(17);
    for _ in 0..r.bits(4) {
        parsed.distribution.push((r.bits(7), r.bits(17)));
    }
    parsed.fraction_bright_pixels = r.bits(10);
    if r.bits(1) == 1 {
        let (rows, columns) = (r.bits(5), r.bits(5));
        for _ in 0..rows * columns {
            r.bits(4);
        }
    }
    parsed.tone_mapping = r.bits(1) == 1;
    if parsed.tone_mapping {
        r.bits(12);
        r.bits(12);
        for _ in 0..r.bits(4) {
            r.bits(10);
        }
    }
    parsed.saturation_mapping = r.bits(1) == 1;
    if parsed.saturation_mapping {
        r.bits(6);
    }
    r.finish();
    parsed
}

fn parse_vivid(t35: &[u8]) -> [u32; 4] {
    let mut r = Reader::new(t35);
    assert_eq!(r.bits(8), 0x26, "itu_t_t35_country_code");
    assert_eq!(r.bits(16), 0x0004, "itu_t_t35_terminal_provider_code");
    assert_eq!(r.bits(16), 0x0005, "terminal_provider_oriented_code");
    assert!((1..=7).contains(&r.bits(8)), "system_start_code");
    let statistics = [r.bits(12), r.bits(12), r.bits(12), r.bits(12)];
    assert_eq!(r.bits(1), 0, "tone_mapping_mode_flag");
    assert_eq!(r.bits(1), 0, "color_saturation_mapping_flag");
    r.finish();
    statistics
}

/// Every SEI message in one Annex B prefix SEI NAL unit, as (payloadType, payload).
fn parse_hevc_sei(nal: &[u8]) -> Vec<(usize, Vec<u8>)> {
    let body = nal
        .strip_prefix(&[0, 0, 0, 1])
        .or_else(|| nal.strip_prefix(&[0, 0, 1]))
        .expect("start code");
    assert_eq!(body[0] >> 7, 0, "forbidden_zero_bit");
    assert_eq!((body[0] >> 1) & 0x3F, 39, "PREFIX_SEI_NUT");
    assert_eq!(((body[0] & 1) << 5) | (body[1] >> 3), 0, "nuh_layer_id");
    assert_eq!(body[1] & 7, 1, "nuh_temporal_id_plus1");
    let mut rbsp = Vec::new();
    let mut zeros = 0;
    for &byte in &body[2..] {
        if zeros == 2 {
            assert!(byte > 2, "unescaped 00 00 {byte:02x}");
            if byte == 3 {
                zeros = 0;
                continue; // emulation_prevention_three_byte
            }
        }
        rbsp.push(byte);
        zeros = if byte == 0 { zeros + 1 } else { 0 };
    }
    let mut messages = Vec::new();
    let mut at = 0;
    let ff_coded = |at: &mut usize| {
        let mut value = 0;
        loop {
            let byte = rbsp[*at];
            *at += 1;
            value += usize::from(byte);
            if byte != 0xFF {
                return value;
            }
        }
    };
    while rbsp[at..] != [0x80] {
        let kind = ff_coded(&mut at);
        let size = ff_coded(&mut at);
        messages.push((kind, rbsp[at..at + size].to_vec()));
        at += size;
    }
    messages
}

/// (metadata_type, payload) of one AV1 metadata OBU, trailing bits removed like dav1d.
fn parse_av1_metadata(obu: &[u8]) -> (u64, Vec<u8>) {
    assert_eq!(obu[0] >> 7, 0, "obu_forbidden_bit");
    assert_eq!((obu[0] >> 3) & 0x0F, 5, "OBU_METADATA");
    assert_eq!(obu[0] & 0x04, 0, "obu_extension_flag");
    assert_eq!(obu[0] & 0x02, 0x02, "obu_has_size_field");
    let mut at = 1;
    let size = leb128(obu, &mut at);
    assert_eq!(size as usize, obu.len() - at, "obu_size");
    let kind = leb128(obu, &mut at);
    let mut end = obu.len();
    while obu[end - 1] == 0 {
        end -= 1;
    }
    assert_eq!(obu[end - 1], 0x80, "trailing_one_bit");
    (kind, obu[at..end - 1].to_vec())
}

fn leb128(data: &[u8], at: &mut usize) -> u64 {
    let mut value = 0;
    for index in 0..8 {
        let byte = data[*at];
        *at += 1;
        value |= u64::from(byte & 0x7F) << (7 * index);
        if byte & 0x80 == 0 {
            break;
        }
    }
    value
}

/// A bright game frame: 0.05 to 4000 nits, mean 180 nits, highlights near 1500.
fn sample() -> FrameStats {
    let nits = [0.5, 2., 5., 20., 80., 200., 600., 900., 1500.];
    FrameStats {
        min_pq: nits_to_pq(0.05),
        mean_pq: nits_to_pq(60.),
        max_pq: nits_to_pq(4000.),
        mean_nits: 180.,
        percentiles_pq: nits.map(nits_to_pq),
    }
}

#[test]
fn pq_matches_st_2084_reference_points() {
    for (nits, pq) in [(100., 0.508), (1000., 0.752), (10_000., 1.)] {
        assert!((nits_to_pq(nits) - pq).abs() < 0.001, "{nits} nits");
        assert!(
            (pq_to_nits(nits_to_pq(nits)) - nits).abs() < nits * 1e-3,
            "{nits}"
        );
    }
    assert_eq!(nits_to_pq(f32::NAN), 0.);
    assert_eq!(nits_to_pq(-5.), 0.);
    assert_eq!(nits_to_pq(f32::INFINITY), 1.);
    assert_eq!(pq_to_nits(f32::NAN), 0.);
}

#[test]
fn hdr10plus_known_vector_from_the_syntax_table() {
    let metadata = Hdr10Plus {
        target_nits: 1000,
        maxscl: 0,
        average_maxrgb: 0,
        distribution: [0; 9],
    };
    // Derived by hand: num_windows 01, 1000 in 27 bits, the nine 7-bit percentages
    // 24 bits apart, everything else zero, 331 bits padded to 42 bytes.
    let mut expected = vec![
        0xB5, 0x00, 0x3C, 0x00, 0x01, 0x04, 0x01, 0x40, 0x00, 0x1F, 0x40,
    ];
    expected.extend([0; 8]);
    expected.extend([
        0x24, 0x08, 0, 0, 0x28, 0, 0, 0x50, 0, 0, 0xC8, 0, 0x01, 0x90, 0,
    ]);
    expected.extend([
        0x02, 0x58, 0, 0x02, 0xD0, 0, 0x02, 0xF8, 0, 0x03, 0x18, 0, 0, 0, 0,
    ]);
    assert_eq!(metadata.to_t35(), expected);
    assert_eq!(expected.len(), 49);
}

#[test]
fn hdr10plus_round_trips_through_an_independent_parser() {
    let metadata = Hdr10Plus::from_stats(&sample(), 1200).unwrap();
    let t35 = metadata.to_t35();
    assert_eq!(t35.len(), 49);
    let parsed = parse_hdr10plus(&t35);
    assert_eq!(parsed.application_version, 1);
    assert_eq!(parsed.target, 1200);
    assert_eq!(parsed.maxscl, [metadata.maxscl; 3]);
    assert_eq!(
        parsed.average_maxrgb, 1800,
        "180 nits over 10000, in 1/100000"
    );
    let percentages: Vec<u32> = parsed.distribution.iter().map(|(p, _)| *p).collect();
    assert_eq!(percentages, PERCENTAGES.map(u32::from));
    let percentiles: Vec<u32> = parsed.distribution.iter().map(|(_, v)| *v).collect();
    assert_eq!(percentiles, metadata.distribution);
    assert_eq!(parsed.fraction_bright_pixels, 0);
    assert!(!parsed.tone_mapping && !parsed.saturation_mapping);

    // Built by hand past ST 2094-40's range: written at the 10000-nit limit.
    let over = Hdr10Plus {
        target_nits: 40_000,
        ..metadata
    };
    assert_eq!(parse_hdr10plus(&over.to_t35()).target, 10_000);
}

#[test]
fn hdr10plus_takes_maxscl_from_p99_and_fixes_the_reserved_slots() {
    let metadata = Hdr10Plus::from_stats(&sample(), 1000).unwrap();
    // P99 is 1500 nits; the 4000-nit maximum is not what maxSCL reports.
    assert!(
        (metadata.maxscl as i64 - 15_000).abs() <= 2,
        "{}",
        metadata.maxscl
    );
    assert_eq!(metadata.distribution[1], 0, "V1");
    assert_eq!(metadata.distribution[2], 255, "V2");
    assert!(
        (metadata.distribution[4] as i64 - 800).abs() <= 1,
        "P50 80 nits"
    );

    // A frame whose 1% of highlights pull the mean above P99, with a bright 5th
    // percentile that must not raise maxSCL through the reserved slot.
    let mut stats = sample();
    stats.percentiles_pq = [0.1; 9];
    stats.percentiles_pq[1] = 0.9;
    stats.mean_nits = 400.;
    let metadata = Hdr10Plus::from_stats(&stats, 1000).unwrap();
    assert_eq!(metadata.maxscl, 4000, "never below the average");
    assert_eq!(metadata.distribution[1], 0);

    let mut broken = sample();
    broken.percentiles_pq[3] = f32::NAN;
    assert_eq!(Hdr10Plus::from_stats(&broken, 1000), None);
    broken = sample();
    broken.mean_nits = -1.;
    assert_eq!(Hdr10Plus::from_stats(&broken, 1000), None);
    assert_eq!(Hdr10Plus::from_stats(&sample(), 0).unwrap().target_nits, 1);
    assert_eq!(
        Hdr10Plus::from_stats(&sample(), 20_000)
            .unwrap()
            .target_nits,
        10_000
    );
}

#[test]
fn vivid_known_vector_and_round_trip() {
    let metadata = Vivid {
        minimum_maxrgb: 0,
        average_maxrgb: 0x123,
        variance_maxrgb: 0x045,
        maximum_maxrgb: 0xABC,
    };
    let t35 = metadata.to_t35();
    assert_eq!(
        t35,
        [
            0x26, 0x00, 0x04, 0x00, 0x05, 0x01, 0x00, 0x01, 0x23, 0x04, 0x5A, 0xBC, 0x00
        ]
    );
    assert_eq!(parse_vivid(&t35), [0, 0x123, 0x045, 0xABC]);

    let stats = sample();
    let metadata = Vivid::from_stats(&stats).unwrap();
    let code = |pq: f32| (pq * 4095.).round() as u32;
    assert_eq!(
        parse_vivid(&metadata.to_t35()),
        [
            code(stats.min_pq),
            code(stats.mean_pq),
            code(stats.percentiles_pq[6] - stats.percentiles_pq[2]),
            code(stats.max_pq),
        ]
    );
    let mut broken = stats;
    broken.max_pq = 1.5;
    assert_eq!(Vivid::from_stats(&broken), None);
}

#[test]
fn hevc_sei_wraps_and_escapes_the_payload() {
    let vivid = Vivid {
        minimum_maxrgb: 0,
        average_maxrgb: 0x123,
        variance_maxrgb: 0x045,
        maximum_maxrgb: 0xABC,
    }
    .to_t35();
    let mut expected = vec![0, 0, 0, 1, 0x4E, 0x01, 0x04, 13];
    expected.extend(&vivid);
    expected.push(0x80);
    assert_eq!(hevc_t35_sei(&vivid), expected, "nothing to escape");

    // All-zero statistics make seven zero bytes in a row.
    let zeros = Vivid {
        minimum_maxrgb: 0,
        average_maxrgb: 0,
        variance_maxrgb: 0,
        maximum_maxrgb: 0,
    }
    .to_t35();
    let mut expected = vec![
        0, 0, 0, 1, 0x4E, 0x01, 0x04, 13, 0x26, 0, 0x04, 0, 0x05, 0x01,
    ];
    expected.extend([0, 0, 3, 0, 0, 3, 0, 0, 3, 0, 0x80]);
    assert_eq!(hevc_t35_sei(&zeros), expected);
    assert_eq!(parse_hevc_sei(&expected), [(4, zeros)]);

    let hdr10plus = Hdr10Plus::from_stats(&sample(), 1000).unwrap().to_t35();
    assert_eq!(parse_hevc_sei(&hevc_t35_sei(&hdr10plus)), [(4, hdr10plus)]);

    // payloadSize of 255 or more takes 0xFF continuation bytes.
    let long = vec![0xB5; 300];
    let nal = hevc_t35_sei(&long);
    assert_eq!(nal[6..9], [0x04, 0xFF, 45]);
    assert_eq!(parse_hevc_sei(&nal), [(4, long)]);
}

#[test]
fn emulation_prevention_covers_every_start_code_prefix() {
    for (rbsp, escaped) in [
        (&[0, 0, 0][..], &[0, 0, 3, 0][..]),
        (&[0, 0, 1], &[0, 0, 3, 1]),
        (&[0, 0, 2], &[0, 0, 3, 2]),
        (&[0, 0, 3], &[0, 0, 3, 3]),
        (&[0, 0, 4], &[0, 0, 4]),
        (&[0, 0, 0, 0, 0, 0], &[0, 0, 3, 0, 0, 3, 0, 0]),
        (&[0, 1, 0, 0, 0], &[0, 1, 0, 0, 3, 0]),
        (&[0, 0, 0x80], &[0, 0, 0x80]),
    ] {
        let mut out = Vec::new();
        escape(rbsp, &mut out);
        assert_eq!(out, escaped, "{rbsp:02x?}");
    }
}

#[test]
fn av1_obu_carries_the_payload_with_its_size() {
    let vivid = Vivid {
        minimum_maxrgb: 0,
        average_maxrgb: 0x123,
        variance_maxrgb: 0x045,
        maximum_maxrgb: 0xABC,
    }
    .to_t35();
    let mut expected = vec![0x2A, 15, 0x04];
    expected.extend(&vivid);
    expected.push(0x80);
    assert_eq!(av1_t35_obu(&vivid), expected);

    let hdr10plus = Hdr10Plus::from_stats(&sample(), 1000).unwrap().to_t35();
    assert_eq!(parse_av1_metadata(&av1_t35_obu(&hdr10plus)), (4, hdr10plus));
    // Sizes of 128 or more take a second leb128 byte.
    let long = vec![0xB5; 200];
    let obu = av1_t35_obu(&long);
    assert_eq!(obu[1..3], [0xCA, 0x01], "202 as leb128");
    assert_eq!(parse_av1_metadata(&obu), (4, long));
}

fn nal(kind: u8, body: &[u8]) -> Vec<u8> {
    let mut unit = vec![0, 0, 0, 1, kind << 1, 0x01];
    unit.extend(body);
    unit
}

fn nal_types(frame: &[u8]) -> Vec<u8> {
    (0..frame.len().saturating_sub(3))
        .filter(|&at| frame[at..at + 3] == [0, 0, 1])
        .map(|at| (frame[at + 3] >> 1) & 0x3F)
        .collect()
}

#[test]
fn hevc_insert_offset_lands_before_the_first_slice() {
    let aud = nal(35, &[0x50]);
    let vps = nal(32, &[0x0C, 0x01, 0xFF, 0xFF, 0x01, 0x60]);
    let sps = nal(33, &[0x01, 0x01, 0x60, 0x00, 0x00, 0x03, 0x00, 0x90]);
    let pps = nal(34, &[0xC1, 0x72, 0xB4]);
    let sei = nal(39, &[0x05, 0x02, 0xAA, 0xBB, 0x80]);
    let idr = nal(19, &[0xAF, 0x00, 0x00, 0x03, 0x01, 0x12, 0x34]);
    let p_slice = nal(1, &[0xD0, 0x00, 0x00, 0x03, 0x00, 0x56]);

    let headers = [aud.clone(), vps.clone(), sps.clone(), pps.clone()].concat();
    let keyframe = [headers.clone(), idr.clone()].concat();
    assert_eq!(insert_offset(Codec::Hevc, &keyframe), Some(headers.len()));

    let no_aud = [vps.clone(), sps.clone(), pps.clone(), idr.clone()].concat();
    assert_eq!(
        insert_offset(Codec::Hevc, &no_aud),
        Some(no_aud.len() - idr.len())
    );

    let with_sei = [headers.clone(), sei.clone(), idr.clone()].concat();
    assert_eq!(
        insert_offset(Codec::Hevc, &with_sei),
        Some(with_sei.len() - idr.len())
    );

    assert_eq!(insert_offset(Codec::Hevc, &p_slice), Some(0));
    let aud_p = [aud.clone(), p_slice.clone()].concat();
    assert_eq!(insert_offset(Codec::Hevc, &aud_p), Some(aud.len()));

    // Three-byte start codes: before the slice's own start code.
    let short = [&aud[1..], &p_slice[1..]].concat();
    assert_eq!(insert_offset(Codec::Hevc, &short), Some(aud.len() - 1));

    let hdr10plus = Hdr10Plus::from_stats(&sample(), 1000).unwrap().to_t35();
    let vivid = Vivid::from_stats(&sample()).unwrap().to_t35();
    let (hdr10plus_nal, vivid_nal) = (hevc_t35_sei(&hdr10plus), hevc_t35_sei(&vivid));
    let units = frame_units(Codec::Hevc, &sample(), 1000).unwrap();
    assert_eq!(units, [hdr10plus_nal.clone(), vivid_nal.clone()].concat());
    let mut spliced = keyframe.clone();
    assert!(splice(Codec::Hevc, &mut spliced, &units));
    assert_eq!(nal_types(&spliced), [35, 32, 33, 34, 39, 39, 19]);
    assert_eq!(
        spliced,
        [headers, hdr10plus_nal.clone(), vivid_nal.clone(), idr].concat()
    );
    assert_eq!(parse_hevc_sei(&hdr10plus_nal), [(4, hdr10plus)]);
    assert_eq!(parse_hevc_sei(&vivid_nal), [(4, vivid)]);
}

#[test]
fn hevc_without_an_insertion_point_stays_unchanged() {
    let units = frame_units(Codec::Hevc, &sample(), 1000).unwrap();
    let parameter_sets = [nal(32, &[0x0C]), nal(33, &[0x01]), nal(34, &[0xC1])].concat();
    let length_prefixed = vec![0, 0, 0, 7, 0x26, 0x01, 0xAF, 0x12, 0x34, 0x56, 0x78];
    let mut forbidden = nal(1, &[0xD0]);
    forbidden[4] |= 0x80;
    for frame in [parameter_sets, length_prefixed, forbidden, vec![]] {
        let mut copy = frame.clone();
        assert!(!splice(Codec::Hevc, &mut copy, &units), "{frame:02x?}");
        assert_eq!(copy, frame);
    }
    let mut frame = nal(1, &[0xD0]);
    assert!(!splice(Codec::Hevc, &mut frame, &[]), "nothing to insert");
    assert_eq!(frame, nal(1, &[0xD0]));
}

fn obu(kind: u8, body: &[u8]) -> Vec<u8> {
    let mut unit = vec![(kind << 3) | 0x02, body.len() as u8];
    unit.extend(body);
    unit
}

#[test]
fn av1_insert_offset_lands_after_the_sequence_header() {
    let td = obu(2, &[]);
    let seq = obu(1, &[0x00, 0x00, 0x00, 0x0A, 0x0B, 0xF0, 0x00]);
    let frame_obu = obu(6, &[0x10, 0x00, 0x00, 0x00, 0x01, 0x02]);
    let keyframe = [td.clone(), seq.clone(), frame_obu.clone()].concat();
    assert_eq!(
        insert_offset(Codec::Av1, &keyframe),
        Some(td.len() + seq.len())
    );
    let inter = [td.clone(), frame_obu.clone()].concat();
    assert_eq!(insert_offset(Codec::Av1, &inter), Some(td.len()));
    let split = [td.clone(), obu(3, &[0x30]), obu(4, &[0x00, 0x01])].concat();
    assert_eq!(insert_offset(Codec::Av1, &split), Some(td.len()));
    // An extension byte is skipped with the header.
    let mut extended = vec![(1 << 3) | 0x06, 0x00, 1, 0xAA];
    extended.extend(&frame_obu);
    assert_eq!(insert_offset(Codec::Av1, &extended), Some(4));

    let units = frame_units(Codec::Av1, &sample(), 1000).unwrap();
    let hdr10plus = Hdr10Plus::from_stats(&sample(), 1000).unwrap().to_t35();
    assert_eq!(
        units,
        av1_t35_obu(&hdr10plus),
        "HDR10+ only: Vivid has no AV1 carriage"
    );
    let mut spliced = keyframe.clone();
    assert!(splice(Codec::Av1, &mut spliced, &units));
    let at = td.len() + seq.len();
    assert_eq!(spliced[..at], keyframe[..at]);
    assert_eq!(
        parse_av1_metadata(&spliced[at..at + units.len()]),
        (4, hdr10plus)
    );
    assert_eq!(spliced[at + units.len()..], frame_obu);
}

#[test]
fn av1_without_an_insertion_point_stays_unchanged() {
    let units = frame_units(Codec::Av1, &sample(), 1000).unwrap();
    let td = obu(2, &[]);
    let headers_only = [td.clone(), obu(1, &[0x00, 0x0A])].concat();
    let mut sizeless = td.clone();
    sizeless.extend([1 << 3, 0x00, 0x0A]);
    let mut truncated = td.clone();
    truncated.extend([(1 << 3) | 0x02, 40, 0x00]);
    let mut endless_leb = td.clone();
    endless_leb.extend([(1 << 3) | 0x02, 0xFF, 0xFF]);
    // A size of zero padded to eight leb128 bytes is still valid; nine are not.
    let padded_size = |bytes: usize| {
        let mut frame = td.clone();
        frame.push((1 << 3) | 0x02);
        frame.extend(std::iter::repeat_n(0x80, bytes - 1));
        frame.push(0x00);
        [frame, obu(6, &[0x10, 0x20])].concat()
    };
    assert_eq!(
        insert_offset(Codec::Av1, &padded_size(8)),
        Some(td.len() + 9)
    );
    let overlong_leb = padded_size(9);
    let forbidden = [td.clone(), vec![0x80 | (6 << 3) | 0x02, 0]].concat();
    for frame in [
        headers_only,
        sizeless,
        truncated,
        endless_leb,
        overlong_leb,
        forbidden,
        vec![],
    ] {
        let mut copy = frame.clone();
        assert!(!splice(Codec::Av1, &mut copy, &units), "{frame:02x?}");
        assert_eq!(copy, frame);
    }
}

#[test]
fn histogram_statistics_follow_the_bins() {
    // 990 pixels at PQ 0.25, ten at PQ 0.75: P99 still in the dark bin.
    let mut bins = [0u32; 1024];
    bins[255] = 990;
    bins[767] = 10;
    let stats = FrameStats::from_histogram(&bins, 0.2495, 0.7495).unwrap();
    assert!(stats.is_valid());
    assert_eq!(stats.percentiles_pq[..8], [0.25; 8]);
    assert_eq!(stats.percentiles_pq[8], 0.25, "P99 is pixel 990 of 1000");
    let mean_pq = (990. * 255.5 + 10. * 767.5) / 1000. / 1024.;
    assert!((stats.mean_pq - mean_pq).abs() < 1e-5);
    let mean_nits = (990. * pq_to_nits(255.5 / 1024.) + 10. * pq_to_nits(767.5 / 1024.)) / 1000.;
    assert!(
        (stats.mean_nits - mean_nits).abs() < 0.01,
        "{}",
        stats.mean_nits
    );
    assert!(
        stats.mean_nits > pq_to_nits(stats.mean_pq),
        "linear mean above PQ mean"
    );

    bins[767] = 11;
    let stats = FrameStats::from_histogram(&bins, 0.2495, 0.7495).unwrap();
    assert_eq!(stats.percentiles_pq[8], 0.7495, "kept at the frame maximum");

    let mut single = [0u32; 16];
    single[0] = 4;
    let black = FrameStats::from_histogram(&single, 0., 0.).unwrap();
    assert_eq!(black, FrameStats::default(), "kept inside min and max");

    assert_eq!(FrameStats::from_histogram(&[0; 1024], 0., 1.), None);
    assert_eq!(FrameStats::from_histogram(&bins, 0.5, 0.4), None);
    assert_eq!(FrameStats::from_histogram(&bins, f32::NAN, 1.), None);
}

#[test]
fn target_display_prefers_client_then_host_then_1000() {
    assert_eq!(target_display_nits(Some(800.), Some(1500.)), 800);
    assert_eq!(target_display_nits(None, Some(1500.)), 1500);
    assert_eq!(target_display_nits(Some(0.), Some(1500.)), 1500);
    assert_eq!(target_display_nits(Some(f32::NAN), None), 1000);
    assert_eq!(target_display_nits(None, None), 1000);
    assert_eq!(target_display_nits(Some(40_000.), None), 10_000);
}

#[test]
fn smoothing_follows_slowly_and_snaps_at_a_scene_cut() {
    let mut smoother = StatsSmoother::default();
    let first = smoother.update(&sample()).unwrap();
    assert!(first.scene_cut, "the first sample is taken as it is");
    assert_eq!(first.stats, sample());

    // A small change between samples: smoothed, not a cut.
    let mut brighter = sample();
    brighter.mean_pq += 0.03;
    brighter.percentiles_pq[8] += 0.05;
    let step = smoother.update(&brighter).unwrap();
    assert!(!step.scene_cut);
    let expected = sample().mean_pq + StatsSmoother::ALPHA * 0.03;
    assert!((step.stats.mean_pq - expected).abs() < 1e-6);
    assert!(step.stats.percentiles_pq[8] < brighter.percentiles_pq[8]);

    // A lone peak (cursor, subtitle) is not a cut either.
    let mut peak = brighter;
    peak.max_pq = 1.;
    assert!(!smoother.update(&peak).unwrap().scene_cut);

    // Cut to a dark scene: snaps to the new statistics.
    let dark = FrameStats {
        min_pq: 0.,
        mean_pq: 0.1,
        max_pq: 0.3,
        mean_nits: 0.5,
        percentiles_pq: [0.02, 0.03, 0.04, 0.06, 0.1, 0.14, 0.2, 0.25, 0.3],
    };
    let cut = smoother.update(&dark).unwrap();
    assert!(cut.scene_cut);
    assert_eq!(cut.stats, dark);

    // Invalid samples change nothing: neither the smoothed state nor the cut reference.
    let mut nan = dark;
    nan.mean_pq = f32::NAN;
    assert_eq!(smoother.update(&nan), None);
    let mut bright_but_broken = sample();
    bright_but_broken.min_pq = -0.1;
    assert_eq!(smoother.update(&bright_but_broken), None);
    let after = smoother.update(&dark).unwrap();
    assert!(!after.scene_cut, "compared with the last valid sample");
    assert_eq!(after.stats, dark);

    smoother.reset();
    let fresh = smoother.update(&sample()).unwrap();
    assert!(fresh.scene_cut, "a reset starts over");
    assert_eq!(fresh.stats, sample());
}

#[test]
fn frame_units_reject_invalid_statistics() {
    let mut broken = sample();
    broken.min_pq = -0.1;
    assert_eq!(frame_units(Codec::Hevc, &broken, 1000), None);
    assert_eq!(frame_units(Codec::Av1, &broken, 1000), None);
    assert_eq!(Codec::from_video_format(1), Some(Codec::Hevc));
    assert_eq!(Codec::from_video_format(2), Some(Codec::Av1));
    assert_eq!(Codec::from_video_format(0), None);
}
