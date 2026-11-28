//! Tests for Windows Direct3D 11 video decoding isolation, `ID3D10Multithread` locking,
//! `CopySubresourceRegion1(..., D3D11_COPY_DISCARD)`, and cross-device shared-texture handoff.

use super::*;
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP};

fn make_device(
    driver: windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE,
) -> windows::core::Result<(ID3D11Device, ID3D11DeviceContext)> {
    let mut device = None;
    let mut context = None;
    // SAFETY: Standalone D3D11 test device.
    unsafe {
        D3D11CreateDevice(
            None,
            driver,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&[D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0]),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )?;
    }
    Ok((device.unwrap(), context.unwrap()))
}

/// Tier 2.3: `CopySubresourceRegion1(..., D3D11_COPY_DISCARD)` copies pixel-exact NV12 data
/// into a non-shared `D3D11_USAGE_DEFAULT` texture on WARP.
#[test]
fn copy_subresource_discard_copies_nv12_pixels() {
    let (device, context) = make_device(D3D_DRIVER_TYPE_WARP).expect("WARP device");
    let make_nv12 = |usage, cpu, bind| -> ID3D11Texture2D {
        let mut tex = None;
        unsafe {
            device
                .CreateTexture2D(
                    &D3D11_TEXTURE2D_DESC {
                        Width: 16,
                        Height: 16,
                        MipLevels: 1,
                        ArraySize: 1,
                        Format: DXGI_FORMAT_NV12,
                        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                        Usage: usage,
                        BindFlags: bind,
                        CPUAccessFlags: cpu,
                        MiscFlags: 0,
                    },
                    None,
                    Some(&mut tex),
                )
                .unwrap();
        }
        tex.unwrap()
    };
    let src = make_nv12(D3D11_USAGE_DEFAULT, 0, D3D11_BIND_SHADER_RESOURCE.0 as u32);
    let dst = make_nv12(D3D11_USAGE_DEFAULT, 0, D3D11_BIND_SHADER_RESOURCE.0 as u32);
    let stage = make_nv12(D3D11_USAGE_STAGING, D3D11_CPU_ACCESS_READ.0 as u32, 0);
    let mut pixels = vec![0u8; 16 * 24];
    for (i, b) in pixels.iter_mut().enumerate() {
        *b = ((i * 37 + 13) & 0xFF) as u8;
    }
    let region = D3D11_BOX { left: 0, top: 0, front: 0, right: 16, bottom: 16, back: 1 };
    unsafe {
        context.UpdateSubresource(&src, 0, None, pixels.as_ptr().cast(), 16, 0);
        copy_subresource_discard(&context, None, &dst, &src, 0, &region);
        context.CopyResource(&stage, &dst);
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        context.Map(&stage, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).unwrap();
        for row in 0..24 {
            let got = std::slice::from_raw_parts(
                mapped.pData.cast::<u8>().add(row * mapped.RowPitch as usize),
                16,
            );
            assert_eq!(got, &pixels[row * 16..(row + 1) * 16], "row {row}");
        }
        context.Unmap(&stage, 0);
    }
}

