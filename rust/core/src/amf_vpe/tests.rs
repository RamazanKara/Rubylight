use super::*;

fn stream(transfer: Transfer) -> Stream {
    Stream {
        width: 1920,
        height: 1080,
        transfer,
        matrix: if transfer == Transfer::Sdr { 1 } else { 2 },
        full_range: false,
        ten_bit: transfer != Transfer::Sdr,
        chroma_packing: 0,
        yuv444: false,
    }
}
fn frame(source: Source) -> Frame {
    Frame {
        source,
        hdr_scale: 1.0,
        pointer: false,
        menu: false,
    }
}

#[test]
fn the_setting_is_off_by_default_as_in_foundation_sunshine() {
    let mut config = crate::config::Config::default();
    assert!(!wanted(&config));
    config.values.insert("amd_vpe".into(), "true".into());
    assert!(wanted(&config));
}

#[test]
fn stream_of_reads_the_negotiated_transfer_and_matrix() {
    let mut config = Negotiated::default();
    assert_eq!(Stream::of(&config).transfer, Transfer::Sdr);
    assert_eq!(Stream::of(&config).matrix, 1);
    config.hdr = true;
    assert_eq!(Stream::of(&config).transfer, Transfer::Pq);
    assert!(Stream::of(&config).ten_bit);
    config.hdr_format = rubylight_protocol::hdr::HLG;
    assert_eq!(Stream::of(&config).transfer, Transfer::Hlg);
    config.hdr_format = rubylight_protocol::hdr::DV_84;
    assert_eq!(Stream::of(&config).transfer, Transfer::Hlg);
}

#[test]
fn sdr_streams_convert_to_nv12_with_their_own_matrix_and_range() {
    let output = stream_plan(&stream(Transfer::Sdr)).unwrap();
    assert_eq!(output.format, Format::Nv12);
    assert_eq!(output.profile, vui::PROFILE_709);
    assert_eq!(output.color.matrix, vui::MATRIX_BT709);
    assert_eq!(output.color.range, vui::RANGE_STUDIO);
    let mut s = stream(Transfer::Sdr);
    s.matrix = 0;
    s.full_range = true;
    let output = stream_plan(&s).unwrap();
    assert_eq!(output.profile, vui::PROFILE_JPEG);
    assert_eq!(output.color.matrix, vui::MATRIX_BT601);
    assert_eq!(output.color.range, vui::RANGE_FULL);
    s.matrix = 2;
    s.ten_bit = true;
    let output = stream_plan(&s).unwrap();
    assert_eq!(output.format, Format::P010);
    assert_eq!(output.profile, vui::PROFILE_FULL_2020);
    assert_eq!(output.color.transfer, vui::TRANSFER_BT2020_10);
}

#[test]
fn hdr_streams_convert_to_p010_bt2020_with_pq_or_hlg() {
    let pq = stream_plan(&stream(Transfer::Pq)).unwrap();
    assert_eq!(pq.format, Format::P010);
    assert_eq!(pq.color.transfer, vui::TRANSFER_PQ);
    assert_eq!(pq.color.primaries, vui::PRIMARIES_BT2020);
    assert_eq!(pq.color.matrix, vui::MATRIX_BT2020_NCL);
    let hlg = stream_plan(&stream(Transfer::Hlg)).unwrap();
    assert_eq!(hlg.color.transfer, vui::TRANSFER_HLG);
}

#[test]
fn layouts_amf_cannot_take_keep_the_shader_for_the_whole_stream() {
    let mut s = stream(Transfer::Sdr);
    s.yuv444 = true;
    assert_eq!(stream_plan(&s), Err(Refusal::Yuv444));
    s.yuv444 = false;
    s.chroma_packing = 1;
    assert_eq!(stream_plan(&s), Err(Refusal::PackedChroma));
    s.chroma_packing = 0;
    s.width = 1921;
    assert_eq!(stream_plan(&s), Err(Refusal::Size));
    for refusal in [Refusal::Yuv444, Refusal::PackedChroma, Refusal::Size] {
        assert!(refusal.whole_stream());
    }
    assert!(!Refusal::Pointer.whole_stream());
}

#[test]
fn sdr_input_is_named_with_the_output_curve_so_amf_does_not_relinearize_it() {
    let output = stream_plan(&stream(Transfer::Sdr)).unwrap();
    let input = frame_plan(&output, &frame(Source::Bgra8)).unwrap();
    assert_eq!(input.format, Format::Bgra);
    assert_eq!(input.color.transfer, output.color.transfer);
    assert_eq!(input.color.matrix, vui::MATRIX_RGB);
    assert_eq!(input.hdr_peak_nits, None);
    let mut s = stream(Transfer::Sdr);
    s.matrix = 2;
    s.ten_bit = true;
    let output = stream_plan(&s).unwrap();
    let input = frame_plan(&output, &frame(Source::Bgra8)).unwrap();
    assert_eq!(input.color.transfer, vui::TRANSFER_SRGB);
}

