use super::*;

/// A D3D11VA 4:4:4 surface is drawn as one 4:4:4 picture of its depth; nothing else is.
#[test]
fn a_d3d11_444_surface_has_a_full_chroma_layout_of_its_depth() {
    let info = |kind, ten_bit| FrameInfo { width: 64, height: 32, kind, ten_bit, pts: 0, data: [std::ptr::null_mut(); 3],
        linesize: [0; 3], color_space: 1, color_range: 1, color_trc: 1 };
    for ten_bit in [0, 1] {
        assert_eq!(PlanarLayout::of_d3d11(&info(FRAME_D3D11_444, ten_bit)),
            Some(PlanarLayout { width: 64, height: 32, chroma444: true, ten_bit: ten_bit != 0 }));
        assert_eq!(PlanarLayout::of(&info(FRAME_D3D11_444, ten_bit)), None, "no planes in memory");
    }
    assert_eq!(PlanarLayout::of_d3d11(&info(FRAME_D3D11, 1)), None);
    assert_eq!(PlanarLayout::of_d3d11(&FrameInfo { width: 0, ..info(FRAME_D3D11_444, 0) }), None);
    // NVDEC's high-bit 10-bit 4:4:4 is a planar 10-bit 4:4:4 picture on Linux.
    #[cfg(unix)]
    assert_eq!(PlanarLayout::of(&info(FRAME_YUV444P16, 1)),
        Some(PlanarLayout { width: 64, height: 32, chroma444: true, ten_bit: true }));
    #[cfg(not(unix))]
    assert_eq!(PlanarLayout::of(&info(FRAME_YUV444P16, 1)), None);
}

/// Opened without a packet timebase, V4L2 M2M rescaled each pts through the unset 0/1
/// `time_base`, so no picture kept its frame's number.
#[test]
fn a_v4l2_picture_keeps_the_pts_of_its_frame() {
    let timebase = packet_timebase(Hardware::V4l2m2m);
    for number in [0u32, 1, 2, 61, u32::MAX] {
        let pts = (i64::from(number) << 32) | i64::from(number.wrapping_mul(1500));
        assert_eq!(v4l2_pts_round_trip(pts, timebase), pts, "frame {number}");
    }
    assert_ne!(v4l2_pts_round_trip(1 << 32, None), 1 << 32, "the unset timebase loses it");
    assert_eq!(packet_timebase(Hardware::None), None, "the other decoders are left as they were");
    assert_eq!(packet_timebase(Hardware::Vaapi), None);
}

#[test]
fn software_decodes_packed_fixtures_and_preserves_timestamps() {
    for (codec, bytes, ten_bit) in [
        (
            Codec::Hevc,
            include_bytes!("../../../../app/src/main/assets/packed444/hevc-8.bin").as_slice(),
            0,
        ),
        (
            Codec::Hevc,
            include_bytes!("../../../../app/src/main/assets/packed444/hevc-10.bin").as_slice(),
            1,
        ),
        (
            Codec::Av1,
            include_bytes!("../../../../app/src/main/assets/packed444/av1-8.bin").as_slice(),
            0,
        ),
        (
            Codec::Av1,
            include_bytes!("../../../../app/src/main/assets/packed444/av1-10.bin").as_slice(),
            1,
        ),
    ] {
        let mut decoder = VideoDecoder::open(codec, Hardware::None, 1, false).unwrap();
        assert_eq!(decoder.send(bytes, 12345), 0);
        let Received::Frame(frame) = decoder.receive() else {
            panic!("no decoded frame")
        };
        let info = frame.info();
        assert_eq!((info.width, info.height, info.pts), (128, 128, 12345));
        assert!(!info.data[0].is_null());
        if ten_bit == 0 {
            assert_eq!(info.kind, FRAME_YUV420P);
        }
        assert!(matches!(decoder.receive(), Received::None));
    }
}

#[test]
fn access_units_lose_only_their_trailing_zeros() {
    assert_eq!(access_unit(&[0, 0, 0, 1, 0x7c, 0x01, 0x80, 0, 0, 0]), &[0, 0, 0, 1, 0x7c, 0x01, 0x80]);
    assert_eq!(access_unit(&[0, 0, 1, 2]), &[0, 0, 1, 2]);
    assert_eq!(access_unit(&[0, 0]), &[0, 0]);
    assert_eq!(access_unit(&[]), &[] as &[u8]);
}

/// A Rubylight host's profile 8.1 RPU (the host's `dovi::tests::GOLDEN`, read back by dovi_tool)
/// after an HEVC Main10 picture and the last shard's padding: FFmpeg reads it, L1 peak and all.
#[cfg(windows)]
#[test]
fn a_padded_hevc_frame_keeps_its_dolby_vision_rpu() {
    const RPU: &str = concat!(
        "000000017c0119080908406136506f003ff801ffc00fffd0000008000006800000400000340000030200",
        "000301a2566000035ea2566f9fceb1c256644ca00000100000030080000003008000000301c362243018",
        "60a5e308e0514000001a63e5affff000000300000300000300060207d80e15103008005dc32000c02b23",
        "8008008008008004000120c07d00064064001680d76703ed80",
    );
    let mut frame = include_bytes!("../../../../app/src/main/assets/packed444/hevc-10.bin").to_vec();
    frame.extend((0..RPU.len()).step_by(2).map(|at| u8::from_str_radix(&RPU[at..at + 2], 16).unwrap()));
    frame.extend([0; 300]);
    let mut decoder = VideoDecoder::open(Codec::Hevc, Hardware::None, 1, false).unwrap();
    assert_eq!(decoder.send(&frame, 1), 0);
    let Received::Frame(picture) = decoder.receive() else { panic!("no decoded frame") };
    let rpu = picture.dolby_vision().expect("the RPU's metadata on the picture");
    // L1 maximum PQ 3000 of 4095.
    let peak = crate::stream::direct::pq_nits(3000.0 / 4095.0) as f32;
    assert!((rpu.peak.unwrap() - peak).abs() < 1.0, "{:?} against {peak}", rpu.peak);
}

