use super::*;
use crate::dmabuf::{Layer, Object as DmaObject};

fn mode(width: u32, height: u32, refresh_hz: f64) -> Mode {
    Mode { width, height, refresh_hz, interlaced: false, preferred: false }
}

#[test]
fn a_rate_is_a_multiple_within_half_a_hertz() {
    assert_eq!(rate_multiple(60.0, 60), Some(1));
    assert_eq!(rate_multiple(59.94, 60), Some(1));
    assert_eq!(rate_multiple(120.0, 60), Some(2));
    assert_eq!(rate_multiple(144.0, 60), None);
    assert_eq!(rate_multiple(60.0, 0), None);
    assert_eq!(rate_multiple(0.0, 60), None);
}

#[test]
fn the_mode_in_use_stays_when_it_fits_the_stream() {
    let modes = [mode(1920, 1080, 60.0), mode(1920, 1080, 120.0), mode(1280, 720, 60.0)];
    assert_eq!(pick_mode(&modes, Some(0), 60), Some(0));
    // 120 Hz shows 60 fps twice per picture, as well as 60 Hz does once: no modeset for it.
    assert_eq!(pick_mode(&modes, Some(1), 120), Some(1));
}

#[test]
fn a_stream_rate_the_mode_in_use_misses_takes_a_mode_of_that_rate_at_the_same_size() {
    let modes = [mode(3840, 2160, 60.0), mode(3840, 2160, 50.0), mode(1920, 1080, 120.0), mode(3840, 2160, 120.0)];
    assert_eq!(pick_mode(&modes, Some(0), 50), Some(1), "50 fps on a 60 Hz desktop goes to 50 Hz");
    assert_eq!(pick_mode(&modes, Some(0), 120), Some(3), "never another size");
    assert_eq!(pick_mode(&modes, Some(1), 60), Some(0));
}

#[test]
fn the_smallest_multiple_wins_over_a_larger_one() {
    let modes = [mode(1920, 1080, 144.0), mode(1920, 1080, 120.0), mode(1920, 1080, 60.0)];
    assert_eq!(pick_mode(&modes, Some(0), 60), Some(2));
}

#[test]
fn no_fitting_rate_keeps_the_mode_in_use() {
    let modes = [mode(1920, 1080, 144.0), mode(1920, 1080, 165.0)];
    assert_eq!(pick_mode(&modes, Some(1), 60), Some(1));
}

#[test]
fn without_a_mode_in_use_the_preferred_one_is_the_base_and_interlaced_ones_are_never_chosen() {
    let mut modes = vec![mode(3840, 2160, 30.0), mode(1920, 1080, 60.0), mode(1920, 1080, 50.0)];
    modes[1].preferred = true;
    modes[2].interlaced = true;
    assert_eq!(pick_mode(&modes, None, 50), Some(1), "the interlaced 50 Hz mode is not taken");
    modes[1].preferred = false;
    assert_eq!(pick_mode(&modes, None, 30), Some(0), "else the largest");
    assert_eq!(pick_mode(&[], None, 60), None);
}

fn output(connector_id: u32, name: &str, width: u32, height: u32) -> Output {
    Output { connector_id, name: name.into(), crtc_id: connector_id + 100, crtc_index: 0, width, height }
}

#[test]
fn the_only_driven_output_is_the_windows_and_several_are_told_apart_by_size() {
    let one = [output(1, "HDMI-A-1", 1920, 1080)];
    assert_eq!(pick_output(&one, 3840, 2160).unwrap().connector_id, 1);
    let two = [output(1, "HDMI-A-1", 1920, 1080), output(2, "DP-1", 2560, 1440)];
    assert_eq!(pick_output(&two, 2560, 1440).unwrap().connector_id, 2);
    let same = [output(1, "HDMI-A-1", 1920, 1080), output(2, "DP-1", 1920, 1080)];
    assert!(pick_output(&same, 1920, 1080).unwrap_err().contains("2 of 2"));
    assert!(pick_output(&[], 1920, 1080).is_err());
}

fn plane(id: u32, kind: PlaneKind, formats: &[(u32, &[u64])], zpos: Option<Zpos>) -> Plane {
    Plane {
        id,
        kind,
        possible_crtcs: 1,
        formats: formats.iter().map(|(f, m)| (*f, m.to_vec())).collect(),
        zpos,
        crtc_id: 0,
        color_props: true,
    }
}