/// Tier 2.1: hooking `AVD3D11VADeviceContext::lock`/`unlock` to `ID3D10Multithread` releases
/// every COM ref in `ctx->free` both when `av_hwdevice_ctx_init` fails (WARP lacks
/// `ID3D11VideoDevice`) and when it succeeds on a real hardware video device.
#[test]
fn av_d3d11_device_lock_uses_id3d10_multithread_and_releases_on_free() {
    use super::super::ffi::{
        av_buffer_unref, av_hwdevice_ctx_alloc, av_hwdevice_ctx_init, AVD3D11VADeviceContext,
        AVHWDeviceContext, AV_HWDEVICE_TYPE_D3D11VA,
    };
    let check_device = |device: ID3D11Device, expect_init_ok: bool| {
        let raw_dev = device.as_raw();
        let ref_count = || unsafe {
            let d = ID3D11Device::from_raw_borrowed(&raw_dev).unwrap();
            (d.vtable().base__.AddRef)(raw_dev);
            (d.vtable().base__.Release)(raw_dev)
        };
        let before = ref_count();
        unsafe {
            let mut hwref = av_hwdevice_ctx_alloc(AV_HWDEVICE_TYPE_D3D11VA);
            assert!(!hwref.is_null());
            let dev_ctx = (*hwref).data.cast::<AVHWDeviceContext>();
            let d3d = &mut *((*dev_ctx).hwctx.cast::<AVD3D11VADeviceContext>());
            d3d.device = device.clone().into_raw().cast();
            attach_device_lock(dev_ctx, &device).expect("attach lock");
            assert!(d3d.lock.is_some() && d3d.unlock.is_some() && !d3d.lock_ctx.is_null());
            let status = av_hwdevice_ctx_init(hwref);
            if expect_init_ok {
                assert!(status >= 0, "av_hwdevice_ctx_init failed: {status}");
                let mt: ID3D10Multithread = device.cast().unwrap();
                assert!(mt.GetMultithreadProtected().as_bool());
                (d3d.lock.unwrap())(d3d.lock_ctx);
                (d3d.unlock.unwrap())(d3d.lock_ctx);
            } else {
                assert!(status < 0, "WARP lacks ID3D11VideoDevice");
            }
            av_buffer_unref(&mut hwref);
            assert!(hwref.is_null());
        }
        assert_eq!(
            ref_count(),
            before,
            "device and ID3D10Multithread COM refs all released (expect_init_ok={expect_init_ok})"
        );
    };
    let (warp, _ctx) = make_device(D3D_DRIVER_TYPE_WARP).expect("WARP device");
    check_device(warp, false);
    if let Ok((renderer, _)) = make_device(D3D_DRIVER_TYPE_HARDWARE) {
        let (video, _) = create_video_device_for(&renderer).expect("hardware video device");
        check_device(video, true);
    }
}

/// Tier 2.3 + Tier 3.1 edge cases: `ProbeVideoDevice::for_renderer` rejects null and creates
/// an independent device on the renderer's exact adapter LUID.
#[test]
fn probe_video_device_isolates_from_renderer_and_rejects_null() {
    assert!(ProbeVideoDevice::for_renderer(ptr::null_mut()).is_err());
    let Ok((renderer, _)) = make_device(D3D_DRIVER_TYPE_HARDWARE) else {
        return;
    };
    let probe = ProbeVideoDevice::for_renderer(renderer.as_raw()).expect("probe video device");
    assert_ne!(probe.as_raw(), renderer.as_raw());
    let luid = |raw: *mut c_void| unsafe {
        let d = ID3D11Device::from_raw_borrowed(&raw).unwrap();
        let desc = d.cast::<IDXGIDevice>().unwrap().GetAdapter().unwrap().GetDesc().unwrap();
        (desc.AdapterLuid.LowPart, desc.AdapterLuid.HighPart)
    };
    assert_eq!(luid(probe.as_raw()), luid(renderer.as_raw()));
}