/// A VAAPI pool whose pictures may be mapped for the GPU renderer gets the surfaces the
/// presenter keeps until their copies are done, and opens on the renderer's render node; no
/// other pool changes (nightjar#80).
#[test]
fn a_vaapi_pool_that_maps_keeps_surfaces_for_the_presenters_copies() {
    let mapped = Pictures { vaapi_node: VaapiNode::Mapped(129), ..Pictures::default() };
    assert_eq!(mapped_extra(Hardware::Vaapi, mapped), if cfg!(unix) { crate::vulkan_video::mapped_held() } else { 0 });
    assert_eq!(mapped_extra(Hardware::Vaapi, Pictures::default()), 0);
    assert_eq!(mapped_extra(Hardware::Cuda, mapped), 0);
    // The OpenGL renderer imports DMA-BUFs: no copies to wait for.
    assert_eq!(mapped_extra(Hardware::Vaapi, Pictures { vaapi_node: VaapiNode::Display(129), ..Pictures::default() }), 0);
    assert_eq!(vaapi_node_path(129).to_str(), Ok("/dev/dri/renderD129"));
}

/// The OpenGL renderer's node (`Hardware::Auto` without a GPU device) is where VAAPI opens,
/// for the probes and the stream alike; without one it opens on the default device, as before.
#[test]
fn vaapi_opens_on_the_opengl_renderers_node_when_it_names_one() {
    let expected = if cfg!(target_os = "linux") { VaapiNode::Display(129) } else { VaapiNode::Default };
    assert_eq!(vaapi_node(Hardware::Auto(ptr::null_mut(), Some(129))), expected);
    assert_eq!(expected.minor(), cfg!(target_os = "linux").then_some(129));
    assert_eq!(vaapi_node(Hardware::Auto(ptr::null_mut(), None)), VaapiNode::Default);
    assert_eq!(vaapi_node(Hardware::Vaapi), VaapiNode::Default);
    assert_eq!(VaapiNode::Default.minor(), None);
    assert_eq!(VaapiNode::Mapped(128).minor(), Some(128));
}

#[test]
fn software_decodes_444_fixtures_with_full_chroma_and_10_bit_as_planar_kinds() {
    let source = include_bytes!("../../tests/fixtures/yuv444-8.yuv");
    for (codec, bytes) in [
        (Codec::H264, include_bytes!("../../tests/fixtures/h264-444-8.bin").as_slice()),
        (Codec::Hevc, include_bytes!("../../tests/fixtures/hevc-444-8.bin").as_slice()),
        (Codec::Av1, include_bytes!("../../tests/fixtures/av1-444-8.bin").as_slice()),
    ] {
        let mut decoder = VideoDecoder::open(codec, Hardware::None, 1, false).unwrap();
        assert_eq!(decoder.send(bytes, 7), 0, "{codec:?}");
        let Received::Frame(frame) = decoder.receive() else { panic!("{codec:?}: no decoded frame") };
        let info = frame.info();
        // 256x256: a 64x64 probe was under NVDEC's minimum size and never offered NVIDIA 4:4:4.
        assert_eq!((info.kind, info.ten_bit, info.width, info.height), (FRAME_YUV444P, 0, 256, 256), "{codec:?}");
        // The fixtures are lossless: every plane is the full-size 64x64 source plane, each
        // sample scaled up to 4x4.
        for plane in 0..3 {
            for y in 0..256 {
                // SAFETY: a 256x256 4:4:4 picture's planes hold 256 bytes per row.
                let row = unsafe { std::slice::from_raw_parts(info.data[plane].add(y * info.linesize[plane] as usize), 256) };
                let expected: Vec<u8> = (0..256).map(|x| source[(plane * 64 + y / 4) * 64 + x / 4]).collect();
                assert_eq!(row, &expected[..], "{codec:?} plane {plane} row {y}");
            }
        }
    }
    // 10-bit 4:4:4: every sample is the fixture's code, in the low bits of its word.
    let mut decoder = VideoDecoder::open(Codec::Hevc, Hardware::None, 1, false).unwrap();
    assert_eq!(decoder.send(include_bytes!("../../tests/fixtures/hevc-444-10.bin"), 7), 0);
    let Received::Frame(frame) = decoder.receive() else { panic!("HEVC 4:4:4 10-bit: no decoded frame") };
    let info = frame.info();
    assert_eq!((info.kind, info.ten_bit, info.width, info.height), (FRAME_YUV444P10, 1, 256, 256));
    for plane in 0..3 {
        for y in 0..256 {
            // SAFETY: a 256x256 10-bit 4:4:4 picture's planes hold 256 words per row.
            let row = unsafe { std::slice::from_raw_parts(info.data[plane].add(y * info.linesize[plane] as usize).cast::<u16>(), 256) };
            let expected: Vec<u16> = (0..256).map(|x| hevc_444_10_fixture_code(plane, x, y)).collect();
            assert_eq!(row, &expected[..], "10-bit plane {plane} row {y}");
        }
    }
    for (codec, bytes) in [
        (Codec::Hevc, include_bytes!("../../../../app/src/main/assets/packed444/hevc-10.bin").as_slice()),
        (Codec::Av1, include_bytes!("../../../../app/src/main/assets/packed444/av1-10.bin").as_slice()),
    ] {
        let mut decoder = VideoDecoder::open(codec, Hardware::None, 1, false).unwrap();
        assert_eq!(decoder.send(bytes, 7), 0);
        let Received::Frame(frame) = decoder.receive() else { panic!("{codec:?}: no decoded frame") };
        let info = frame.info();
        assert_eq!((info.kind, info.ten_bit), (FRAME_YUV420P10, 1), "{codec:?} Main10 in software");
    }
}

