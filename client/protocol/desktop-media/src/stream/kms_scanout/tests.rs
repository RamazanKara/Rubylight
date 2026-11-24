//! Unit tests for KMS display-plane scanout.

use super::*;
use crate::config::{Settings, Upscaling};

#[test]
fn pictures_stay_with_the_renderer_where_it_has_work_on_them() {
    assert_eq!(renderer_work(&Settings::default()), None);
    assert_eq!(renderer_work(&Settings { upscaling: Upscaling::Fsr1, ..Settings::default() }), Some("Upscaling"));
    // MetalFX is macOS's: a stored choice asks nothing of the renderer here.
    assert_eq!(renderer_work(&Settings { upscaling: Upscaling::MetalFx, ..Settings::default() }), None);
    assert_eq!(renderer_work(&Settings { frame_warp: true, ..Settings::default() }), Some("Frame Warp"));
    assert_eq!(renderer_work(&Settings { prefer_yuv444: true, ..Settings::default() }), Some("Sharper colored text"));
}

#[test]
fn planes_need_kms_the_setting_and_opengl() {
    // The tests never draw through KMS.
    assert!(possible(true, "opengl").unwrap_err().contains("not drawing straight to the display"));
    assert!(planned(&Settings::default(), "opengl").is_err());
}

#[test]
fn unchanged_overlay_skips_buffer_copy_until_hidden_or_modified() {
    let mut src = vec![1u8, 2, 3, 4, 5, 6, 7, 8];
    let mut dst = vec![0u8; 8];
    let mut cache = Vec::new();
    // First draw (not yet shown): copies into dst and caches.
    let changed = unsafe {
        sync_overlay_bytes(
            OverlayPlanes { src: src.as_ptr(), src_pitch: 4, dst: dst.as_mut_ptr(), dst_pitch: 4, rows: 2, row_bytes: 4 },
            false,
            &mut cache,
        )
    };
    assert!(changed);
    assert_eq!(dst, src);

    // Identical pixels while already shown: skips dst copy (clear dst to prove it isn't touched).
    dst.fill(0xAA);
    let changed = unsafe {
        sync_overlay_bytes(
            OverlayPlanes { src: src.as_ptr(), src_pitch: 4, dst: dst.as_mut_ptr(), dst_pitch: 4, rows: 2, row_bytes: 4 },
            true,
            &mut cache,
        )
    };
    assert!(!changed);
    assert_eq!(dst, vec![0xAA; 8], "write-combined buffer must not be written when unchanged");

    // Changing a single pixel copies again.
    src[5] = 99;
    let changed = unsafe {
        sync_overlay_bytes(
            OverlayPlanes { src: src.as_ptr(), src_pitch: 4, dst: dst.as_mut_ptr(), dst_pitch: 4, rows: 2, row_bytes: 4 },
            true,
            &mut cache,
        )
    };
    assert!(changed);
    assert_eq!(dst, src);

    // Re-showing after the plane was hidden still copies even if pixels match cache.
    dst.fill(0x55);
    let changed = unsafe {
        sync_overlay_bytes(
            OverlayPlanes { src: src.as_ptr(), src_pitch: 4, dst: dst.as_mut_ptr(), dst_pitch: 4, rows: 2, row_bytes: 4 },
            false,
            &mut cache,
        )
    };
    assert!(changed);
    assert_eq!(dst, src);
}