/// Tier 3.1 end-to-end: FFmpeg D3D11VA decodes real HEVC/AV1 8-bit and 10-bit fixtures on a
/// separate `VIDEO_SUPPORT` device, exports them via NT-handle shared textures + monitored
/// fences onto a plain `BGRA_SUPPORT` renderer device (matching SDL3), and both `Direct` and
/// `copy_to_d3d11` consume them cleanly on the renderer device.
#[test]
fn ffmpeg_d3d11va_decodes_on_separate_device_and_renders_on_renderer_device() {
    use super::super::{Codec, Hardware, Received, VideoDecoder};
    use crate::stream::direct::{Direct, Output, Picture, Tone, Transfer};

    let Ok((renderer, render_ctx)) = make_device(D3D_DRIVER_TYPE_HARDWARE) else {
        return;
    };
    let fixtures: [(Codec, bool, &[u8]); 4] = [
        (Codec::Hevc, false, include_bytes!("../../../tests/fixtures/hevc-420-8.bin")),
        (Codec::Hevc, true, include_bytes!("../../../tests/fixtures/hevc-420-10.bin")),
        (Codec::Av1, false, include_bytes!("../../../tests/fixtures/av1-420-8.bin")),
        (Codec::Av1, true, include_bytes!("../../../tests/fixtures/av1-420-10.bin")),
    ];
    let mut direct = unsafe { Direct::new(&renderer) }.expect("Direct pass on renderer");
    let mut rtv_tex = None;
    let mut rtv = None;
    unsafe {
        renderer
            .CreateTexture2D(
                &D3D11_TEXTURE2D_DESC {
                    Width: 256,
                    Height: 256,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                    SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
                    ..Default::default()
                },
                None,
                Some(&mut rtv_tex),
            )
            .unwrap();
        renderer
            .CreateRenderTargetView(rtv_tex.as_ref().unwrap(), None, Some(&mut rtv))
            .unwrap();
    }
    let rtv = rtv.unwrap();
    let mut decoded_any = 0usize;
    for (codec, ten_bit, bytes) in fixtures {
        let pictures = Pictures {
            ten_bit,
            chroma444: false,
            waiting: 2,
            width: 256,
            height: 256,
            ..Pictures::default()
        };
        let Ok(mut decoder) =
            VideoDecoder::open_for(codec, pictures, Hardware::D3d11(renderer.as_raw()), 1, true)
        else {
            continue;
        };
        assert!(
            decoder.owns_d3d11_device(),
            "{codec:?} ten_bit={ten_bit} should own separate video device"
        );
        if decoder.send(bytes, 1) < 0 {
            continue;
        }
        let Received::Frame(frame) = decoder.receive() else {
            continue;
        };
        if decoder.refused() {
            continue;
        }
        decoded_any += 1;
        let surface = frame.d3d11_surface().expect("bridged D3D11 surface");
        assert!(surface.readable && surface.slice == 0);
        let owner_raw = unsafe {
            ID3D11Texture2D::from_raw_borrowed(&surface.texture)
                .unwrap()
                .GetDevice()
                .unwrap()
                .as_raw()
        };
        assert_eq!(
            owner_raw,
            renderer.as_raw(),
            "exported surface must belong to renderer device"
        );
        // Drop decoder BEFORE rendering to prove `Frame` / `SharedFrame` keeps bridge & textures alive.
        let held = frame.reference().expect("frame reference");
        drop(frame);
        drop(decoder);
        held.wait_d3d11_ready().expect("wait_d3d11_ready");
        let pic = Picture {
            texture: surface.texture,
            slice: surface.slice,
            size: (256, 256),
            ten_bit,
            full_range: false,
            matrix: 1,
            transfer: Transfer::Sdr,
            centred_chroma: false,
        };
        unsafe {
            direct
                .draw(
                    &rtv,
                    [0.0, 0.0, 256.0, 256.0],
                    &pic,
                    Output { linear: false, color_scale: 1.0, headroom: 1.0 },
                    Tone { white_nits: 80.0, ..Tone::default() },
                    |submit| held.with_decoder_lock(submit),
                )
                .expect("Direct draw from bridged texture");
        }
        let mut static_tex = None;
        unsafe {
            renderer
                .CreateTexture2D(
                    &D3D11_TEXTURE2D_DESC {
                        Width: 256,
                        Height: 256,
                        MipLevels: 1,
                        ArraySize: 1,
                        Format: if ten_bit { DXGI_FORMAT_P010 } else { DXGI_FORMAT_NV12 },
                        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                        Usage: D3D11_USAGE_DEFAULT,
                        BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                        ..Default::default()
                    },
                    None,
                    Some(&mut static_tex),
                )
                .unwrap();
        }
        let static_tex = static_tex.unwrap();
        assert!(held.copy_to_d3d11(static_tex.as_raw(), 256, 256));
        unsafe { render_ctx.Flush() };
    }
    assert!(
        decoded_any > 0,
        "hardware GPU should decode at least one HEVC/AV1 fixture via D3D11VA"
    );
}