#[test]
fn packet_buffers_come_from_one_pool_with_zeroed_padding_until_a_frame_outgrows_it() {
    let fixture =
        include_bytes!("../../../../app/src/main/assets/packed444/hevc-8.bin").as_slice();
    let padding = AV_INPUT_BUFFER_PADDING_SIZE as usize;
    let mut decoder = VideoDecoder::open(Codec::Hevc, Hardware::None, 1, false).unwrap();
    assert_eq!(decoder.send(fixture, 1), 0);
    let Received::Frame(frame) = decoder.receive() else {
        panic!("no decoded frame")
    };
    assert_eq!(frame.info().pts, 1);
    let (pool, size) = (decoder.pool, decoder.pool_size);
    assert!(!pool.is_null() && size >= fixture.len() + padding && size % (64 * 1024) == 0);
    // A shorter packet takes a buffer from the same pool (the one the whole frame was in, once
    // the decoder let go of it); the bytes after it are padding again, not the rest of that frame.
    let short = &fixture[..fixture.len() / 2];
    assert!(fixture[short.len()..short.len() + padding]
        .iter()
        .any(|&b| b != 0));
    decoder.send(short, 2);
    assert_eq!((decoder.pool, decoder.pool_size), (pool, size));
    unsafe {
        let packet = &*decoder.packet;
        assert_eq!(packet.size as usize, short.len());
        assert_eq!(std::slice::from_raw_parts(packet.data, short.len()), short);
        assert!(
            std::slice::from_raw_parts(packet.data.add(short.len()), padding)
                .iter()
                .all(|&b| b == 0)
        );
    }
    // A packet that does not fit makes a larger pool.
    decoder.send(&vec![0u8; size], 3);
    assert!(
        decoder.pool_size >= size + padding
            && decoder.pool_size == packet_buffer_size(size + padding)
    );
    // Frame after frame from the pool decodes with its own timestamp.
    let mut decoder = VideoDecoder::open(Codec::Hevc, Hardware::None, 1, false).unwrap();
    for pts in 10..13 {
        assert_eq!(decoder.send(fixture, pts), 0);
        let Received::Frame(frame) = decoder.receive() else {
            panic!("no decoded frame")
        };
        assert_eq!(frame.info().pts, pts);
    }
}

/// A packet goes to FFmpeg by reference only when it lies in the lent frame's bytes with zero
/// padding after it; the buffer's free callback gives the reference back.
#[test]
fn a_packet_is_lent_only_from_the_frames_bytes_with_padding_after_it() {
    let padding = AV_INPUT_BUFFER_PADDING_SIZE as usize;
    let mut frame = vec![7u8; 100];
    frame.extend_from_slice(&[0; 20]);
    let bytes = FrameBytes::from(frame);
    let data = bytes.as_slice();
    // The whole frame, and the frame without its trailing zeros (`access_unit`).
    for packet in [data, &data[..100]] {
        let mut buffer = lend_packet(&bytes, packet, padding).expect("lent");
        assert!(!bytes.unshared(), "the buffer holds a reference");
        unsafe {
            assert_eq!((*buffer).data.cast_const(), data.as_ptr());
            assert_eq!((*buffer).size as usize, data.len() + FRAME_PADDING);
            av_buffer_unref(&mut buffer);
        }
        assert!(bytes.unshared(), "freeing the buffer gave the reference back");
    }
    // Bytes elsewhere, a part with picture data after it, and an empty packet are copied.
    let elsewhere = data.to_vec();
    assert!(lend_packet(&bytes, &elsewhere, padding).is_none());
    assert!(lend_packet(&bytes, &data[..50], padding).is_none());
    assert!(lend_packet(&bytes, &data[..0], padding).is_none());
    // More padding than the frame carries is not claimed.
    assert!(lend_packet(&bytes, data, FRAME_PADDING + 1).is_none());
    assert!(bytes.unshared());
}

/// A lent frame decodes as a copied one does, FFmpeg lets go of its bytes by the time the frame
/// is done, and a packet that is not the lent frame's (a probe, a stale lend) is copied.
#[test]
fn a_lent_frame_decodes_without_a_copy_and_comes_back() {
    let fixture =
        include_bytes!("../../../../app/src/main/assets/packed444/hevc-8.bin").as_slice();
    let mut padded = fixture.to_vec();
    // The last shard's zero padding, which `access_unit` trims off.
    padded.extend_from_slice(&[0; 300]);
    let mut decoder = VideoDecoder::open(Codec::Hevc, Hardware::None, 1, false).unwrap();
    for pts in 20..23 {
        let bytes = FrameBytes::from(padded.clone());
        decoder.lend(Some(bytes.clone()));
        assert_eq!(decoder.send(bytes.as_slice(), pts), 0);
        let Received::Frame(frame) = decoder.receive() else {
            panic!("no decoded frame")
        };
        assert_eq!(frame.info().pts, pts);
        assert!(matches!(decoder.receive(), Received::None));
        decoder.lend(None);
        assert!(bytes.unshared(), "FFmpeg let go of the frame's bytes after frame {pts}");
    }
    assert_eq!(decoder.lent_packets(), 3);
    assert!(decoder.pool.is_null(), "no packet was copied");
    // Not the lent bytes: copied into the pool.
    decoder.lend(Some(FrameBytes::from(padded.clone())));
    assert_eq!(decoder.send(fixture, 30), 0);
    assert_eq!(decoder.lent_packets(), 3);
    assert!(!decoder.pool.is_null());
    let Received::Frame(frame) = decoder.receive() else {
        panic!("no decoded frame")
    };
    assert_eq!(frame.info().pts, 30);
    // Bytes FFmpeg still holds when the decoder closes go with it.
    let bytes = FrameBytes::from(padded);
    decoder.lend(Some(bytes.clone()));
    decoder.send(bytes.as_slice(), 31);
    drop(frame);
    drop(decoder);
    assert!(bytes.unshared(), "closing the decoder released every reference");
}

