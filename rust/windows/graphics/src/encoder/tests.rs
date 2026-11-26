//! Unit tests for encoder failure guidance, GPU eligibility, warning replay,
//! and SDR white luminance clamping.

use super::*;

#[test]
fn amf_failure_names_the_adapter_and_console_recovery() {
    let message = format!(
        "{:#}",
        amf_failed(
            anyhow::anyhow!("AMD AMF runtime unavailable"),
            "Microsoft Remote Display Adapter",
        )
    );
    assert!(
        message.starts_with("AMF could not start on Microsoft Remote Display Adapter"),
        "{message}"
    );
    assert!(message.contains("sign-in screen"));
    assert!(message.contains("RDP session was disconnected"));
    assert!(message.contains("unlock the console session"));
    assert!(message.contains("update the AMD graphics driver"));
    assert!(
        message.ends_with("AMD AMF runtime unavailable"),
        "{message}"
    );
}
#[test]
fn a_size_the_encoder_cannot_take_says_so_and_not_to_update_the_driver() {
    let size = SizeTooLarge {
        encoder: "AMF",
        width: 8192,
        height: 8192,
        max: Some((4096, 2176)),
    };
    let message = format!(
        "{:#}",
        amf_failed(
            anyhow::Error::new(size.clone()).context("AMF encoder Init"),
            "AMD Radeon RX 7900 XT"
        )
    );
    assert_eq!(
        message,
        "AMF could not start on AMD Radeon RX 7900 XT: 8192x8192 is larger than AMF on this GPU can encode (at most 4096x2176); choose a lower resolution on the device"
    );
    let message = format!(
        "{:#}",
        nvenc_failed(
            anyhow::Error::new(SizeTooLarge {
                encoder: "NVENC",
                max: None,
                ..size
            }),
            "NVIDIA GeForce RTX 4070"
        )
    );
    assert_eq!(
        message,
        "NVENC could not start on NVIDIA GeForce RTX 4070: 8192x8192 is larger than NVENC on this GPU can encode; choose a lower resolution on the device"
    );
    for advice in ["driver", "sign-in", "unlock"] {
        assert!(!message.contains(advice), "{message}");
    }
}
#[test]
fn a_size_the_encoder_cannot_take_is_not_retried_on_the_graphics_queue() {
    let size = SizeTooLarge {
        encoder: "AMF",
        width: 8192,
        height: 8192,
        max: Some((8192, 4352)),
    };
    assert!(!retries_on_graphics(
        &anyhow::Error::new(size).context("AMF encoder Init")
    ));
    assert!(retries_on_graphics(&anyhow::anyhow!(
        "AMF error AMF_FAIL (1)"
    )));
}
fn gpu(name: &str, vendor: u32) -> crate::capture::Gpu {
    crate::capture::Gpu {
        name: name.into(),
        vendor,
        dedicated_memory: 0,
        device: 0,
        luid: (0, 0),
        pnp_id: None,
        preference: None,
    }
}
#[test]
fn nvenc_failure_names_the_adapter_and_the_nvidia_driver() {
    let message = format!(
        "{:#}",
        nvenc_failed(
            anyhow::anyhow!("NVENC session failed"),
            "NVIDIA GeForce RTX 4070",
        )
    );
    assert!(
        message.starts_with("NVENC could not start on NVIDIA GeForce RTX 4070"),
        "{message}"
    );
    assert!(message.contains("unlock the console session"));
    assert!(message.contains("update the NVIDIA graphics driver"));
    assert!(!message.contains("AMD"), "{message}");
    assert!(message.ends_with("NVENC session failed"), "{message}");
}
#[test]
fn an_nvenc_session_limit_names_the_apps_not_the_driver() {
    let message = format!(
        "{:#}",
        nvenc_failed(
            anyhow::anyhow!("NVENC open session failed (10)")
                .context(crate::nvenc::SessionLimit { memory: false }),
            "NVIDIA GeForce RTX 4070",
        )
    );
    assert!(
        message.starts_with("NVENC could not start on NVIDIA GeForce RTX 4070"),
        "{message}"
    );
    assert!(message.contains("NVENC session limit reached"), "{message}");
    assert!(message.contains("OBS, ShadowPlay or Discord"), "{message}");
    assert!(!message.contains("driver"), "{message}");
    assert!(!message.contains("sign in"), "{message}");
}
#[test]
fn a_stream_on_an_intel_gpu_is_refused_in_words() {
    let all = [gpu("Intel(R) Arc(TM) A770", 0x8086)];
    let error = require_encoding_among(&all[0].name, all[0].vendor, &all).unwrap_err();
    let message = format!("{error:#}");
    assert!(
        message.contains("this PC has only Intel(R) Arc(TM) A770"),
        "{message}"
    );
    assert!(message.contains("stay on Rubylight 2.6.x"), "{message}");
    assert!(message.contains("/releases/tag/v2.6.0"), "{message}");
    // It never reaches an encoder, whose failure text would blame the driver.
    assert!(!message.contains("could not start"), "{message}");
}
#[test]
fn amd_and_nvidia_gpus_encode_and_an_intel_display_names_the_one_that_can() {
    let all = [
        gpu("Intel(R) UHD Graphics 770", 0x8086),
        gpu("NVIDIA GeForce RTX 4070", 0x10de),
        gpu("AMD Radeon(TM) Graphics", 0x1002),
    ];
    assert!(require_encoding_among(&all[1].name, 0x10de, &all).is_ok());
    assert!(require_encoding_among(&all[2].name, 0x1002, &all).is_ok());
    let message = format!(
        "{:#}",
        require_encoding_among(&all[0].name, 0x8086, &all).unwrap_err()
    );
    assert!(
        message
            .starts_with("Intel(R) UHD Graphics 770 is neither an AMD nor an NVIDIA graphics card"),
        "{message}"
    );
    assert!(
        message.contains("also has NVIDIA GeForce RTX 4070"),
        "{message}"
    );
}
#[test]
fn startup_warnings_survive_attaching_the_session_reporter() {
    let mut encoder = butterpollo_media_policy::recorded_warnings();
    encoder.set(
        "encoder_frame_size",
        "driver cannot apply the requested cap",
    );
    let s = std::sync::Arc::new(butterpollo_media_policy::RecordedWarnings::default());
    let session: std::sync::Arc<dyn WarningSink> = s.clone();
    session.set("encoder_conversion", "retained conversion warning");
    attach_warnings(&mut encoder, &session);
    assert!(std::sync::Arc::ptr_eq(&encoder, &session));
    assert!(
        s.snapshot().iter().any(|w| w.code == "encoder_frame_size"
            && w.message == "driver cannot apply the requested cap")
    );
    assert!(s.snapshot().iter().any(|w| w.code == "encoder_conversion"));
    encoder.set("encoder_buffer", "later rate-control warning");
    assert!(s.snapshot().iter().any(|w| w.code == "encoder_buffer"));
}
/// Enumerates native adapters before rejecting a nonexistent GPU.
#[test]
#[ignore = "requires an interactive desktop display and a hardware GPU"]
fn encoders_open_on_the_configured_gpu() {
    let stream = Negotiated {
        width: 640,
        height: 480,
        fps: 30,
        bitrate_kbps: 2000,
        ..Default::default()
    };
    // A GPU that is not there: the encoder must not quietly use the
    // display's GPU instead, as the codec probe did with two GPUs.
    let tuning = butterpollo_config::config::Config::parse("adapter_name = No Such GPU\n").unwrap();
    let error = Encoder::new_options(&stream, "", &tuning)
        .err()
        .expect("an encoder on a missing GPU");
    assert!(format!("{error:#}").contains("No Such GPU"), "{error:#}");
}

#[test]
fn encoder_luminance_allows_windows_sdr_white_range() {
    assert_eq!(Encoder::clamp_luminance(80., 1.), [80., 1.]);
    assert_eq!(Encoder::clamp_luminance(280., 1.), [280., 1.]);
    assert_eq!(Encoder::clamp_luminance(480., 1.), [480., 1.]);
    assert_eq!(Encoder::clamp_luminance(1500., 3.), [1000., 2.]);
    assert_eq!(Encoder::clamp_luminance(f32::NAN, 1.), [100., 1.]);
}