/// Tier 3.1: on a hardware GPU, `DecodeBridge` creates a separate `VIDEO_SUPPORT` device on
/// the same adapter, transfers pixels across devices through NT-handle shared textures +
/// monitored fences, and recycles slots cleanly across multiple frames.
#[test]
fn decode_bridge_transfers_nv12_and_p010_across_separate_devices() {
    let Ok((renderer, render_ctx)) = make_device(D3D_DRIVER_TYPE_HARDWARE) else {
        return;
    };
    for ten_bit in [false, true] {
        let pictures = Pictures { ten_bit, chroma444: false, waiting: 2, ..Pictures::default() };
        let bridge = DecodeBridge::new(&renderer, pictures).expect("DecodeBridge on hardware GPU");
        assert_ne!(
            bridge.decode_device().as_raw(),
            renderer.as_raw(),
            "decode device must be separate from SDL3 renderer device"
        );
        let luid = |dev: &ID3D11Device| unsafe {
            let d = dev.cast::<IDXGIDevice>().unwrap().GetAdapter().unwrap().GetDesc().unwrap();
            (d.AdapterLuid.LowPart, d.AdapterLuid.HighPart)
        };
        assert_eq!(luid(bridge.decode_device()), luid(&renderer));

        let format = if ten_bit { DXGI_FORMAT_P010 } else { DXGI_FORMAT_NV12 };
        let row_bytes = if ten_bit { 32 } else { 16 };
        let mut src_tex = None;
        let mut dst_tex = None;
        let mut stage_tex = None;
        unsafe {
            bridge
                .decode_device()
                .CreateTexture2D(
                    &D3D11_TEXTURE2D_DESC {
                        Width: 16,
                        Height: 16,
                        MipLevels: 1,
                        ArraySize: 2,
                        Format: format,
                        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                        Usage: D3D11_USAGE_DEFAULT,
                        BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                        ..Default::default()
                    },
                    None,
                    Some(&mut src_tex),
                )
                .unwrap();
            renderer
                .CreateTexture2D(
                    &D3D11_TEXTURE2D_DESC {
                        Width: 16,
                        Height: 16,
                        MipLevels: 1,
                        ArraySize: 1,
                        Format: format,
                        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                        Usage: D3D11_USAGE_DEFAULT,
                        BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                        ..Default::default()
                    },
                    None,
                    Some(&mut dst_tex),
                )
                .unwrap();
            renderer
                .CreateTexture2D(
                    &D3D11_TEXTURE2D_DESC {
                        Width: 16,
                        Height: 16,
                        MipLevels: 1,
                        ArraySize: 1,
                        Format: format,
                        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                        Usage: D3D11_USAGE_STAGING,
                        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                        ..Default::default()
                    },
                    None,
                    Some(&mut stage_tex),
                )
                .unwrap();
        }
        let (src_tex, dst_tex, stage_tex) =
            (src_tex.unwrap(), dst_tex.unwrap(), stage_tex.unwrap());
        for frame_idx in 0u8..8 {
            let slice = u32::from(frame_idx % 2);
            let mut pattern = vec![0u8; row_bytes * 24];
            for (i, b) in pattern.iter_mut().enumerate() {
                *b = frame_idx.wrapping_mul(29).wrapping_add((i & 0xFF) as u8);
            }
            let mut fake_frame: AVFrame = unsafe { std::mem::zeroed() };
            fake_frame.data[0] = src_tex.as_raw().cast();
            fake_frame.data[1] = slice as usize as *mut u8;
            unsafe {
                bridge.decode_context.UpdateSubresource(
                    &src_tex,
                    slice,
                    None,
                    pattern.as_ptr().cast(),
                    row_bytes as u32,
                    0,
                );
                let shared = bridge.export(&fake_frame).expect("export shared frame");
                assert!(shared.copy_to_renderer_texture(dst_tex.as_raw(), 16, 16));
                render_ctx.CopyResource(&stage_tex, &dst_tex);
                let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
                render_ctx.Map(&stage_tex, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).unwrap();
                for row in 0..24 {
                    let got = std::slice::from_raw_parts(
                        mapped.pData.cast::<u8>().add(row * mapped.RowPitch as usize),
                        row_bytes,
                    );
                    assert_eq!(
                        got,
                        &pattern[row * row_bytes..(row + 1) * row_bytes],
                        "ten_bit={ten_bit} frame {frame_idx} row {row}"
                    );
                }
                render_ctx.Unmap(&stage_tex, 0);
            }
        }
    }
}