#[test]
fn the_frame_of_a_receive_without_a_picture_holds_the_next_picture() {
    let fixture =
        include_bytes!("../../../../app/src/main/assets/packed444/hevc-8.bin").as_slice();
    let mut decoder = VideoDecoder::open(Codec::Hevc, Hardware::None, 1, false).unwrap();
    for pts in 20..23 {
        assert!(matches!(decoder.receive(), Received::None));
        let spare = decoder.spare.as_ref().expect("the empty frame is kept").raw;
        assert_eq!(decoder.send(fixture, pts), 0);
        let Received::Frame(frame) = decoder.receive() else {
            panic!("no decoded frame")
        };
        assert_eq!(frame.raw, spare, "the picture is decoded into the kept frame");
        assert_eq!(frame.info().pts, pts);
        assert!(decoder.spare.is_none(), "the picture left with its frame");
    }
}

/// The fixtures' RPUs are the Rubylight host's (`rust/core/src/dovi.rs`, profiles 8.1 and
/// 10.1): L1 maximum 3079 (1000 nits), source display 0.005 to 4000 nits.
#[cfg(windows)]
#[test]
fn a_dolby_vision_rpu_reaches_the_frame_as_its_peak() {
    let hevc = include_bytes!("../../../../app/src/main/assets/packed444/hevc-10.bin").as_slice();
    let av1 = include_bytes!("../../../../app/src/main/assets/packed444/av1-10.bin").as_slice();
    // Profile 8.1: the RPU NAL unit ends the access unit. Profile 10.1: the metadata OBU
    // follows the temporal delimiter.
    let mut hevc_dv = hevc.to_vec();
    hevc_dv.extend_from_slice(include_bytes!("../../tests/fixtures/dovi-81-rpu.bin"));
    assert_eq!(&av1[..2], &[0x12, 0], "the AV1 fixture starts with a temporal delimiter");
    let av1_dv = [&av1[..2], include_bytes!("../../tests/fixtures/dovi-101-rpu.bin"), &av1[2..]].concat();
    for (codec, plain, with_rpu) in [(Codec::Hevc, hevc, hevc_dv.as_slice()), (Codec::Av1, av1, av1_dv.as_slice())] {
        let decode = |bytes: &[u8]| {
            let mut decoder = VideoDecoder::open(codec, Hardware::None, 1, false).unwrap();
            assert_eq!(decoder.send(bytes, 1), 0, "{codec:?}");
            let Received::Frame(frame) = decoder.receive() else { panic!("{codec:?}: no decoded frame") };
            frame.dolby_vision()
        };
        assert_eq!(decode(plain), None, "{codec:?} without an RPU");
        let rpu = decode(with_rpu).unwrap_or_else(|| panic!("{codec:?}: no Dolby Vision metadata"));
        assert!(!rpu.ipt, "{codec:?}: an HDR10 base layer");
        let peak = rpu.peak.unwrap();
        assert!((peak - 1000.0).abs() < 2.0, "{codec:?}: L1's 1000 nits, not the source's 4000: {peak}");
    }
}

#[cfg(windows)]
#[test]
fn a_dolby_vision_peak_comes_from_l1_else_the_source_display_and_profile_5_is_ipt() {
    let peak = |l1, source| DolbyVision::of(1, 0, l1, source).peak;
    assert!((peak(Some(3079), 3696).unwrap() - 1000.0).abs() < 2.0);
    assert!((peak(None, 3696).unwrap() - 4000.0).abs() < 10.0, "no L1: the source display's maximum");
    assert!((peak(Some(0), 3079).unwrap() - 1000.0).abs() < 2.0, "an empty L1 is no peak");
    assert_eq!(peak(None, 0), None);
    assert!((peak(Some(u16::MAX), 0).unwrap() - 10000.0).abs() < 1.0, "codes past 12 bits are the top");
    // ff_dovi_guess_profile_hevc: RPU profile 0 with a full-range base layer is profile 5.
    assert!(DolbyVision::of(0, 1, Some(3079), 0).ipt);
    assert!(!DolbyVision::of(1, 1, Some(3079), 0).ipt && !DolbyVision::of(0, 0, Some(3079), 0).ipt);
}

