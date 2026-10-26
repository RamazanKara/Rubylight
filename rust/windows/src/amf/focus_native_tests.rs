//! Opt-in M0 probe (docs/encoder-focus-design.md section 7): does this GPU's
//! AMF honour an ROI map, per codec and bit depth? Synthetic textures only.
use super::*;
use butterpollo_core::encoder_focus::Support;

#[test]
#[ignore = "requires an idle AMD GPU and AMF"]
fn focus_native_probe_verifies_roi_per_codec_and_depth() -> Result<()> {
    let _com = crate::capture::ComGuard::new()?;
    let device = Device::new("")?;
    let compute = crate::compute::Compute::for_device(&device.device)?;
    let mut rows = Vec::new();
    let mut failures = Vec::new();
    for (name, codec, ten_bit) in [
        ("h264", 0, false),
        ("hevc", 1, false),
        ("hevc", 1, true),
        ("av1", 2, false),
        ("av1", 2, true),
    ] {
        for (width, height, bitrate_kbps) in [(640, 480, 2_000), (1920, 1080, 20_000)] {
            let config = butterpollo_core::rtsp::Negotiated {
                width,
                height,
                fps: 60,
                bitrate_kbps,
                codec,
                sdr_10bit: ten_bit,
                ..Default::default()
            };
            let mut encoder = Encoder::new_gpu(
                &config,
                device.clone(),
                &Default::default(),
                Some(compute.clone()),
            )?;
            let probe = encoder
                .probe_focus()
                .with_context(|| format!("{name} {width}x{height}"))?;
            let depth = if ten_bit { 10 } else { 8 };
            let (verified, reason, message) = match &probe.support {
                Support::Verified { .. } => (true, None, None),
                Support::Unsupported { reason, message } => {
                    (false, Some(reason.wire()), Some(message.clone()))
                }
                Support::NotProbed => (false, None, None),
            };
            println!(
                "{name} {depth}-bit {width}x{height}: {} (qp_delta {:?}, control {:?}, feedback {:?})",
                probe.support.summary(),
                probe.qp_delta,
                probe.control_delta,
                probe.feedback_grid
            );
            if !verified {
                failures.push(format!("{name} {depth}-bit {width}x{height}"));
            }
            rows.push(serde_json::json!({
                "codec": name,
                "depth": depth,
                "width": width,
                "height": height,
                "verified": verified,
                "qp_delta": probe.qp_delta,
                "control_delta": probe.control_delta,
                "feedback_grid": probe.feedback_grid,
                "reason": reason,
                "message": message,
            }));
        }
    }
    if let Some(path) = std::env::var_os("BUTTERPOLLO_FOCUS_PROBE_OUT") {
        let path = std::path::PathBuf::from(path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, serde_json::to_string_pretty(&rows)?)
            .with_context(|| format!("write {}", path.display()))?;
    }
    anyhow::ensure!(
        failures.is_empty(),
        "ROI not verified for {}",
        failures.join(", ")
    );
    Ok(())
}