fn z(min: u64, max: u64, value: u64) -> Option<Zpos> {
    Some(Zpos { min, max, value, immutable: false })
}

fn fixed(value: u64) -> Option<Zpos> {
    Some(Zpos { min: value, max: value, value, immutable: true })
}

const TILED: u64 = 0x0100_0000_0000_0004;

#[test]
fn a_plane_takes_listed_modifiers_and_a_bare_format_takes_linear_or_implicit() {
    let p = plane(1, PlaneKind::Overlay, &[(NV12, &[TILED]), (ARGB8888, &[])], None);
    assert!(p.takes(NV12, TILED));
    assert!(!p.takes(NV12, MOD_LINEAR));
    assert!(p.takes(ARGB8888, MOD_LINEAR) && p.takes(ARGB8888, MOD_INVALID));
    assert!(!p.takes(P010, TILED));
}

#[test]
fn the_picture_goes_on_the_primary_plane_when_it_takes_the_format_and_the_overlay_above() {
    // An Intel-like pipe: a primary and two sprite planes, all with NV12 and ARGB.
    let planes = [
        plane(31, PlaneKind::Primary, &[(NV12, &[MOD_LINEAR, TILED]), (ARGB8888, &[MOD_LINEAR])], z(0, 2, 0)),
        plane(40, PlaneKind::Overlay, &[(NV12, &[MOD_LINEAR, TILED]), (ARGB8888, &[MOD_LINEAR])], z(0, 2, 1)),
        plane(49, PlaneKind::Overlay, &[(NV12, &[MOD_LINEAR, TILED]), (ARGB8888, &[MOD_LINEAR])], z(0, 2, 2)),
        plane(55, PlaneKind::Cursor, &[(ARGB8888, &[MOD_LINEAR])], z(3, 3, 3)),
    ];
    let layouts = candidates(&planes, 0, NV12, TILED);
    assert_eq!(layouts[0], Layout { video: 31, background: None, overlay: Some(40), zpos: Vec::new() });
    assert!(layouts[0].video_on_primary(&planes));
    // Then the picture on a sprite over the primary, still with an overlay above it.
    assert_eq!(layouts[1], Layout { video: 40, background: Some(31), overlay: Some(49), zpos: Vec::new() });
    // Layouts without an overlay plane come after every one with.
    let first_without = layouts.iter().position(|l| l.overlay.is_none()).unwrap();
    assert!(layouts[first_without..].iter().all(|l| l.overlay.is_none()));
    assert!(layouts.iter().all(|l| l.video != 55 && l.overlay != Some(55)), "never the cursor plane");
}

#[test]
fn a_primary_without_the_format_puts_the_picture_on_an_overlay_plane_over_it() {
    // An amdgpu-like CRTC: an RGB-only primary and one overlay plane with NV12.
    let planes = [
        plane(1, PlaneKind::Primary, &[(ARGB8888, &[MOD_LINEAR]), (XRGB8888, &[MOD_LINEAR])], z(0, 0, 0)),
        plane(2, PlaneKind::Overlay, &[(NV12, &[MOD_LINEAR]), (ARGB8888, &[MOD_LINEAR])], z(1, 254, 1)),
    ];
    let layouts = candidates(&planes, 0, NV12, MOD_LINEAR);
    assert_eq!(layouts, vec![Layout { video: 2, background: Some(1), overlay: None, zpos: Vec::new() }], "no third plane for the overlay");
}

#[test]
fn zpos_is_set_so_the_picture_is_above_the_primary_and_the_overlay_above_the_picture() {
    let planes = [
        plane(1, PlaneKind::Primary, &[(ARGB8888, &[MOD_LINEAR])], fixed(0)),
        // The NV12 plane sits high, the ARGB one low: both are moved.
        plane(2, PlaneKind::Overlay, &[(ARGB8888, &[MOD_LINEAR])], z(1, 5, 1)),
        plane(3, PlaneKind::Overlay, &[(NV12, &[MOD_LINEAR])], z(1, 5, 4)),
    ];
    let layouts = candidates(&planes, 0, NV12, MOD_LINEAR);
    assert_eq!(layouts[0], Layout { video: 3, background: Some(1), overlay: Some(2), zpos: vec![(3, 1), (2, 2)] });
}