/// FFmpeg takes a hardware decoder to software without an error (HEVC Main10 on a GPU without
/// it): a picture that is not the backend's says so for every backend, not only Vulkan.
#[test]
fn a_picture_in_memory_from_any_hardware_decoder_says_it_was_refused() {
    for hardware in [AV_PIX_FMT_D3D11, AV_PIX_FMT_VAAPI, AV_PIX_FMT_VULKAN, AV_PIX_FMT_CUDA] {
        assert!(!refused_by(hardware, hardware));
        assert!(refused_by(hardware, AV_PIX_FMT_NV12));
        assert!(refused_by(hardware, AV_PIX_FMT_YUV420P10LE));
    }
    // Software and V4L2 M2M decoders have no hardware format to be refused.
    assert!(!refused_by(AV_PIX_FMT_NONE, AV_PIX_FMT_NV12));
    let device = ptr::dangling_mut::<c_void>();
    assert_eq!(refused_name("hevc d3d11va", Hardware::D3d11(device)), "hevc (software, D3D11VA refused)");
    assert_eq!(refused_name("av1 vaapi", Hardware::Vaapi), "av1 (software, VAAPI refused)");
    assert_eq!(refused_name("hevc vulkan", Hardware::Vulkan(device)), "hevc (software, Vulkan refused)");
    assert_eq!(refused_name("h264 nvdec", Hardware::Cuda), "h264 (software, NVDEC refused)");
}

/// A software decoder is synchronous and never refused; a clean picture is not damaged.
#[test]
fn a_software_decoder_is_synchronous_and_its_clean_pictures_are_not_damaged() {
    let mut decoder = VideoDecoder::open(Codec::Hevc, Hardware::None, 1, false).unwrap();
    assert!(!decoder.asynchronous() && !decoder.refused() && !decoder.pooled());
    assert!(decoder.send(include_bytes!("../../../../app/src/main/assets/packed444/hevc-8.bin"), 7 << 32) >= 0);
    let Received::Frame(frame) = decoder.receive() else { panic!("no decoded frame") };
    assert!(!frame.corrupt());
    assert_eq!(frame.info().pts, 7 << 32);
    assert!(!decoder.refused() && decoder.renamed().is_none());
}

#[test]
fn packet_buffers_leave_room_to_grow_in_whole_64_kib() {
    assert_eq!(packet_buffer_size(1), 64 * 1024);
    assert_eq!(packet_buffer_size(64 * 1024), 128 * 1024);
    assert_eq!(packet_buffer_size(1 << 20), (1 << 20) + (256 << 10));
    for needed in [100, 65_535, 300_000, 5_000_000] {
        let size = packet_buffer_size(needed);
        assert!(size >= needed + needed / 4 && size.is_multiple_of(64 * 1024));
    }
}

#[test]
fn linux_tries_vaapi_then_vulkan_then_v4l2_m2m_then_nvdec_and_other_choices_alone() {
    use crate::vulkan_video::Choice;
    let device = ptr::dangling_mut::<c_void>();
    let (vaapi, vulkan, v4l2, nvdec) = (Hardware::Vaapi, Hardware::Vulkan(device), Hardware::V4l2m2m, Hardware::Cuda);
    let mesa = crate::linux_video::vaapi_plan(Ok("Mesa Gallium driver 24.2.8 for AMD Radeon 780M (radeonsi, gfx1103_r1, LLVM 19.1.1, DRM 3.57, 6.8.0)"), false);
    let order = |gpu, choice, gpu_first| Hardware::linux_order(gpu, choice, gpu_first, &mesa).0;
    // With "Vulkan video first" Vulkan video goes before VAAPI on the GPU renderer's device;
    // by default VAAPI stays first, its pictures mapped into the renderer without a copy.
    assert_eq!(order(device, Choice::Default, true), [vulkan, vaapi, v4l2, nvdec]);
    assert_eq!(order(device, Choice::Default, false), [vaapi, vulkan, v4l2, nvdec], "the default");
    let other_renderer = Hardware::Vulkan(ptr::null_mut());
    assert_eq!(order(ptr::null_mut(), Choice::Default, true), [vaapi, other_renderer, v4l2, nvdec], "without the GPU renderer VAAPI stays first");
    for gpu_first in [true, false] {
        assert_eq!(order(device, Choice::First, gpu_first), [vulkan, vaapi, v4l2, nvdec], "NIGHTJAR_VULKAN_VIDEO=1");
        assert_eq!(order(device, Choice::Off, gpu_first), [vaapi, v4l2, nvdec], "NIGHTJAR_VULKAN_VIDEO=0");
    }
    assert_eq!(Hardware::linux_order(device, Choice::Default, false, &mesa).1, "VA-API first, its pictures mapped into the renderer's Vulkan images");
    // The setting, off by default, unless the environment says on or off.
    assert!(!crate::config::Settings::default().vulkan_video_first);
    assert!(!crate::vulkan_video::gpu_first(None, false));
    assert!(crate::vulkan_video::gpu_first(None, true));
    assert!(crate::vulkan_video::gpu_first(Some(true), false));
    assert!(!crate::vulkan_video::gpu_first(Some(false), true));
    assert_eq!(Hardware::Vulkan(device).label(), "Vulkan");
    assert!(Hardware::None.order().is_empty());
    assert_eq!(Hardware::D3d11(device).order(), [Hardware::D3d11(device)]);
    assert_eq!(Hardware::Vaapi.order(), [Hardware::Vaapi]);
    assert_eq!(Hardware::Cuda.order(), [Hardware::Cuda]);
}