#[test]
fn hdr_inputs_name_their_light_and_the_full_nominal_range() {
    let pq = stream_plan(&stream(Transfer::Pq)).unwrap();
    let scrgb = frame_plan(&pq, &frame(Source::ScRgbF16)).unwrap();
    assert_eq!(scrgb.format, Format::RgbaF16);
    assert_eq!(scrgb.color.primaries, vui::PRIMARIES_CCCS);
    assert_eq!(scrgb.color.transfer, vui::TRANSFER_LINEAR);
    assert_eq!(scrgb.hdr_peak_nits, Some(10_000));
    assert!(!scrgb.hlg_prepass);
    let pq10 = frame_plan(&pq, &frame(Source::Pq10)).unwrap();
    assert_eq!(pq10.format, Format::R10G10B10A2);
    assert_eq!(pq10.color.transfer, vui::TRANSFER_PQ);
    let hlg = stream_plan(&stream(Transfer::Hlg)).unwrap();
    let input = frame_plan(&hlg, &frame(Source::ScRgbF16)).unwrap();
    assert!(input.hlg_prepass);
    assert_eq!(input.color.primaries, vui::PRIMARIES_BT2020);
    assert!(frame_plan(&hlg, &frame(Source::Pq10)).unwrap().hlg_prepass);
}

#[test]
fn frames_the_shader_must_adjust_or_draw_on_stay_with_the_shader() {
    let sdr = stream_plan(&stream(Transfer::Sdr)).unwrap();
    let pq = stream_plan(&stream(Transfer::Pq)).unwrap();
    assert_eq!(
        frame_plan(&sdr, &frame(Source::ScRgbF16)),
        Err(Refusal::HdrSourceOnSdrStream)
    );
    assert_eq!(
        frame_plan(&pq, &frame(Source::Bgra8)),
        Err(Refusal::SdrSourceOnHdrStream)
    );
    let mut scaled = frame(Source::ScRgbF16);
    scaled.hdr_scale = 0.8;
    assert_eq!(frame_plan(&pq, &scaled), Err(Refusal::LuminanceScale));
    let mut pointer = frame(Source::Bgra8);
    pointer.pointer = true;
    assert_eq!(frame_plan(&sdr, &pointer), Err(Refusal::Pointer));
    let mut menu = frame(Source::Bgra8);
    menu.menu = true;
    assert_eq!(frame_plan(&sdr, &menu), Err(Refusal::Menu));
    assert_eq!(Refusal::Menu.code(), "vpe_menu");
}

#[test]
fn the_picture_fills_a_stream_of_its_shape() {
    let full = Rect {
        x: 0,
        y: 0,
        width: 1920,
        height: 1080,
    };
    assert_eq!(content_rect((3840, 2160), (1920, 1080)), full);
    assert_eq!(content_rect((1920, 1080), (1920, 1080)), full);
    // Within a pixel of the shape: no one-pixel bar from rounding.
    assert_eq!(content_rect((2559, 1440), (1920, 1080)), full);
}

#[test]
fn other_shapes_are_centred_between_even_black_bars() {
    // A 32:9 desktop on a 16:10 tablet: bars above and below.
    let rect = content_rect((5120, 1440), (2560, 1600));
    assert_eq!(rect.width, 2560);
    assert_eq!(rect.height, 720);
    assert_eq!(rect.y, 440);
    assert_eq!(rect.x, 0);
    // 4:3 on 16:9: bars left and right, even edges.
    let rect = content_rect((1024, 768), (1920, 1080));
    assert_eq!(rect.height, 1080);
    assert_eq!(rect.width, 1440);
    assert_eq!(rect.x, 240);
    for (source, target) in [((1000, 999), (1920, 1080)), ((333, 1000), (1280, 720))] {
        let rect = content_rect(source, target);
        assert!(rect.x.is_multiple_of(2) && rect.y.is_multiple_of(2));
        assert!(rect.width.is_multiple_of(2) && rect.height.is_multiple_of(2));
        assert!(rect.x + rect.width <= target.0 && rect.y + rect.height <= target.1);
    }
}

#[test]
fn the_tally_keeps_each_paths_mean_and_worst() {
    let mut tally = Tally::default();
    tally.add(true, 300, 2000);
    tally.add(true, 500, 3000);
    tally.add(false, 100, 1000);
    assert_eq!(tally.frames(), 3);
    assert_eq!(tally.mean_vpe_us(), 400.);
    assert_eq!(tally.vpe_worst_us, 500);
    assert_eq!(tally.mean_shader_us(), 100.);
    assert_eq!(tally.mean_capture_to_submit_us(), 2000.);
    assert_eq!(tally.capture_to_submit_worst_us, 3000);
    assert_eq!(Tally::default().mean_vpe_us(), 0.);
}

#[test]
fn amf_surface_numbers_match_surface_h() {
    assert_eq!(Format::Nv12.amf(), 1);
    assert_eq!(Format::Bgra.amf(), 3);
    assert_eq!(Format::P010.amf(), 10);
    assert_eq!(Format::RgbaF16.amf(), 11);
    assert_eq!(Format::R10G10B10A2.amf(), 13);
}

#[test]
fn the_latest_refusal_carries_into_the_next_report_window() {
    let mut tally = Tally::default();
    tally.add(false, 140, 900);
    tally.refused = Some(Refusal::SdrSourceOnHdrStream.code());
    let next = tally.next_window();
    assert_eq!(next.frames(), 0, "the counts start over");
    assert_eq!(next.refused, Some("vpe_sdr_source_on_hdr_stream"));
    assert_eq!(Tally::default().next_window().refused, None);
}