#[test]
fn fence_completed_ready_rejects_u64_max_device_removed_sentinel() {
    assert_eq!(fence_completed_ready(0, 1), Ok(false));
    assert_eq!(fence_completed_ready(1, 1), Ok(true));
    assert_eq!(fence_completed_ready(5, 3), Ok(true));
    assert!(
        fence_completed_ready(u64::MAX, 1).is_err(),
        "D3D11 returns UINT64_MAX from GetCompletedValue() when a device is removed"
    );
}

#[test]
fn copy_helpers_reject_non_positive_dimensions() {
    let Ok((renderer, _)) = make_device(D3D_DRIVER_TYPE_HARDWARE) else {
        return;
    };
    let pictures = Pictures { ten_bit: false, chroma444: false, waiting: 1, ..Pictures::default() };
    let bridge = DecodeBridge::new(&renderer, pictures).expect("DecodeBridge");
    let mut src_tex = None;
    let mut dst_tex = None;
    unsafe {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: 16, Height: 16, MipLevels: 1, ArraySize: 1, Format: DXGI_FORMAT_NV12,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT, BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            ..Default::default()
        };
        bridge.decode_device().CreateTexture2D(&desc, None, Some(&mut src_tex)).unwrap();
        renderer.CreateTexture2D(&desc, None, Some(&mut dst_tex)).unwrap();
        let (src_tex, dst_tex) = (src_tex.unwrap(), dst_tex.unwrap());
        let mut fake_frame: AVFrame = std::mem::zeroed();
        fake_frame.data[0] = src_tex.as_raw().cast();
        let shared = bridge.export(&fake_frame).expect("export");
        assert!(!shared.copy_to_renderer_texture(dst_tex.as_raw(), 0, 16));
        assert!(!shared.copy_to_renderer_texture(dst_tex.as_raw(), 16, -4));
        assert!(!copy_shared_device_frame(&fake_frame, dst_tex.as_raw(), -2, 16));
    }
}

#[test]
fn decode_bridge_preflights_srvs_for_420_and_444_formats() {
    let Ok((renderer, _)) = make_device(D3D_DRIVER_TYPE_HARDWARE) else {
        return;
    };
    let (decode_device, _) = create_video_device_for(&renderer).expect("video device");
    let render_device1: ID3D11Device1 = renderer.cast().unwrap();
    for fmt in [DXGI_FORMAT_NV12, DXGI_FORMAT_P010] {
        preflight_format_and_srv(&decode_device, &renderer, &render_device1, fmt)
            .unwrap_or_else(|e| panic!("4:2:0 format {} failed preflight: {e}", fmt.0));
    }
    for fmt in [DXGI_FORMAT_AYUV, DXGI_FORMAT_Y410, DXGI_FORMAT_Y416] {
        let dec_support = unsafe { decode_device.CheckFormatSupport(fmt) }
            .is_ok_and(|f| f & D3D11_FORMAT_SUPPORT_DECODER_OUTPUT.0 as u32 != 0);
        let res = preflight_format_and_srv(&decode_device, &renderer, &render_device1, fmt);
        eprintln!("preflight 4:4:4 fmt={} decoder_output={dec_support} result={res:?}", fmt.0);
        if dec_support {
            res.unwrap_or_else(|e| panic!("decoder-supported 4:4:4 format {} failed preflight: {e}", fmt.0));
        }
    }
}