#[cfg(target_os = "linux")]
#[test]
fn decode_stages_bill_no_poll_to_a_picture_and_drop_a_failed_one() {
    let mut stages = Stages::new(DECODE_STAGES);
    let long_ago = std::time::Instant::now().checked_sub(std::time::Duration::from_millis(50)).unwrap();
    // Polls that find no picture (EAGAIN) and a failed hand-off add nothing to the next picture.
    time_receive(&mut stages, &Received::None, long_ago, None);
    stages.add(0, long_ago);
    time_receive(&mut stages, &Received::Error, long_ago, Some(long_ago));
    assert_eq!(stages.frames(), 0, "a failed picture is not kept");
    let now = std::time::Instant::now();
    let frame = Frame::new(NonNull::new(unsafe { av_frame_alloc() }).unwrap());
    time_receive(&mut stages, &Received::Frame(frame), now, Some(now));
    assert_eq!(stages.frames(), 1);
    let summary = stages.summary().unwrap();
    assert!(summary.contains("send 0.00/0.00, receive 0.00/0.00"), "{summary}");
}

#[cfg(target_os = "linux")]
#[test]
fn decode_stages_are_summed_up_for_a_stream_not_a_probe() {
    let mut stages = Stages::new(DECODE_STAGES);
    stages.commit();
    assert!(stage_summary(&stages).is_none(), "a probe's picture says nothing");
    for _ in 1..crate::stream::stage_times::SUMMARY_FRAMES {
        stages.commit();
    }
    let summary = stage_summary(&stages).unwrap();
    assert!(summary.ends_with("send 0.00/0.00, receive 0.00/0.00, hand-off 0.00/0.00"), "{summary}");
    let software = VideoDecoder::open(Codec::H264, Hardware::None, 1, false).unwrap();
    assert!(software.stages.is_none(), "software decoding is not timed");
}

/// NVIDIA's VA-API wrapper (nvidia-vaapi-driver) and the VDPAU backend leave VAAPI out, so
/// Vulkan video comes first and NVDEC after it, unless NIGHTJAR_FORCE_VAAPI=1; i965 is noted.
#[test]
fn wrapper_va_drivers_leave_vaapi_out_of_the_linux_order_and_say_why() {
    use crate::linux_video::vaapi_plan;
    use crate::vulkan_video::Choice;
    let device = ptr::dangling_mut::<c_void>();
    let nvidia = vaapi_plan(Ok("VA-API NVDEC driver [direct backend]"), false);
    let (order, why) = Hardware::linux_order(device, Choice::Default, false, &nvidia);
    assert_eq!(order, [Hardware::Vulkan(device), Hardware::V4l2m2m, Hardware::Cuda]);
    assert_eq!(why, "NVIDIA: VA-API is the nvidia-vaapi-driver wrapper; NIGHTJAR_FORCE_VAAPI=1 tries it");
    assert_eq!(
        crate::linux_video::order_line(&order.iter().map(|backend| backend.label()).collect::<Vec<_>>(), &why),
        "decoder order: Vulkan, V4L2 M2M, NVDEC (NVIDIA: VA-API is the nvidia-vaapi-driver wrapper; NIGHTJAR_FORCE_VAAPI=1 tries it)"
    );
    let vdpau = vaapi_plan(Ok("Splitted-Desktop Systems VDPAU backend for VA-API - 0.7.4"), false);
    assert_eq!(Hardware::linux_order(device, Choice::First, false, &vdpau).0, [Hardware::Vulkan(device), Hardware::V4l2m2m, Hardware::Cuda]);
    let forced = vaapi_plan(Ok("VA-API NVDEC driver [direct backend]"), true);
    assert_eq!(Hardware::linux_order(device, Choice::Default, false, &forced).0, [Hardware::Vaapi, Hardware::Vulkan(device), Hardware::V4l2m2m, Hardware::Cuda]);
    let i965 = vaapi_plan(Ok("Intel i965 driver for Intel(R) Coffee Lake - 2.4.1"), false);
    let (order, why) = Hardware::linux_order(ptr::null_mut(), Choice::Default, false, &i965);
    assert_eq!(order[0], Hardware::Vaapi, "i965 still decodes");
    assert!(why.starts_with("VA-API first; Vulkan video needs the GPU renderer; Intel i965"), "{why}");
}

/// Without the GPU renderer's device Vulkan video does not open and says why, so the next
/// backend is tried; packed 4:4:4 is no reason (its planes are sampled, #80).
#[test]
fn vulkan_video_without_the_renderers_device_names_the_reason_and_packed_chroma_is_none() {
    let error = VideoDecoder::open_for(Codec::Hevc, Pictures::default(), Hardware::Vulkan(ptr::null_mut()), 1, false).err().unwrap();
    if cfg!(target_os = "linux") {
        assert!(error.contains("does not draw with SDL's GPU API") || error.contains("libavutil"), "{error}");
        let packed = Pictures { packed: true, ..Pictures::default() };
        let error = VideoDecoder::open_for(Codec::Hevc, packed, Hardware::Vulkan(ptr::null_mut()), 1, false).err().unwrap();
        assert!(!error.contains("packed 4:4:4"), "{error}");
    } else {
        assert!(error.contains("only used on Linux") || error.contains("no Vulkan support"), "{error}");
    }
    let error = VideoDecoder::open(Codec::H264, Hardware::Auto(ptr::null_mut(), None), 1, false).err().unwrap();
    assert!(error.contains("not a backend"), "{error}");
}