#[test]
fn an_immutable_zpos_out_of_order_rules_a_layout_out() {
    let planes = [
        plane(1, PlaneKind::Primary, &[(ARGB8888, &[MOD_LINEAR])], fixed(2)),
        plane(2, PlaneKind::Overlay, &[(NV12, &[MOD_LINEAR]), (ARGB8888, &[MOD_LINEAR])], fixed(1)),
    ];
    assert!(candidates(&planes, 0, NV12, MOD_LINEAR).is_empty(), "the picture would be under the primary");
}

#[test]
fn planes_of_another_crtc_are_left_out() {
    let mut other = plane(2, PlaneKind::Overlay, &[(NV12, &[MOD_LINEAR]), (ARGB8888, &[MOD_LINEAR])], None);
    other.possible_crtcs = 0b10;
    let planes = [plane(1, PlaneKind::Primary, &[(NV12, &[MOD_LINEAR])], None), other];
    let layouts = candidates(&planes, 0, NV12, MOD_LINEAR);
    assert_eq!(layouts, vec![Layout { video: 1, background: None, overlay: None, zpos: Vec::new() }]);
    assert!(candidates(&planes, 0, P010, MOD_LINEAR).is_empty(), "no plane takes P010");
}

#[test]
fn without_zpos_a_higher_plane_id_counts_as_above() {
    let planes = [
        plane(5, PlaneKind::Overlay, &[(ARGB8888, &[MOD_LINEAR])], None),
        plane(10, PlaneKind::Primary, &[(NV12, &[MOD_LINEAR])], None),
        plane(20, PlaneKind::Overlay, &[(ARGB8888, &[MOD_LINEAR])], None),
    ];
    assert_eq!(candidates(&planes, 0, NV12, MOD_LINEAR)[0].overlay, Some(20));
}

fn nv12_export(separate: bool, modifier: u64) -> Descriptor {
    let mut d = Descriptor { fourcc: NV12, width: 1920, height: 1088, num_objects: 1, ..Default::default() };
    d.objects[0] = DmaObject { fd: 7, size: 3_133_440, drm_format_modifier: modifier };
    if separate {
        d.num_layers = 2;
        d.layers[0] = Layer { drm_format: fourcc(b"R8  "), num_planes: 1, object_index: [0; 4], offset: [0; 4], pitch: [2048, 0, 0, 0] };
        d.layers[1] = Layer { drm_format: fourcc(b"GR88"), num_planes: 1, object_index: [0; 4], offset: [2048 * 1088, 0, 0, 0], pitch: [2048, 0, 0, 0] };
    } else {
        d.num_layers = 1;
        d.layers[0] = Layer { drm_format: NV12, num_planes: 2, object_index: [0; 4], offset: [0, 2048 * 1088, 0, 0], pitch: [2048, 2048, 0, 0] };
    }
    d
}

#[test]
fn a_vaapi_export_in_separate_layers_or_composed_makes_the_same_nv12_framebuffer() {
    let expected = FbLayout {
        format: NV12,
        width: 1920,
        height: 1088,
        modifier: TILED,
        planes: vec![FbPlane { object: 0, offset: 0, pitch: 2048 }, FbPlane { object: 0, offset: 2048 * 1088, pitch: 2048 }],
    };
    assert_eq!(fb_layout(&nv12_export(true, TILED)), Ok(expected.clone()));
    assert_eq!(fb_layout(&nv12_export(false, TILED)), Ok(expected));
}

#[test]
fn a_ten_bit_export_in_separate_layers_is_p010() {
    let mut d = nv12_export(true, MOD_LINEAR);
    d.fourcc = P010;
    d.layers[0].drm_format = fourcc(b"R16 ");
    d.layers[1].drm_format = fourcc(b"GR32");
    assert_eq!(fb_layout(&d).unwrap().format, P010);
}

#[test]
fn a_broken_export_is_refused_with_why() {
    let mut d = nv12_export(true, MOD_LINEAR);
    d.layers[1].object_index[0] = 3;
    assert!(fb_layout(&d).unwrap_err().contains("object 3"));
    let mut d = nv12_export(true, MOD_LINEAR);
    d.num_objects = 2;
    d.objects[1] = DmaObject { fd: 8, size: 1, drm_format_modifier: TILED };
    assert!(fb_layout(&d).unwrap_err().contains("modifiers"));
    assert!(fb_layout(&Descriptor::default()).is_err());
    let mut d = nv12_export(false, MOD_LINEAR);
    d.layers[0].num_planes = 1;
    assert!(fb_layout(&d).unwrap_err().contains("not 2"));
}