#[test]
fn decode_bridge_handles_resolution_change_and_skips_drop_flush_after_present() {
    let Ok((renderer, render_ctx)) = make_device(D3D_DRIVER_TYPE_HARDWARE) else {
        return;
    };
    let pictures = Pictures { ten_bit: false, chroma444: false, waiting: 2, ..Pictures::default() };
    let bridge = DecodeBridge::new(&renderer, pictures).expect("DecodeBridge");
    let make_nv12 = |dev: &ID3D11Device, w: u32, h: u32, usage: D3D11_USAGE, cpu: u32, bind: u32| -> ID3D11Texture2D {
        let mut tex = None;
        unsafe {
            dev.CreateTexture2D(
                &D3D11_TEXTURE2D_DESC {
                    Width: w, Height: h, MipLevels: 1, ArraySize: 1, Format: DXGI_FORMAT_NV12,
                    SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                    Usage: usage, BindFlags: bind, CPUAccessFlags: cpu, MiscFlags: 0,
                },
                None,
                Some(&mut tex),
            )
            .unwrap();
        }
        tex.unwrap()
    };

    // Frame 1 at 16x16: Present flushes the render context, so `mark_renderer_flushed()` avoids
    // a redundant `Flush()` inside `SlotLease::drop()`.
    let src16 = make_nv12(bridge.decode_device(), 16, 16, D3D11_USAGE_DEFAULT, 0, D3D11_BIND_SHADER_RESOURCE.0 as u32);
    let dst16 = make_nv12(&renderer, 16, 16, D3D11_USAGE_DEFAULT, 0, D3D11_BIND_SHADER_RESOURCE.0 as u32);
    unsafe {
        let mut f: AVFrame = std::mem::zeroed();
        f.data[0] = src16.as_raw().cast();
        let shared = bridge.export(&f).expect("export 16x16");
        assert!(shared.copy_to_renderer_texture(dst16.as_raw(), 16, 16));
        render_ctx.Flush();
        shared.mark_renderer_flushed();
        drop(shared);
    }
    assert_eq!(bridge.drop_flushes.load(Ordering::Relaxed), 0, "no redundant drop flush after Present");

    // Frame 2 switches resolution to 32x32 on the same DecodeBridge and drops WITHOUT marking
    // flushed, verifying both pool recreation and pixel-exact 32x32 transfer.
    drop(src16);
    let src32 = make_nv12(bridge.decode_device(), 32, 32, D3D11_USAGE_DEFAULT, 0, D3D11_BIND_SHADER_RESOURCE.0 as u32);
    let dst32 = make_nv12(&renderer, 32, 32, D3D11_USAGE_DEFAULT, 0, D3D11_BIND_SHADER_RESOURCE.0 as u32);
    let stage32 = make_nv12(&renderer, 32, 32, D3D11_USAGE_STAGING, D3D11_CPU_ACCESS_READ.0 as u32, 0);
    let pattern: Vec<u8> = (0..32 * 48).map(|i| ((i * 19 + 7) & 0xFF) as u8).collect();
    unsafe {
        bridge.decode_context.UpdateSubresource(&src32, 0, None, pattern.as_ptr().cast(), 32, 0);
        let mut f: AVFrame = std::mem::zeroed();
        f.data[0] = src32.as_raw().cast();
        let shared = bridge.export(&f).expect("export 32x32");
        assert!(shared.copy_to_renderer_texture(dst32.as_raw(), 32, 32));
        render_ctx.CopyResource(&stage32, &dst32);
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        render_ctx.Map(&stage32, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).unwrap();
        for row in 0..48 {
            let got = std::slice::from_raw_parts(mapped.pData.cast::<u8>().add(row * mapped.RowPitch as usize), 32);
            assert_eq!(got, &pattern[row * 32..(row + 1) * 32], "32x32 row {row}");
        }
        render_ctx.Unmap(&stage32, 0);
    }
}