#[test]
fn the_first_backend_that_opens_is_used_and_each_one_before_it_is_logged_once() {
    let mut logged = Vec::new();
    let mut tried = Vec::new();
    let order = vec![Hardware::Vaapi, Hardware::V4l2m2m, Hardware::Cuda];
    let opened = first_opened(
        order.clone(),
        |backend| {
            tried.push(backend);
            if backend == Hardware::V4l2m2m { Ok("v4l2") } else { Err(format!("no {}", backend.label())) }
        },
        |backend, error| logged.push(format!("{} {error}", backend.label())),
    );
    assert_eq!(opened, Ok("v4l2"));
    assert_eq!(tried, [Hardware::Vaapi, Hardware::V4l2m2m], "NVDEC is not tried once one opened");
    assert_eq!(logged, ["VAAPI no VAAPI"]);

    logged.clear();
    let failed: Result<(), String> = first_opened(order, |_| Err("gone".into()), |backend, _| logged.push(backend.label().into()));
    assert_eq!(failed.unwrap_err(), "VAAPI: gone; V4L2 M2M: gone; NVDEC: gone");
    assert_eq!(logged, ["VAAPI", "V4L2 M2M", "NVDEC"]);
    let none: Result<(), String> = first_opened(Vec::new(), |_| Ok(()), |_, _| panic!("nothing was tried"));
    assert!(none.is_err());
}

#[test]
fn a_held_reference_keeps_the_picture_and_a_software_picture_does_not_map() {
    let fixture = include_bytes!("../../../../app/src/main/assets/packed444/hevc-8.bin").as_slice();
    let mut decoder = VideoDecoder::open(Codec::Hevc, Hardware::None, 1, false).unwrap();
    assert_eq!(decoder.hardware, Hardware::None);
    assert!(!decoder.export_dmabuf(), "only a VAAPI decoder exports");
    assert_eq!(decoder.send(fixture, 42), 0);
    let Received::Frame(frame) = decoder.receive() else { panic!("no decoded frame") };
    let held = frame.reference().expect("a reference");
    let info = frame.info();
    drop(frame);
    let kept = held.info();
    assert_eq!((kept.pts, kept.width, kept.data[0]), (42, info.width, info.data[0]), "the reference is the same picture");
    assert!(held.map_to_memory().is_none(), "only hardware pictures map");
}

#[test]
fn invalid_opus_mapping_is_rejected_before_ffi() {
    assert!(OpusDecoder::new(13, 1, 0, &[0; 13]).is_err());
    assert!(OpusDecoder::new(12, 8, 4, &[0; 11]).is_err());
    assert!(OpusDecoder::new(2, 1, 1, &[0]).is_err());
}

/// 7.1.4 in Foundation Sunshine's layouts opens a decoder.
#[test]
fn seven_one_four_opens_an_opus_decoder() {
    use rubylight_protocol::audio_ext::{LAYOUT_714, LAYOUT_714_HQ};
    for layout in [LAYOUT_714, LAYOUT_714_HQ] {
        let decoder =
            OpusDecoder::new(12, layout.streams, layout.coupled, &layout.mapping).unwrap();
        assert_eq!(decoder.channels, 12);
    }
}

/// Each picture format reaches the presenter as the kind it draws, hardware ones by the
/// format of their frames context, and anything else as the kind nothing shows.
#[test]
fn each_picture_format_has_the_kind_the_presenter_draws() {
    assert_eq!(frame_kind(AV_PIX_FMT_NV12, None, false), (FRAME_NV12, 0));
    assert_eq!(frame_kind(AV_PIX_FMT_P010LE, None, false), (FRAME_P010, 1));
    assert_eq!(frame_kind(AV_PIX_FMT_YUVJ420P, None, false), (FRAME_YUV420P, 0));
    assert_eq!(frame_kind(AV_PIX_FMT_YUV444P, None, false), (FRAME_YUV444P, 0));
    assert_eq!(frame_kind(AV_PIX_FMT_YUV420P10LE, None, false), (FRAME_YUV420P10, 1));
    assert_eq!(frame_kind(AV_PIX_FMT_YUV444P10LE, None, false), (FRAME_YUV444P10, 1));
    // NVDEC's 10-bit 4:4:4 in memory, high-bit samples.
    assert_eq!(frame_kind(AV_PIX_FMT_YUV444P16LE, None, false), (FRAME_YUV444P16, 1));
    // D3D11: NV12 without a frames context; the 4:4:4 surfaces AYUV (VUYX), Y410 (XV30) and
    // Y416 (XV36, XV48) are drawn from a copy; other formats by nothing.
    assert_eq!(frame_kind(AV_PIX_FMT_D3D11, None, false), (FRAME_D3D11, 0));
    assert_eq!(frame_kind(AV_PIX_FMT_D3D11, Some(AV_PIX_FMT_P010LE), false), (FRAME_D3D11, 1));
    assert_eq!(frame_kind(AV_PIX_FMT_D3D11, Some(AV_PIX_FMT_VUYX), false), (FRAME_D3D11_444, 0));
    assert_eq!(frame_kind(AV_PIX_FMT_D3D11, Some(AV_PIX_FMT_VUYA), false), (FRAME_D3D11_444, 0));
    assert_eq!(frame_kind(AV_PIX_FMT_D3D11, Some(AV_PIX_FMT_XV30LE), false), (FRAME_D3D11_444, 1));
    assert_eq!(frame_kind(AV_PIX_FMT_D3D11, Some(AV_PIX_FMT_XV36LE), false), (FRAME_D3D11_444, 1));
    #[cfg(windows)]
    assert_eq!(frame_kind(AV_PIX_FMT_D3D11, Some(AV_PIX_FMT_XV48LE), false), (FRAME_D3D11_444, 1));
    assert_eq!(frame_kind(AV_PIX_FMT_D3D11, Some(AV_PIX_FMT_YUV444P), false), (4, 0));
    assert_eq!(frame_kind(AV_PIX_FMT_D3D11, Some(AV_PIX_FMT_Y210LE), false), (4, 0), "4:2:2 is not streamed");
    assert_eq!(frame_kind(AV_PIX_FMT_RGBA, None, false), (4, 0));
    #[cfg(unix)]
    {
        assert_eq!(frame_kind(AV_PIX_FMT_VULKAN, Some(AV_PIX_FMT_NV12), false), (FRAME_VULKAN, 0));
        assert_eq!(frame_kind(AV_PIX_FMT_VULKAN, Some(AV_PIX_FMT_P010LE), false), (FRAME_VULKAN, 1));
        assert_eq!(frame_kind(AV_PIX_FMT_VULKAN, Some(AV_PIX_FMT_NV24), false), (FRAME_VULKAN444, 0));
        assert_eq!(frame_kind(AV_PIX_FMT_VULKAN, Some(AV_PIX_FMT_P410LE), false), (FRAME_VULKAN444, 1));
        // Mapped 4:4:4 VAAPI pictures, one interleaved image.
        assert_eq!(frame_kind(AV_PIX_FMT_VULKAN, Some(AV_PIX_FMT_RGBA), false), (FRAME_VULKAN_INTERLEAVED444, 0));
        assert_eq!(frame_kind(AV_PIX_FMT_VULKAN, Some(AV_PIX_FMT_XV30LE), false), (FRAME_VULKAN_INTERLEAVED444, 1));
        assert_eq!(frame_kind(AV_PIX_FMT_VULKAN, None, false), (4, 0), "no frames context");
        assert_eq!(frame_kind(AV_PIX_FMT_VULKAN, Some(AV_PIX_FMT_YUV420P), false), (4, 0), "not planes Planes copies");
        // VAAPI only as exported DMA-BUFs; otherwise it is mapped to memory before this.
        assert_eq!(frame_kind(AV_PIX_FMT_VAAPI, Some(AV_PIX_FMT_P010LE), true), (FRAME_DMABUF, 1));
        assert_eq!(frame_kind(AV_PIX_FMT_VAAPI, Some(AV_PIX_FMT_NV12), true), (FRAME_DMABUF, 0));
        assert_eq!(frame_kind(AV_PIX_FMT_VAAPI, None, true), (FRAME_DMABUF, 0));
        assert_eq!(frame_kind(AV_PIX_FMT_VAAPI, Some(AV_PIX_FMT_NV12), false), (4, 0));
    }
}