fn in_formats(formats: &[u32], modifiers: &[(u64, u32, u64)]) -> Vec<u8> {
    let header = 24usize;
    let formats_offset = header;
    let modifiers_offset = formats_offset + 4 * formats.len();
    let mut blob = Vec::new();
    for value in [1u32, 0, formats.len() as u32, formats_offset as u32, modifiers.len() as u32, modifiers_offset as u32] {
        blob.extend_from_slice(&value.to_le_bytes());
    }
    for f in formats {
        blob.extend_from_slice(&f.to_le_bytes());
    }
    for (mask, offset, modifier) in modifiers {
        blob.extend_from_slice(&mask.to_le_bytes());
        blob.extend_from_slice(&offset.to_le_bytes());
        blob.extend_from_slice(&0u32.to_le_bytes());
        blob.extend_from_slice(&modifier.to_le_bytes());
    }
    blob
}

#[test]
fn in_formats_gives_each_format_its_modifiers() {
    let blob = in_formats(&[XRGB8888, ARGB8888, NV12], &[(0b111, 0, MOD_LINEAR), (0b100, 0, TILED), (0b1, 1, TILED)]);
    assert_eq!(
        parse_in_formats(&blob),
        Ok(vec![(XRGB8888, vec![MOD_LINEAR]), (ARGB8888, vec![MOD_LINEAR, TILED]), (NV12, vec![MOD_LINEAR, TILED])])
    );
}

#[test]
fn a_short_or_unknown_in_formats_blob_is_an_error() {
    let blob = in_formats(&[NV12], &[(1, 0, MOD_LINEAR)]);
    assert!(parse_in_formats(&blob[..blob.len() - 4]).is_err());
    let mut v2 = blob.clone();
    v2[0] = 2;
    assert!(parse_in_formats(&v2).unwrap_err().contains("version 2"));
    assert!(parse_in_formats(&[]).is_err());
}

fn metadata() -> nightjar_core::control::HdrMetadata {
    nightjar_core::control::HdrMetadata {
        display_primaries: [(34000, 16000), (13250, 34500), (7500, 3000)],
        white_point: (15635, 16450),
        max_display_luminance: 1000,
        min_display_luminance: 50,
        max_content_light_level: 1000,
        max_frame_average_light_level: 400,
        max_full_frame_luminance: 0,
    }
}

#[test]
fn the_hdr_blob_is_the_kernels_static_metadata_type_1_with_pq() {
    let blob = hdr_output_metadata(&metadata());
    assert_eq!(&blob[0..4], &[0, 0, 0, 0], "metadata_type: static type 1");
    assert_eq!(blob[4], 2, "EOTF: SMPTE ST 2084");
    assert_eq!(blob[5], 0);
    let u16_at = |at: usize| u16::from_le_bytes([blob[at], blob[at + 1]]);
    assert_eq!((u16_at(6), u16_at(8)), (34000, 16000), "red first, as the host sends them");
    assert_eq!((u16_at(18), u16_at(20)), (15635, 16450), "white point");
    assert_eq!(u16_at(22), 1000, "max mastering luminance in nits");
    assert_eq!(u16_at(24), 50, "min in 0.0001 nits");
    assert_eq!((u16_at(26), u16_at(28)), (1000, 400), "MaxCLL, MaxFALL");
    assert_eq!(&blob[30..], &[0, 0], "padding to the struct's 32 bytes");
}

fn all_caps() -> Caps {
    Caps { vrr_enabled: true, vrr_capable: true, content_type: true, hdr_metadata: true, colorspace_bt2020: true, max_bpc: Some((16, 8)) }
}

fn sdr() -> Stream {
    Stream { ten_bit: false, hdr: None, vrr: false, color_space: 1, color_range: 0 }
}

fn value_of<'a>(sets: &'a [Set], object: Object, name: &str) -> Option<&'a Value> {
    sets.iter().find(|s| s.object == object && s.name == name).map(|s| &s.value)
}

