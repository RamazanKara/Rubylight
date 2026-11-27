//! Unit tests for KMS display-plane scanout.

use super::*;
use crate::config::{Settings, Upscaling};

#[test]
fn pictures_stay_with_the_renderer_where_it_has_work_on_them() {
    assert_eq!(renderer_work(&Settings::default()), None);
    assert_eq!(
        renderer_work(&Settings {
            upscaling: Upscaling::Fsr1,
            ..Settings::default()
        }),
        Some("Upscaling")
    );
    // MetalFX is macOS's: a stored choice asks nothing of the renderer here.
    assert_eq!(
        renderer_work(&Settings {
            upscaling: Upscaling::MetalFx,
            ..Settings::default()
        }),
        None
    );
    assert_eq!(
        renderer_work(&Settings {
            frame_warp: true,
            ..Settings::default()
        }),
        Some("Frame Warp")
    );
    assert_eq!(
        renderer_work(&Settings {
            prefer_yuv444: true,
            ..Settings::default()
        }),
        Some("Sharper colored text")
    );
}

#[test]
fn planes_need_kms_the_setting_and_opengl() {
    // The tests never draw through KMS.
    assert!(
        possible(true, "opengl")
            .unwrap_err()
            .contains("not drawing straight to the display")
    );
    assert!(planned(&Settings::default(), "opengl").is_err());
}

#[test]
fn hdr_blob_retires_superseded_ids_on_update_and_disable() {
    let mut active = None;
    assert_eq!(atomic::retire_hdr_blob(&mut active, Some(10)), None);
    assert_eq!(active, Some(10));
    assert_eq!(atomic::retire_hdr_blob(&mut active, Some(11)), Some(10));
    assert_eq!(active, Some(11));
    assert_eq!(atomic::retire_hdr_blob(&mut active, Some(11)), None);
    assert_eq!(active, Some(11));
    assert_eq!(atomic::retire_hdr_blob(&mut active, None), Some(11));
    assert_eq!(active, None);
    assert_eq!(atomic::retire_hdr_blob(&mut active, None), None);
}