/// A failed CUDA copy drops the map for good, keeps why for the toast and sends the picture
/// through memory; a good one keeps the map, and without one nothing is tried.
#[test]
fn a_failed_copy_drops_the_map_and_says_why_once() {
    let (mut map, mut fallback) = (Some(0u32), None);
    assert!(copy_or_drop_map(&mut map, &mut fallback, |copies| { *copies += 1; Ok(()) }));
    assert_eq!((map, fallback.as_deref()), (Some(1), None));
    assert!(!copy_or_drop_map(&mut map, &mut fallback, |_| Err("CUDA error 999".into())));
    assert_eq!((map, fallback.as_deref()), (None, Some("CUDA error 999")));
    let mut tried = false;
    assert!(!copy_or_drop_map(&mut map, &mut fallback, |_: &mut u32| { tried = true; Ok(()) }));
    assert!(!tried, "no map, no copy: the picture goes through memory");
}

/// Nightjar #115: SDL let go of its Direct3D 11 device (a driver reset) while a launch still
/// had the old pointer, and FFmpeg's AddRef ran on freed memory. A launch's reference keeps the
/// device alive past its maker's release, and tells it from the device made in its place.
#[cfg(windows)]
#[test]
fn a_held_device_outlives_the_renderers_release() {
    use windows::core::Interface;
    use windows::Win32::Foundation::HMODULE;
    use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL_11_0};
    use windows::Win32::Graphics::Direct3D11::{D3D11CreateDevice, ID3D11Device, D3D11_CREATE_DEVICE_FLAG, D3D11_SDK_VERSION};
    let make = || {
        let mut device = None;
        // SAFETY: A WARP device of this test's own.
        unsafe {
            D3D11CreateDevice(None, D3D_DRIVER_TYPE_WARP, HMODULE::default(), D3D11_CREATE_DEVICE_FLAG(0), Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION, Some(&mut device), None, None).unwrap();
        }
        device.unwrap()
    };
    // The references on the device at `raw`, counted by an AddRef and its Release.
    let references = |raw: *mut c_void| {
        // SAFETY: A balanced AddRef and Release on a live device.
        unsafe {
            let device = ID3D11Device::from_raw_borrowed(&raw).unwrap();
            (device.vtable().base__.AddRef)(raw);
            (device.vtable().base__.Release)(raw)
        }
    };
    let renderers = make();
    let raw = renderers.as_raw();
    let before = references(raw);
    let held = DeviceRef::of(Hardware::D3d11(raw));
    assert_eq!(references(raw), before + 1, "the launch holds one reference");
    assert!(held.holds(raw));
    // The renderer lets go of its device and makes another, as SDL does after a driver reset.
    drop(renderers);
    let again = make();
    assert_eq!(references(raw), before, "the launch's reference keeps the old device alive");
    assert!(!held.holds(again.as_raw()), "the launch sees the device changed");
    assert!(!held.holds(std::ptr::null_mut()));
    let copy = held.clone();
    drop(held);
    assert_eq!(references(raw), before, "the decoder's copy keeps it alive after the plan goes");
    assert!(copy.holds(raw));
    assert!(DeviceRef::of(Hardware::None).holds(std::ptr::null_mut()));
    assert!(DeviceRef::of_device(std::ptr::null_mut()).holds(std::ptr::null_mut()));
}