#[test]
fn an_sdr_stream_asks_for_game_content_clears_hdr_and_sets_its_encoding() {
    let sets = stream_properties(&all_caps(), &sdr(), 40, true);
    assert_eq!(value_of(&sets, Object::Connector, "content type"), Some(&Value::Enum("Game")));
    assert_eq!(value_of(&sets, Object::Crtc, "VRR_ENABLED"), Some(&Value::Number(0)));
    assert_eq!(value_of(&sets, Object::Connector, "HDR_OUTPUT_METADATA"), Some(&Value::HdrBlob(None)));
    assert_eq!(value_of(&sets, Object::Connector, "Colorspace"), Some(&Value::Enum("Default")));
    assert_eq!(value_of(&sets, Object::Connector, "max bpc"), None, "8-bit leaves the depth");
    assert_eq!(value_of(&sets, Object::Plane(40), "COLOR_ENCODING"), Some(&Value::Enum("ITU-R BT.709 YCbCr")));
    assert_eq!(value_of(&sets, Object::Plane(40), "COLOR_RANGE"), Some(&Value::Enum("YCbCr limited range")));
}

#[test]
fn an_hdr_stream_sends_its_metadata_with_bt2020_and_ten_bits() {
    let stream = Stream { ten_bit: true, hdr: Some(metadata()), vrr: true, color_space: 2, color_range: 1 };
    let sets = stream_properties(&all_caps(), &stream, 40, true);
    assert_eq!(value_of(&sets, Object::Connector, "HDR_OUTPUT_METADATA"), Some(&Value::HdrBlob(Some(hdr_output_metadata(&metadata())))));
    assert_eq!(value_of(&sets, Object::Connector, "Colorspace"), Some(&Value::Enum("BT2020_RGB")));
    assert_eq!(value_of(&sets, Object::Connector, "max bpc"), Some(&Value::Number(10)));
    assert_eq!(value_of(&sets, Object::Crtc, "VRR_ENABLED"), Some(&Value::Number(1)));
    assert_eq!(value_of(&sets, Object::Plane(40), "COLOR_ENCODING"), Some(&Value::Enum("ITU-R BT.2020 YCbCr")));
    assert_eq!(value_of(&sets, Object::Plane(40), "COLOR_RANGE"), Some(&Value::Enum("YCbCr full range")));
}

#[test]
fn properties_the_card_lacks_are_left_out() {
    let caps = Caps { vrr_enabled: true, vrr_capable: false, max_bpc: Some((8, 8)), ..Caps::default() };
    let stream = Stream { ten_bit: true, hdr: Some(metadata()), vrr: true, ..sdr() };
    let sets = stream_properties(&caps, &stream, 40, false);
    assert!(!caps.hdr());
    assert_eq!(value_of(&sets, Object::Crtc, "VRR_ENABLED"), Some(&Value::Number(0)), "a connector that cannot run VRR keeps it off");
    assert_eq!(value_of(&sets, Object::Connector, "max bpc"), Some(&Value::Number(8)), "clamped to the range");
    assert!(sets.iter().all(|s| !matches!(s.name, "content type" | "HDR_OUTPUT_METADATA" | "Colorspace" | "COLOR_ENCODING" | "COLOR_RANGE")));
    // Metadata without BT.2020 colorimetry is not HDR: the blob is cleared, not sent.
    let caps = Caps { hdr_metadata: true, ..Caps::default() };
    assert_eq!(value_of(&stream_properties(&caps, &stream, 40, false), Object::Connector, "HDR_OUTPUT_METADATA"), Some(&Value::HdrBlob(None)));
}

#[test]
fn a_ten_bit_depth_already_set_is_not_touched() {
    let caps = Caps { max_bpc: Some((12, 12)), ..Caps::default() };
    assert!(stream_properties(&caps, &Stream { ten_bit: true, ..sdr() }, 1, false).is_empty());
}

#[test]
fn encodings_follow_the_colour_space() {
    assert_eq!(color_encoding(0, false), "ITU-R BT.601 YCbCr");
    assert_eq!(color_encoding(1, false), "ITU-R BT.709 YCbCr");
    assert_eq!(color_encoding(2, false), "ITU-R BT.2020 YCbCr");
    assert_eq!(color_encoding(1, true), "ITU-R BT.2020 YCbCr", "HDR is BT.2020");
}