impl DecodeBridge {
    pub(crate) fn new_for_test(renderer_device: &ID3D11Device, pictures: Pictures) -> Result<Arc<Self>, String> {
        if let Ok(bridge) = Self::new(renderer_device, pictures) {
            return Ok(bridge);
        }
        // On WARP (which lacks `D3D11_CREATE_DEVICE_VIDEO_SUPPORT`), open a second device on the same adapter.
        let (decode_device, decode_context) = unsafe {
            let dxgi: IDXGIDevice = renderer_device.cast().map_err(|e| format!("querying IDXGIDevice: {e}"))?;
            let adapter: IDXGIAdapter = dxgi.GetAdapter().map_err(|e| format!("reading DXGI adapter: {e}"))?;
            let (mut device, mut context) = (None, None);
            D3D11CreateDevice(
                &adapter, D3D_DRIVER_TYPE_UNKNOWN, HMODULE::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0]), D3D11_SDK_VERSION,
                Some(&mut device), None, Some(&mut context),
            ).map_err(|e| format!("creating test decode device: {e}"))?;
            let device = device.ok_or("no test decode device")?;
            if let Ok(mt) = device.cast::<ID3D10Multithread>() { let _ = mt.SetMultithreadProtected(true); }
            (device, context.ok_or("no test decode context")?)
        };
        Self::new_with_decode_device(renderer_device, decode_device, decode_context, pictures)
    }

    pub(crate) fn test_decode_device(&self) -> &ID3D11Device { &self.decode_device }

    pub(crate) fn test_decode_context(&self) -> &ID3D11DeviceContext { &self.decode_context }

    pub(crate) fn test_export_frame(
        self: &Arc<Self>,
        texture: &ID3D11Texture2D,
        slice: u32,
        width: i32,
        height: i32,
        ten_bit: bool,
    ) -> Result<super::super::Frame, String> {
        let mut raw: AVFrame = unsafe { std::mem::zeroed() };
        raw.data[0] = texture.as_raw().cast();
        raw.data[1] = slice as usize as *mut u8;
        raw.width = width;
        raw.height = height;
        let shared = unsafe { self.export(&raw) }?;
        super::super::Frame::from_test_shared(shared, width, height, ten_bit)
            .ok_or_else(|| "allocating test AVFrame failed".into())
    }

    pub(crate) fn test_counters(&self) -> (u64, u64, usize) {
        let in_use = self.pool.lock().unwrap().pool.as_ref()
            .map_or(0, |p| p.slots.iter().filter(|s| s.in_use.load(Ordering::Acquire)).count());
        (self.decode_counter.load(Ordering::Acquire), self.reuse_counter.load(Ordering::Acquire), in_use)
    }
}

#[test]
fn choose_reuse_slot_never_picks_unflushed_slot_while_headroom_exists() {
    let slots = [(true, 1), (true, 2), (false, 5)];
    assert_eq!(choose_reuse_slot(&slots, 1, 3, 2, HARD_SLOT_CAP), SlotChoice::Alloc);
    let capped = vec![(false, 5); HARD_SLOT_CAP];
    assert_eq!(choose_reuse_slot(&capped, 1, 3, 2, HARD_SLOT_CAP), SlotChoice::Exhausted);
    let flushed_slots = [(true, 1), (false, 5), (false, 4)];
    assert_eq!(choose_reuse_slot(&flushed_slots, 1, 5, 2, HARD_SLOT_CAP), SlotChoice::Reuse(2));
}

#[test]
fn device_removed_hresult_matches_dxgi_reset_and_hung_codes() {
    use windows::Win32::Graphics::Dxgi::{
        DXGI_ERROR_DEVICE_HUNG, DXGI_ERROR_DEVICE_REMOVED, DXGI_ERROR_DEVICE_RESET,
        DXGI_ERROR_DRIVER_INTERNAL_ERROR, DXGI_ERROR_WAS_STILL_DRAWING,
    };
    assert!(is_device_removed_code(DXGI_ERROR_DEVICE_REMOVED.0));
    assert!(is_device_removed_code(DXGI_ERROR_DEVICE_HUNG.0));
    assert!(is_device_removed_code(DXGI_ERROR_DEVICE_RESET.0));
    assert!(is_device_removed_code(DXGI_ERROR_DRIVER_INTERNAL_ERROR.0));
    assert!(!is_device_removed_code(DXGI_ERROR_WAS_STILL_DRAWING.0));
    assert!(!is_device_removed_code(0));
}