#[test]
fn an_async_flip_needs_every_condition() {
    assert!(async_flip(true, true, true, true));
    assert!(!async_flip(false, true, true, true), "V-Sync on");
    assert!(!async_flip(true, false, true, true), "no kernel support");
    assert!(!async_flip(true, true, false, true), "the picture on an overlay plane");
    assert!(!async_flip(true, true, true, false), "the overlay or properties change too");
}

#[test]
fn a_vrr_stream_that_asks_for_whole_frames_does_not_tear() {
    // V-Sync off, VRR on at the display, Tear-free VRR.
    assert!(!may_tear(true, true, true), "synchronised flips: each picture whole");
    assert!(may_tear(true, true, false), "Tear-free VRR off: pictures at once, torn");
    assert!(may_tear(true, false, true), "no VRR at the display: a synchronised flip would wait for a fixed refresh");
    assert!(may_tear(true, false, false));
    for (vrr, tear_free) in [(true, true), (true, false), (false, true), (false, false)] {
        assert!(!may_tear(false, vrr, tear_free), "V-Sync on");
    }
    // What stops the tearing wish also stops the async flip.
    assert!(!async_flip(may_tear(true, true, true), true, true, true));
    assert!(async_flip(may_tear(true, true, false), true, true, true));
}

#[test]
fn vrr_is_engaged_only_where_the_crtc_and_the_display_take_it() {
    let caps = |vrr_enabled, vrr_capable| Caps { vrr_enabled, vrr_capable, ..Caps::default() };
    assert!(vrr_engaged(&caps(true, true), true));
    assert!(!vrr_engaged(&caps(true, true), false), "the setting is off");
    assert!(!vrr_engaged(&caps(false, true), true), "the CRTC has no VRR_ENABLED");
    assert!(!vrr_engaged(&caps(true, false), true), "the display is not vrr_capable");
}

#[test]
fn commit_times_sum_up_for_the_log() {
    use std::time::Duration;
    let mut times = CommitTimes::default();
    assert_eq!(times.summary(), None);
    times.add(Duration::from_micros(500));
    times.add(Duration::from_micros(1500));
    times.add(Duration::from_micros(4000));
    assert_eq!(times.summary().as_deref(), Some("3 pictures committed, 2.00 ms each on average, 4.00 ms at worst"));
}

#[test]
fn a_plane_rect_is_rounded_and_kept_inside_the_crtc() {
    let rect = plane_rect((1920, 1080), (0.0, 140.4, 2560.0, 1440.0), (2560, 1600));
    assert_eq!(rect, PlaneRect { src_w: 1920, src_h: 1080, crtc_x: 0, crtc_y: 140, crtc_w: 2560, crtc_h: 1440 });
    let rect = plane_rect((1920, 1080), (-3.0, 0.0, 4000.0, 1080.0), (1920, 1080));
    assert_eq!((rect.crtc_x, rect.crtc_w), (0, 1920));
}

#[test]
fn fourcc_names_read_as_their_characters() {
    assert_eq!(fourcc_name(NV12), "NV12");
    assert_eq!(fourcc_name(fourcc(b"R8  ")), "R8??");
}

/// A base block and one CTA-861 extension holding `blocks` as its data blocks.
fn edid(blocks: &[&[u8]]) -> Vec<u8> {
    let mut edid = vec![0u8; 256];
    edid[126] = 1;
    edid[128] = 0x02;
    edid[129] = 3;
    let mut at = 132;
    for block in blocks {
        edid[at..at + block.len()].copy_from_slice(block);
        at += block.len();
    }
    edid[130] = at as u8;
    edid
}

#[test]
fn the_edid_says_hdr10_in_its_hdr_static_metadata_block() {
    // A video data block, then HDR static metadata: SDR, PQ and HLG, static type 1.
    let hdr = edid(&[&[0x43, 0x10, 0x04, 0x03], &[0xE3, 0x06, 0b1101, 0x01]]);
    assert!(edid_supports_pq(&hdr));
    let hlg_only = edid(&[&[0xE3, 0x06, 0b1001, 0x01]]);
    assert!(!edid_supports_pq(&hlg_only), "HLG without PQ");
    let colorimetry = edid(&[&[0xE3, 0x05, 0xC0, 0x00]]);
    assert!(!edid_supports_pq(&colorimetry), "another extended block");
    assert!(!edid_supports_pq(&[0u8; 128]), "no extension");
    assert!(!edid_supports_pq(&[]));
}