#[test]
fn decode_bridge_off_thread_drop_allocates_instead_of_stalling_and_removal_stops_immediately() {
    let Ok((renderer, _)) = make_device(D3D_DRIVER_TYPE_HARDWARE) else {
        return;
    };
    let pictures = Pictures { ten_bit: false, chroma444: false, waiting: 0, ..Pictures::default() };
    let bridge = DecodeBridge::new(&renderer, pictures).expect("DecodeBridge");
    let mut src_tex = None;
    unsafe {
        bridge.decode_device().CreateTexture2D(
            &D3D11_TEXTURE2D_DESC {
                Width: 16, Height: 16, MipLevels: 1, ArraySize: 1, Format: DXGI_FORMAT_NV12,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_DEFAULT, BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                ..Default::default()
            },
            None,
            Some(&mut src_tex),
        ).unwrap();
    }
    let src_tex = src_tex.unwrap();
    let mut fake: AVFrame = unsafe { std::mem::zeroed() };
    fake.data[0] = src_tex.as_raw().cast();

    // Queue a short-lived gate fence on `render_queue` so `signal_read_on_renderer()`'s reuse
    // signal cannot complete on the GPU before the off-thread `SlotLease::drop` checks it.
    let (gate_dec, gate_ren) = create_shared_fence_pair(
        &bridge.decode_device.cast::<ID3D11Device5>().unwrap(),
        &renderer.cast::<ID3D11Device5>().unwrap(),
        "test-gate",
    ).unwrap();
    struct GateGuard(ID3D11DeviceContext, ID3D11DeviceContext4, ID3D11Fence);
    impl Drop for GateGuard {
        fn drop(&mut self) {
            unsafe {
                let _ = self.1.Signal(&self.2, 1);
                self.0.Flush();
            }
        }
    }
    let _gate = GateGuard(bridge.decode_context.clone(), bridge.decode_queue.clone(), gate_dec);

    let shared = unsafe { bridge.export(&fake) }.expect("first export");
    shared.wait_on_renderer().expect("wait");
    unsafe { bridge.render_queue.Wait(&gate_ren, 1).unwrap() };
    shared.signal_read_on_renderer();
    std::thread::spawn(move || drop(shared)).join().unwrap();
    assert!(bridge.pending_flush.load(Ordering::Acquire) > 0);
    assert_eq!(bridge.flushed_reuse.load(Ordering::Acquire), 0);

    let second = unsafe { bridge.export(&fake) }.expect("second export allocates fresh slot");
    assert_eq!(bridge.pool.lock().unwrap().pool.as_ref().unwrap().slots.len(), 2);

    drop(_gate);
    second.wait_on_renderer().expect("flushes pending");
    assert!(bridge.flushed_reuse.load(Ordering::Acquire) > 0);
    drop(second);

    bridge.decode_removed.store(true, Ordering::Release);
    assert_eq!(bridge.check_removed(), D3d11Removal::DecodeOnly);
    assert!(unsafe { bridge.export(&fake) }.is_err(), "removed bridge fails export immediately");
    assert!(bridge.take_removal_log_once(), "first removal log prints");
    assert!(!bridge.take_removal_log_once(), "subsequent frames do not spam logs");

    let reuse_before = bridge.reuse_counter.load(Ordering::Acquire);
    let shared3 = SharedFrame { lease: Arc::new(SlotLease { bridge: bridge.clone(), slot: bridge.pool.lock().unwrap().pool.as_ref().unwrap().slots[0].clone() }), decode_value: 1 };
    assert!(shared3.wait_on_renderer().is_err());
    shared3.signal_read_on_renderer();
    assert_eq!(bridge.reuse_counter.load(Ordering::Acquire), reuse_before);
}
