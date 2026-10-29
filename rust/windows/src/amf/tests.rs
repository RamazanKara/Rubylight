use super::*;

/// How long an external tool (ffprobe, ffmpeg) may take before the test
/// fails. An ffprobe once sat on one AV1 frame for 40 minutes while the
/// test held the GPU.
const TOOL_TIMEOUT: Duration = Duration::from_secs(120);

/// `command`'s output, as `Command::output` gives it, but killed and failed
/// after `timeout`.
fn output_within_for(
    command: &mut std::process::Command,
    timeout: Duration,
) -> Result<std::process::Output> {
    use std::{io::Read, process::Stdio};
    let program = command.get_program().to_string_lossy().into_owned();
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("starting {program}"))?;
    let read = |stream: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut stream) = stream {
                let _ = stream.read_to_end(&mut bytes);
            }
            bytes
        })
    };
    let stdout = read(child.stdout.take().map(|s| Box::new(s) as _));
    let stderr = read(child.stderr.take().map(|s| Box::new(s) as _));
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("{program} did not finish within {timeout:?}");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    Ok(std::process::Output {
        status,
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    })
}

/// [`output_within_for`] with [`TOOL_TIMEOUT`].
fn output_within(command: &mut std::process::Command) -> Result<std::process::Output> {
    output_within_for(command, TOOL_TIMEOUT)
}

#[test]
fn a_tool_that_hangs_fails_the_test_instead_of_holding_it() {
    let started = Instant::now();
    let error = output_within_for(
        std::process::Command::new("cmd.exe").args(["/d", "/c", "ping -n 30 127.0.0.1 >nul"]),
        Duration::from_millis(500),
    )
    .unwrap_err();
    assert!(error.to_string().contains("did not finish"), "{error:#}");
    assert!(started.elapsed() < Duration::from_secs(10));
    // One that finishes gives its output as Command::output would.
    let done =
        output_within(std::process::Command::new("cmd.exe").args(["/d", "/c", "echo hi"])).unwrap();
    assert!(done.status.success());
    assert_eq!(String::from_utf8_lossy(&done.stdout).trim(), "hi");
}

#[test]
fn missing_output_type_uses_the_matching_submission_after_buffering_or_recreation() {
    // A buffered initial keyframe and a recovery keyframe at a continued wire
    // index still start a decoder when the driver cannot report its output type.
    for (pts, requested) in [(0, true), (1, false), (37, true), (38, false)] {
        let submission = Submission {
            pts,
            started: Instant::now(),
            accepted: Instant::now(),
            presentation: Instant::now(),
            idr: requested,
            after_invalidation: false,
            frame_cap_bytes: Some(100_000),
            frame_cap_target: Some(100_000),
            skipped: 0,
            cut_limit: None,
            reencoded: false,
            _capture: None,
            _converted: None,
            _converted12: None,
        };
        assert_eq!(output_is_idr(None, submission.idr), requested);
        // A real driver report remains authoritative over a requested type.
        assert!(output_is_idr(Some(0), submission.idr));
        for reported in [1, 2, 3] {
            assert!(!output_is_idr(Some(reported), submission.idr));
        }
    }
    // A late, already-expired output has no request metadata to fall back to.
    assert!(!output_is_idr(None, false));
}

#[test]
fn a_slow_first_probe_frame_is_logged_but_a_slow_stream_frame_warns_the_client() {
    // The 780M's service start: the startup probe's first H.264 frame took
    // over two seconds, then every codec worked. That is no warning.
    let Unreturned::Probe(message) = unreturned_notice(true, Some(0)) else {
        panic!("a probe frame must not raise a stream warning");
    };
    assert!(message.contains("probe's frame Some(0)"), "{message}");
    assert!(!message.contains("You may see a pause"), "{message}");
    let Unreturned::Warning(message) = unreturned_notice(false, Some(0)) else {
        panic!("a stream frame must warn");
    };
    assert!(message.contains("You may see a pause"), "{message}");
}

#[test]
fn a_frame_counts_as_unreturned_from_its_acceptance_not_from_its_conversion() {
    // The RX 7900 XT's packed probe: the first frame's upload and conversion
    // took 4.7 s, then AMF accepted it. Counting from the start dropped it in
    // the same encode call, before AMF had a chance to return it.
    let now = Instant::now();
    let submission = |started: Duration, accepted: Duration| Submission {
        pts: 0,
        started: now - started,
        accepted: now - accepted,
        presentation: now - started,
        idr: true,
        after_invalidation: false,
        frame_cap_bytes: None,
        frame_cap_target: None,
        skipped: 0,
        cut_limit: None,
        reencoded: false,
        _capture: None,
        _converted: None,
        _converted12: None,
    };
    let slow_conversion = submission(Duration::from_millis(4700), Duration::ZERO);
    assert!(!slow_conversion.unreturned(now));
    assert!(!slow_conversion.unreturned(now + Duration::from_millis(1900)));
    assert!(slow_conversion.unreturned(now + Duration::from_millis(2100)));
    // A frame AMF has held for over two seconds is still lost.
    let held = submission(Duration::from_millis(2600), Duration::from_millis(2500));
    assert!(held.unreturned(now));
}

#[test]
#[ignore = "requires a live AMD D3D11 adapter and AMF runtime"]
fn native_output_survives_a_late_poll_without_losing_submission_metadata() -> Result<()> {
    let _com = crate::capture::ComGuard::new()?;
    let device = Device::new("")?;
    let config = butterpollo_core::rtsp::Negotiated {
        width: 640,
        height: 480,
        codec: 0,
        ..Default::default()
    };
    let presented = Instant::now();
    let image = GpuImage::upload(
        &device,
        &Image {
            width: config.width,
            height: config.height,
            stride: config.width as usize * 4,
            bytes: vec![128; config.width as usize * config.height as usize * 4],
            captured: presented,
            pixel: crate::capture::Pixel::Bgra8,
        },
    )?;
    let mut encoder = Encoder::new_device(&config, device)?;
    let mut converter = gpu::Converter::new(
        &encoder._device,
        &config,
        (image.width, image.height, image.pixel),
    )?;
    let (surface, converted) =
        converter.convert(encoder.context, &image, &mut encoder.ownership)?;
    encoder.gpu_convert = Some(converter);
    let (plan, idr) = encoder.prepare_surface(surface.0, true)?;
    // Submit without the convenience encode method's immediate poll. This
    // guarantees that QueryOutput must collect an accepted native picture,
    // independently of how quickly this particular GPU completes it.
    // SAFETY: The surface and component are owned by the live encoder/context;
    // AMF retains the input, and its ownership callback retains the texture.
    unsafe {
        ((*(*surface.0).pVtbl).SetPts.unwrap())(surface.0, encoder.index);
        check(((*(*encoder.component).pVtbl).SubmitInput.unwrap())(
            encoder.component,
            surface.0.cast(),
        ))?;
    }
    let delayed = Instant::now() - Duration::from_secs(3);
    encoder.in_flight.push_back(Submission {
        pts: encoder.index,
        started: delayed,
        accepted: delayed,
        presentation: presented,
        idr,
        after_invalidation: plan.after_invalidation,
        frame_cap_bytes: encoder.frame_cap(),
        frame_cap_target: encoder.frame_cap_target,
        skipped: plan.skipped,
        cut_limit: None,
        reencoded: false,
        _capture: Some(image),
        _converted: Some(converted),
        _converted12: None,
    });
    encoder.backlog.submitted();
    encoder.references.accepted(encoder.index as u64 + 1, &plan);
    encoder.index += 1;
    drop(surface);
    // Let the small synthetic frame complete while the caller does not poll,
    // as happens when its thread is descheduled while the GPU keeps working.
    std::thread::sleep(Duration::from_millis(100));
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut output = Vec::new();
    while encoder.pending() && Instant::now() < deadline {
        output.extend(encoder.poll()?);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        output.len(),
        1,
        "late polling must still query native output"
    );
    assert!(!encoder.pending());
    assert!(!output[0].bytes.is_empty());
    assert!(output[0].idr);
    assert_eq!(output[0].presentation, Some(presented));
    assert!(
        output[0]
            .latency
            .is_some_and(|latency| latency >= Duration::from_secs(3))
    );
    assert!(!encoder.recover, "a late poll is not a lost reference");
    Ok(())
}

#[test]
#[ignore = "requires a live AMD adapter with H.264, HEVC and AV1 encoding"]
fn native_runtime_frame_caps_survive_bitrate_changes_and_follow_submissions() -> Result<()> {
    let _com = crate::capture::ComGuard::new()?;
    let device = Device::new("")?;
    for codec in 0..=2 {
        let config = butterpollo_core::rtsp::Negotiated {
            width: 1280,
            height: 720,
            codec,
            bitrate_kbps: 54_000,
            fps: 120,
            rate_millihz: 119_880,
            ..Default::default()
        };
        let mut image = GpuImage::upload(
            &device,
            &Image {
                width: config.width,
                height: config.height,
                stride: config.width as usize * 4,
                bytes: (0..config.width as usize * config.height as usize * 4)
                    .map(|index| (index % 251) as u8)
                    .collect(),
                captured: Instant::now(),
                pixel: crate::capture::Pixel::Bgra8,
            },
        )?;
        let mut encoder = Encoder::new_device(&config, device.clone())
            .with_context(|| format!("native runtime cap codec {codec}"))?;
        let mut expected = Vec::new();
        let mut output = Vec::new();
        for (pace, bitrate) in [
            (800_000_000, 54_000),
            (70_770_000, 54_000),
            (54_000_000, 27_000),
            (800_000_000, 54_000),
        ] {
            let cap = butterpollo_core::network_pacing::recovery_frame_cap(
                pace,
                bitrate,
                Duration::from_secs_f64(1. / 119.88),
                10,
                1392,
                false,
            );
            assert_eq!(
                encoder.set_frame_cap(cap, 0),
                Some(cap),
                "codec {codec}: runtime cap rejected at pace {pace}"
            );
            image.captured = Instant::now();
            expected.push((image.captured, cap));
            output.extend(encoder.encode_gpu(&image, true, bitrate)?);
            assert_eq!(
                encoder.read(rate_control::frame_cap_name(codec)),
                Some((cap * 8) as i64),
                "codec {codec}: bitrate update changed the cap"
            );
            println!("codec={codec} pace_bps={pace} bitrate_kbps={bitrate} frame_cap_bytes={cap}");
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while encoder.pending() && Instant::now() < deadline {
            output.extend(encoder.poll()?);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(
            output.len(),
            expected.len(),
            "codec {codec}: missing output"
        );
        for frame in output {
            assert!(!frame.bytes.is_empty());
            assert!(frame.idr);
            let (_, cap) = expected
                .iter()
                .find(|(presentation, _)| Some(*presentation) == frame.presentation)
                .expect("output must match a submitted presentation");
            assert_eq!(
                frame.frame_cap_bytes,
                Some(*cap),
                "codec {codec}: output used a later cap"
            );
        }
    }
    Ok(())
}

#[test]
fn output_range_preserves_optional_input_and_limited_driver_defaults() -> Result<()> {
    for (codec, input, output) in [
        (0, "InputFullRangeColor", "FullRangeColor"),
        (1, "HevcInputFullRangeColor", "HevcNominalRange"),
        (2, "Av1InputFullRangeColor", "Av1NominalRange"),
    ] {
        let mut calls = Vec::new();
        let range = OutputRange::configure(codec, false, |name, full| {
            calls.push((name.to_string(), full));
            // Older drivers can reject either property. Limited remains the
            // documented default, including the RX 6800 XT issue #5 case.
            bail!("mock AMF_INVALID_ARG")
        });
        assert_eq!(
            calls,
            [(input.to_string(), false), (output.to_string(), false)]
        );
        assert_eq!(range.name, output);
        range.validate(None)?;
        range.validate(Some(0))?;
        assert!(range.validate(Some(1)).is_err());

        let range = OutputRange::configure(codec, true, |name, full| {
            assert!(full);
            if name == input {
                bail!("optional input property unavailable");
            }
            Ok(())
        });
        range.validate(Some(1))?;
        // A successful output setter remains sufficient if this driver
        // cannot read back a supported property.
        range.validate(None)?;
    }
    Ok(())
}

#[test]
fn output_range_verifies_rejected_setters_and_initialized_readback() -> Result<()> {
    for codec in 0..=2 {
        let rejected =
            OutputRange::configure(codec, true, |_, _| bail!("output setter unavailable"));
        // A preset or the colour profile can still have selected full range.
        rejected.validate(Some(1))?;
        assert!(rejected.validate(Some(0)).is_err());
        assert!(rejected.validate(None).is_err());

        let accepted = OutputRange::configure(codec, true, |_, _| Ok(()));
        accepted.validate(Some(1))?;
        // If Init reverted the setting, its current value wins over setter
        // success. Unknown values must not be interpreted as full range.
        assert!(accepted.validate(Some(0)).is_err());
        assert!(accepted.validate(Some(2)).is_err());
    }
    Ok(())
}

fn hdr_request(maximum_nits: u16) -> metadata::EncodedMetadata {
    metadata::EncodedMetadata::new(
        1,
        butterpollo_core::hdr::Metadata {
            maximum_nits,
            ..Default::default()
        },
    )
}

#[test]
fn hdr_same_value_retries_after_failure_and_stops_after_success() {
    use metadata::{Retry, State, Update, WriteError, WriteStage};
    let mut state = State::default();
    let now = Instant::now();
    let request = hdr_request(1000);
    let error = WriteError {
        stage: WriteStage::Buffer,
        code: AMF_RESULT_AMF_OUT_OF_MEMORY,
    };
    let mut writes = Vec::new();
    assert_eq!(
        state.update(request, now, |value| {
            writes.push(*value);
            Err(error)
        }),
        Update::Failed {
            error,
            retry: Retry::After(Duration::from_secs(1)),
            report: true
        }
    );
    assert!(!state.needs_idr());
    assert_eq!(
        state.update(request, now + Duration::from_millis(999), |_| {
            panic!("backoff must avoid calling AMF")
        }),
        Update::Deferred
    );
    assert_eq!(
        state.update(request, now + Duration::from_secs(1), |value| {
            writes.push(*value);
            Ok(())
        }),
        Update::Applied
    );
    assert_eq!(writes, [request, request]);
    assert!(state.needs_idr());
    state.submitted(AMF_RESULT_AMF_OK);
    assert!(!state.needs_idr());
    assert_eq!(
        state.update(request, now + Duration::from_secs(2), |_| {
            panic!("successful metadata must not be rewritten every second")
        }),
        Update::Unchanged
    );
    assert!(!state.needs_idr());
}

#[test]
fn hdr_transient_retries_are_capped_and_logs_rate_limited() {
    use metadata::{Retry, State, Update, WriteError, WriteStage};
    let mut state = State::default();
    let start = Instant::now();
    let mut elapsed = 0;
    let mut reports = Vec::new();
    let error = WriteError {
        stage: WriteStage::Property,
        code: AMF_RESULT_AMF_FAIL,
    };
    for delay in [1, 2, 4, 8, 16, 30, 30, 30] {
        let now = start + Duration::from_secs(elapsed);
        let Update::Failed { retry, report, .. } =
            state.update(hdr_request(1000), now, |_| Err(error))
        else {
            panic!("the failed request must remain eligible for retry");
        };
        assert_eq!(retry, Retry::After(Duration::from_secs(delay)));
        if report {
            reports.push(elapsed);
        }
        assert_eq!(
            state.update(hdr_request(1000), now + Duration::from_millis(1), |_| {
                panic!("no spin retry")
            }),
            Update::Deferred
        );
        elapsed += delay;
    }
    assert_eq!(reports, [0, 31, 61, 91]);
}

#[test]
fn hdr_retries_latest_request_and_keeps_last_successful_value() {
    use metadata::{State, Update, WriteError, WriteStage};
    let mut state = State::default();
    let now = Instant::now();
    let original = hdr_request(1000);
    assert_eq!(state.update(original, now, |_| Ok(())), Update::Applied);
    state.submitted(AMF_RESULT_AMF_OK);
    let error = WriteError {
        stage: WriteStage::Property,
        code: AMF_RESULT_AMF_FAIL,
    };
    assert!(matches!(
        state.update(hdr_request(600), now, |_| Err(error)),
        Update::Failed { .. }
    ));
    assert!(!state.needs_idr());
    assert_eq!(
        state.update(original, now, |_| {
            panic!("a failed write must not replace the last applied value")
        }),
        Update::Unchanged
    );
    assert!(matches!(
        state.update(hdr_request(600), now, |_| Err(error)),
        Update::Failed { .. }
    ));
    let latest = hdr_request(1400);
    assert_eq!(
        state.update(latest, now + Duration::from_millis(500), |_| {
            panic!("changing requests must not bypass the driver's backoff")
        }),
        Update::Deferred
    );
    assert_eq!(
        state.update(latest, now + Duration::from_secs(1), |value| {
            assert_eq!(*value, latest);
            Ok(())
        }),
        Update::Applied
    );
    assert!(state.needs_idr());
}

#[test]
fn hdr_unsupported_property_is_not_retried_but_bad_values_can_change() {
    use metadata::{Retry, State, Update, WriteError, WriteStage};
    let now = Instant::now();
    for code in [
        AMF_RESULT_AMF_NOT_SUPPORTED,
        AMF_RESULT_AMF_NOT_IMPLEMENTED,
        AMF_RESULT_AMF_NOT_FOUND,
    ] {
        let mut state = State::default();
        let error = WriteError {
            stage: WriteStage::Property,
            code,
        };
        assert_eq!(
            state.update(hdr_request(1000), now, |_| Err(error)),
            Update::Failed {
                error,
                retry: Retry::Unsupported,
                report: true
            }
        );
        for value in [1000, 600] {
            assert_eq!(
                state.update(hdr_request(value), now + Duration::from_secs(60), |_| {
                    panic!("an unsupported property requires a new encoder/driver")
                }),
                Update::Deferred
            );
        }
    }
    for code in [
        AMF_RESULT_AMF_INVALID_ARG,
        AMF_RESULT_AMF_OUT_OF_RANGE,
        AMF_RESULT_AMF_INVALID_FORMAT,
        AMF_RESULT_AMF_INVALID_DATA_TYPE,
    ] {
        let mut state = State::default();
        let error = WriteError {
            stage: WriteStage::Property,
            code,
        };
        assert_eq!(
            state.update(hdr_request(1000), now, |_| Err(error)),
            Update::Failed {
                error,
                retry: Retry::ChangedValue,
                report: true
            }
        );
        assert_eq!(
            state.update(hdr_request(1000), now + Duration::from_secs(60), |_| {
                panic!("retrying an invalid value cannot repair it")
            }),
            Update::Deferred
        );
        assert_eq!(
            state.update(hdr_request(600), now, |_| Ok(())),
            Update::Applied
        );
    }
    // A buffer failure does not establish that the metadata property is
    // unsupported, even if it happens to return the same numeric error.
    let mut state = State::default();
    let error = WriteError {
        stage: WriteStage::Buffer,
        code: AMF_RESULT_AMF_NOT_SUPPORTED,
    };
    assert_eq!(
        state.update(hdr_request(1000), now, |_| Err(error)),
        Update::Failed {
            error,
            retry: Retry::After(Duration::from_secs(1)),
            report: true
        }
    );
    assert_eq!(
        state.update(hdr_request(1000), now + Duration::from_secs(1), |_| Ok(())),
        Update::Applied
    );
}

#[test]
fn hdr_changed_metadata_requests_one_idr_until_submission_succeeds() {
    use metadata::{State, Update};
    let mut state = State::default();
    let now = Instant::now();
    assert_eq!(
        state.update(hdr_request(1000), now, |_| Ok(())),
        Update::Applied
    );
    for result in [
        AMF_RESULT_AMF_INPUT_FULL,
        AMF_RESULT_AMF_FAIL,
        AMF_RESULT_AMF_INPUT_FULL,
    ] {
        assert!(state.needs_idr());
        state.submitted(result);
        assert!(
            state.needs_idr(),
            "failed submission must keep the header/keyframe request"
        );
    }
    // Coalesce another successful update before the first accepted frame.
    assert_eq!(
        state.update(hdr_request(600), now, |_| Ok(())),
        Update::Applied
    );
    assert!(state.needs_idr());
    state.submitted(AMF_RESULT_AMF_OK);
    assert!(!state.needs_idr());
    assert_eq!(
        state.update(hdr_request(600), now, |_| panic!("duplicate")),
        Update::Unchanged
    );
    assert!(!state.needs_idr());
    assert_eq!(
        state.update(hdr_request(1400), now, |_| Ok(())),
        Update::Applied
    );
    assert!(state.needs_idr());
}

#[test]
fn keyframe_acknowledges_all_reasons_only_on_successful_idr_submission() {
    use metadata::State;
    for explicit in [false, true] {
        for changed_metadata in [false, true] {
            for recovery in [false, true] {
                let mut state = State::default();
                let mut recover = recovery;
                if changed_metadata {
                    state.update(hdr_request(1000), Instant::now(), |_| Ok(()));
                }
                let idr = explicit || state.needs_idr() || recover;
                for result in [AMF_RESULT_AMF_INPUT_FULL, AMF_RESULT_AMF_FAIL] {
                    acknowledge_keyframe(idr, result, &mut recover, &mut state);
                    assert_eq!(recover, recovery);
                    assert_eq!(state.needs_idr(), changed_metadata);
                }
                acknowledge_keyframe(idr, AMF_RESULT_AMF_OK, &mut recover, &mut state);
                assert!(!recover);
                assert!(!state.needs_idr());
            }
        }
    }
    // An old frame can time out in poll() after this non-IDR was prepared.
    // Its successful submission does not satisfy that newly requested IDR.
    let mut recover = true;
    let mut state = State::default();
    acknowledge_keyframe(false, AMF_RESULT_AMF_OK, &mut recover, &mut state);
    assert!(recover);
    acknowledge_keyframe(true, AMF_RESULT_AMF_OK, &mut recover, &mut state);
    assert!(!recover);
}

#[test]
fn hdr_effective_metadata_ignores_control_only_values_and_preserves_codec_units() {
    use metadata::{EncodedMetadata, State, Update};
    let metadata = butterpollo_core::hdr::Metadata::display(1000., 0.005, 400.);
    for codec in [1, 2] {
        let encoded = EncodedMetadata::new(codec, metadata);
        let value = encoded.value();
        assert_eq!(
            value.redPrimary,
            if codec == 1 {
                [35400, 14600]
            } else {
                [46399, 19136]
            }
        );
        assert_eq!(
            value.maxMasteringLuminance,
            if codec == 1 { 10_000_000 } else { 256_000 }
        );
        assert_eq!(
            value.minMasteringLuminance,
            if codec == 1 { 50 } else { 81 }
        );
        let mut state = State::default();
        let now = Instant::now();
        assert_eq!(state.update(encoded, now, |_| Ok(())), Update::Applied);
        state.submitted(AMF_RESULT_AMF_OK);
        assert_eq!(
            state.update(
                EncodedMetadata::new(
                    codec,
                    butterpollo_core::hdr::Metadata {
                        full_frame_nits: 600,
                        ..metadata
                    }
                ),
                now,
                |_| panic!("full_frame_nits is only on the control channel")
            ),
            Update::Unchanged
        );
        assert!(!state.needs_idr());
    }
    let extreme = EncodedMetadata::new(
        2,
        butterpollo_core::hdr::Metadata {
            primaries: [[u16::MAX; 2]; 3],
            white: [u16::MAX; 2],
            maximum_nits: u16::MAX,
            minimum: u16::MAX,
            ..metadata
        },
    )
    .value();
    assert_eq!(extreme.redPrimary, [u16::MAX; 2]);
    assert_eq!(extreme.whitePoint, [u16::MAX; 2]);
    assert_eq!(extreme.maxMasteringLuminance, 16_776_960);
    assert_eq!(extreme.minMasteringLuminance, 107_372);
}

#[test]
fn rate_limits_scale_by_the_requested_over_the_last_applied_bitrate() {
    // The arithmetic set_bitrate uses after 80 applied, 40 rejected and 20
    // accepted: the divisor is the applied 80, so the limits are quartered.
    for (current, expected) in [
        (96_000_000, 24_000_000),
        (666_667, 166_666),
        (2_666_668, 666_667),
    ] {
        assert_eq!(scale_rate_control(current, 80_000, 20_000), expected);
        assert_eq!(scale_rate_control(current, 80_000, 80_000), current);
    }
    assert_eq!(scale_rate_control(0, 80_000, 20_000), 0);
    assert_eq!(scale_rate_control(i64::MAX, 1, u32::MAX), i64::MAX);
    // Clamp only the final value, not the multiplication before division.
    assert_eq!(scale_rate_control(i64::MAX, 80_000, 80_000), i64::MAX);
    assert_eq!(scale_rate_control(i64::MAX, 80_000, 40_000), i64::MAX / 2);
}

struct FakeRateProperties {
    values: std::collections::BTreeMap<String, rate_control::Limit>,
    rejected: std::collections::BTreeSet<String>,
    unreadable: std::collections::BTreeSet<String>,
    writes: Vec<(String, i64)>,
    reads: usize,
}

impl FakeRateProperties {
    fn new(codec: u8) -> Self {
        let prefix = match codec {
            0 => "",
            1 => "Hevc",
            _ => "Av1",
        };
        let values = [
            ("PeakBitrate", 96_000_000),
            ("VBVBufferSize", 2_666_668),
            (
                if codec == 2 {
                    "MaxCompressedFrameSize"
                } else {
                    "MaxAUSize"
                },
                666_668,
            ),
        ]
        .into_iter()
        .map(|(suffix, value)| {
            (
                format!("{prefix}{suffix}"),
                rate_control::Limit {
                    value,
                    minimum: None,
                    maximum: None,
                },
            )
        })
        .collect();
        Self {
            values,
            rejected: Default::default(),
            unreadable: Default::default(),
            writes: vec![],
            reads: 0,
        }
    }

    fn apply(
        &mut self,
        state: &mut rate_control::State,
        codec: u8,
        bitrate: u32,
        warnings: &butterpollo_core::session::Warnings,
    ) {
        let driver = std::cell::RefCell::new(self);
        state.update(
            codec,
            bitrate,
            warnings,
            |name| {
                let mut driver = driver.borrow_mut();
                driver.reads += 1;
                if driver.unreadable.contains(name) {
                    None
                } else {
                    driver.values.get(name).copied()
                }
            },
            |name, value| {
                let mut driver = driver.borrow_mut();
                driver.writes.push((name.to_string(), value));
                if driver.rejected.contains(name) {
                    bail!("mock rejected {name}");
                }
                if let Some(limit) = driver.values.get_mut(name) {
                    limit.value = value;
                }
                Ok(())
            },
        );
    }
}

#[test]
fn frame_size_rejection_or_readback_mismatch_warns_without_blocking_streams() -> Result<()> {
    for codec in 0..=2 {
        let stream = butterpollo_core::rtsp::Negotiated {
            codec,
            ..Default::default()
        };
        for text in ["", "amd_max_frame_size=2", "amd_max_frame_size=1"] {
            let config = butterpollo_core::config::Config::parse(text)?;
            let properties = butterpollo_core::encoder_policy::amf_rate_control(&config, &stream)?;
            for failure in [
                "mock AMF_NOT_SUPPORTED",
                "mock readback differs from requested size",
            ] {
                let warnings = butterpollo_core::session::Warnings::default();
                let mut attempted = Vec::new();
                rate_control::configure(&properties, &warnings, |property| {
                    attempted.push(property.name.clone());
                    bail!("{failure}")
                })?;
                assert_eq!(attempted, [properties[0].name.clone()]);
                let messages = warnings.snapshot();
                assert_eq!(messages.len(), 1);
                assert_eq!(messages[0].code, "encoder_frame_size");
                assert!(messages[0].message.contains(failure));
                assert!(messages[0].message.contains(&properties[0].name));
                assert!(messages[0].message.contains("streaming continues"));
            }
        }
        for key in ["amd_peak_bitrate_ratio", "amd_vbv_buffer_frames"] {
            let config = butterpollo_core::config::Config::parse(&format!("{key}=1"))?;
            let properties = butterpollo_core::encoder_policy::amf_rate_control(&config, &stream)?;
            let warnings = butterpollo_core::session::Warnings::default();
            assert!(
                rate_control::configure(&properties, &warnings, |_| {
                    bail!("mock AMF_NOT_SUPPORTED")
                })
                .is_err()
            );
            assert!(
                warnings.snapshot().is_empty(),
                "explicit peak/VBV stay strict"
            );
        }
    }
    Ok(())
}

#[test]
fn rate_control_keeps_each_failed_property_on_its_own_applied_bitrate() {
    for codec in 0..=2 {
        let initial = FakeRateProperties::new(codec).values;
        for failed in initial.keys() {
            let mut driver = FakeRateProperties::new(codec);
            let mut state = rate_control::State::new(80_000, true);
            let warnings = butterpollo_core::session::Warnings::default();
            driver.rejected.insert(failed.clone());
            driver.apply(&mut state, codec, 40_000, &warnings);
            assert_eq!(driver.values[failed].value, initial[failed].value);
            assert!(warnings.snapshot().iter().any(
                |warning| warning.code == "encoder_buffer" && warning.message.contains(failed)
            ));
            driver.rejected.clear();
            driver.apply(&mut state, codec, 20_000, &warnings);
            for (name, original) in &initial {
                assert_eq!(
                    driver.values[name].value,
                    original.value / 4,
                    "{name}, previously rejected {failed}"
                );
            }
            assert!(warnings.snapshot().is_empty());
        }
    }
}

#[test]
fn only_explicit_frame_caps_scale_with_bitrate_and_driver_default_stays_disabled() -> Result<()> {
    for codec in 0..=2 {
        for text in [
            "",
            "amd_max_frame_size=auto",
            "amd_max_frame_size=2",
            "amd_max_frame_size=0",
        ] {
            let stream = butterpollo_core::rtsp::Negotiated {
                codec,
                bitrate_kbps: 80_000,
                fps: 120,
                rate_millihz: 120_000,
                ..Default::default()
            };
            let config = butterpollo_core::config::Config::parse(text)?;
            let properties = butterpollo_core::encoder_policy::amf_rate_control(&config, &stream)?;
            let mut driver = FakeRateProperties::new(codec);
            let cap = match codec {
                0 => "MaxAUSize",
                1 => "HevcMaxAUSize",
                _ => "Av1MaxCompressedFrameSize",
            };
            driver.values.get_mut(cap).unwrap().value = 0;
            let warnings = butterpollo_core::session::Warnings::default();
            rate_control::configure(&properties, &warnings, |property| {
                let butterpollo_core::encoder_policy::Value::Integer(value) = property.value else {
                    panic!("rate limits are integer bit budgets")
                };
                driver.values.get_mut(&property.name).unwrap().value = value;
                Ok(())
            })?;
            let disabled = text.ends_with("=0");
            assert_eq!(
                driver.values[cap].value,
                if disabled { 0 } else { 1_333_333 }
            );
            let automatic = butterpollo_core::encoder_policy::amf_frame_cap_automatic(&config);
            let mut state = rate_control::State::new(stream.bitrate_kbps, !automatic && !disabled);
            driver.apply(&mut state, codec, 40_000, &warnings);
            assert_eq!(
                driver.values[cap].value,
                if disabled {
                    0
                } else if automatic {
                    1_333_333
                } else {
                    666_666
                }
            );
            assert_eq!(
                driver.writes.iter().any(|(name, _)| name == cap),
                !disabled && !automatic
            );
            if disabled {
                // Driver default is an opt-out even if AMF reports a nonzero
                // default limit after its own rate-control reconfiguration.
                driver.values.get_mut(cap).unwrap().value = 1_777_777;
                driver.apply(&mut state, codec, 20_000, &warnings);
                assert_eq!(driver.values[cap].value, 1_777_777);
                assert!(!driver.writes.iter().any(|(name, _)| name == cap));
            }
            assert!(warnings.snapshot().is_empty());
        }
    }
    Ok(())
}

#[test]
fn rate_control_retains_failed_basis_and_warning_across_read_failures_and_clamps() {
    let mut driver = FakeRateProperties::new(1);
    let mut state = rate_control::State::new(80_000, true);
    let warnings = butterpollo_core::session::Warnings::default();
    let peak = "HevcPeakBitrate";
    driver.rejected.insert(peak.into());
    driver.apply(&mut state, 1, 40_000, &warnings);
    driver.rejected.clear();
    driver.unreadable.insert(peak.into());
    driver.apply(&mut state, 1, 20_000, &warnings);
    assert_eq!(driver.values[peak].value, 96_000_000);
    assert!(
        warnings
            .snapshot()
            .iter()
            .any(|warning| warning.code == "encoder_buffer"
                && warning.message.contains("cannot be verified"))
    );
    driver.unreadable.clear();
    let limit = driver.values.get_mut(peak).unwrap();
    limit.minimum = Some(22_000_000);
    limit.maximum = Some(30_000_000);
    driver.apply(&mut state, 1, 10_000, &warnings);
    // 96 * 10 / 80 = 12, clamped to 22. Using the unrelated target20
    // as denominator would produce 48, incorrectly clamped to 30.
    assert_eq!(driver.values[peak].value, 22_000_000);
    assert!(warnings.snapshot().is_empty());
    driver.apply(&mut state, 1, 40_000, &warnings);
    assert_eq!(driver.values[peak].value, 30_000_000);
    let limit = driver.values.get_mut(peak).unwrap();
    limit.minimum = None;
    limit.maximum = None;
    driver.apply(&mut state, 1, 20_000, &warnings);
    // Both clamped writes succeeded: the basis is now target40, not target80.
    assert_eq!(driver.values[peak].value, 15_000_000);
}

#[test]
fn rate_control_target_rejection_preserves_limits_and_existing_buffer_warning() {
    let mut driver = FakeRateProperties::new(2);
    let mut state = rate_control::State::new(80_000, true);
    let warnings = butterpollo_core::session::Warnings::default();
    driver.rejected.insert("Av1PeakBitrate".into());
    driver.apply(&mut state, 2, 40_000, &warnings);
    let reads = driver.reads;
    driver.rejected.insert("Av1TargetBitrate".into());
    driver.apply(&mut state, 2, 20_000, &warnings);
    assert_eq!(
        driver.reads, reads,
        "no limit updates after a target rejection"
    );
    assert!(
        warnings
            .snapshot()
            .iter()
            .any(|warning| warning.code == "encoder_bitrate")
    );
    assert!(
        warnings
            .snapshot()
            .iter()
            .any(|warning| warning.code == "encoder_buffer")
    );
    let writes = driver.writes.len();
    driver.apply(&mut state, 2, 20_000, &warnings);
    assert_eq!(
        driver.writes.len(),
        writes,
        "do not retry a rejected target every frame"
    );
    driver.rejected.clear();
    driver.apply(&mut state, 2, 10_000, &warnings);
    assert_eq!(driver.values["Av1PeakBitrate"].value, 12_000_000);
    assert_eq!(driver.values["Av1VBVBufferSize"].value, 333_333);
    assert_eq!(driver.values["Av1MaxCompressedFrameSize"].value, 83_333);
    assert!(warnings.snapshot().is_empty());
}

#[test]
fn settings_log_shows_each_codecs_profile_and_bit_depth() {
    // A 10-bit SDR stream is told apart from 8-bit only by these two, as the
    // driver applied them.
    for (codec, expected) in [
        (0, ["Profile", "ColorBitDepth"]),
        (1, ["HevcProfile", "HevcColorBitDepth"]),
        (2, ["Av1Profile", "Av1ColorBitDepth"]),
    ] {
        let names: Vec<_> = Encoder::setting_names(codec)
            .iter()
            .copied()
            .filter(|name| name.ends_with("Profile") || name.ends_with("ColorBitDepth"))
            .collect();
        assert_eq!(names, expected);
    }
}

#[test]
fn settings_log_queries_each_codecs_frame_skip_property() {
    for (codec, expected) in [
        (0, "RateControlSkipFrameEnable"),
        (1, "HevcRateControlSkipFrameEnable"),
        (2, "Av1RateControlSkipFrameEnable"),
    ] {
        let names: Vec<_> = Encoder::setting_names(codec)
            .iter()
            .copied()
            .filter(|name| name.contains("RateControlSkipFrame"))
            .collect();
        assert_eq!(names, [expected]);
    }
}

#[test]
fn smart_access_video_excludes_only_forced_low_latency() -> Result<()> {
    use butterpollo_core::{config::Config, encoder_policy, rtsp::Negotiated};
    for codec in 0..=2 {
        for low_latency in ["auto", "enabled", "disabled"] {
            for sav in ["auto", "enabled", "disabled"] {
                let config = Config::parse(&format!(
                    "amd_lowlatency_mode={low_latency}\namd_smart_access_video={sav}\n"
                ))?;
                let stream = Negotiated {
                    codec,
                    ..Default::default()
                };
                let original = encoder_policy::amf(&config, &stream)?;
                let mut guarded = encoder_policy::amf(&config, &stream)?;
                guard_smart_access_video(&mut guarded);
                assert_eq!(original.len(), guarded.len());
                for (before, after) in original.iter().zip(&guarded) {
                    assert_eq!(before.name, after.name);
                    if codec < 2
                        && low_latency == "enabled"
                        && sav == "enabled"
                        && before.name.ends_with("EnableEncoderSmartAccessVideo")
                    {
                        assert_eq!(after.value, encoder_policy::Value::Boolean(false));
                        assert!(after.required);
                    } else {
                        assert_eq!(before.value, after.value, "{}", before.name);
                        assert_eq!(before.required, after.required, "{}", before.name);
                    }
                }
            }
        }
    }
    Ok(())
}

#[test]
#[ignore = "requires AMD AMF and an independent FFprobe"]
fn hdr10_metadata_and_range_reach_the_bitstream() -> Result<()> {
    use crate::capture::{ComGuard, Pixel};
    use std::{os::windows::process::CommandExt, process::Command};
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let ffprobe =
        std::env::var_os("BUTTERPOLLO_TEST_FFPROBE").context("set BUTTERPOLLO_TEST_FFPROBE")?;
    let directory = tempfile::tempdir()?;
    let (width, height) = (1968u32, 2184u32);
    let pixels = (0..width * height)
        .flat_map(|index| {
            let value = half::f16::from_f32([0.05, 1., 6.][(index % width * 3 / width) as usize]);
            [value, value, value, half::f16::ONE]
        })
        .flat_map(|value| value.to_le_bytes())
        .collect();
    let source = GpuImage::upload(
        &gpu,
        &Image {
            width,
            height,
            stride: width as usize * 8,
            bytes: pixels,
            captured: Instant::now(),
            pixel: Pixel::RgbaF16,
        },
    )?;
    source.readback(&mut None)?;
    let mut metadata = butterpollo_core::hdr::Metadata::display(1015., 0.005, 400.);
    metadata.max_cll = 1000;
    metadata.max_fall = 400;
    let options = butterpollo_core::config::Config::parse(
        "amd_usage=ultralowlatency\namd_quality=speed\namd_smart_access_video=disabled\namd_lowlatency_mode=enabled\namd_input_queue_size=4\namd_ltr_frames=4\n",
    )?;
    let defaults = butterpollo_core::config::Config::default();
    for codec in [1u8, 2] {
        for full_range in [false, true] {
            for compute in [false, true] {
                for (references, metadata, options) in [
                    (1, metadata, &defaults),
                    (4, metadata, &defaults),
                    (
                        1,
                        butterpollo_core::hdr::Metadata::display(1015., 0., 1015.),
                        &options,
                    ),
                    (
                        4,
                        butterpollo_core::hdr::Metadata::display(1015., 0., 1015.),
                        &options,
                    ),
                    (4, metadata, &options),
                ] {
                    let config = butterpollo_core::rtsp::Negotiated {
                        width,
                        height,
                        codec,
                        hdr: true,
                        csc_mode: 4 | u8::from(full_range),
                        fps: 120,
                        bitrate_kbps: 80000,
                        references,
                        ..Default::default()
                    };
                    let queue = compute
                        .then(|| crate::compute::Compute::for_device(&gpu.device))
                        .transpose()?;
                    let mut encoder = Encoder::new_gpu(&config, gpu.clone(), options, queue)?;
                    assert_eq!(
                        encoder.supports_invalidation(),
                        // `references` sets the client's limit; whether LTR
                        // recovery is enabled also depends on the options.
                        butterpollo_core::encoder_policy::amf_ltr_frames(options, &config) > 0
                    );
                    let changes = [
                        metadata,
                        butterpollo_core::hdr::Metadata {
                            maximum_nits: 600,
                            max_cll: 0,
                            max_fall: 0,
                            ..metadata
                        },
                        butterpollo_core::hdr::Metadata {
                            maximum_nits: 1400,
                            max_cll: 1200,
                            max_fall: 500,
                            ..metadata
                        },
                    ];
                    let mut output = vec![];
                    for frame in 0..12 {
                        // Repeated refreshes must not keep inserting keyframes.
                        encoder.set_hdr_metadata(changes[frame / 4]);
                        output.extend(encoder.encode_gpu(
                            &source,
                            // Later keyframes must come from changed metadata,
                            // even with no unrelated recovery/client request.
                            frame == 0,
                            config.bitrate_kbps,
                        )?);
                        // Drain before changing a component-wide metadata property.
                        if frame % 4 != 3 {
                            continue;
                        }
                        let deadline = Instant::now() + Duration::from_secs(3);
                        while encoder.pending() {
                            output.extend(encoder.poll()?);
                            if Instant::now() >= deadline {
                                bail!("HDR metadata probe output timed out");
                            }
                            std::thread::sleep(Duration::from_millis(1));
                        }
                    }
                    assert_eq!(output.len(), 12);
                    for (frame, encoded) in output.iter().enumerate() {
                        assert_eq!(
                            encoded.idr,
                            frame % 4 == 0,
                            "unexpected keyframe at {frame}"
                        );
                    }
                    for frame in [0, 4, 8] {
                        let metadata = changes[frame / 4];
                        assert!(output[frame].idr);
                        let name = format!(
                            "codec{codec}-full{full_range}-compute{compute}-refs{references}-cll{}-defaults{}-frame{frame}",
                            metadata.max_cll,
                            options.values.is_empty()
                        );
                        let bitstream = directory.path().join(format!("{name}.bin"));
                        // Decode each IDR separately: a decoder retains earlier SEI.
                        // Moonlight's decode-unit dump strips HEVC prefix SEI, so inspect
                        // the encoder output before client depacketization.
                        std::fs::write(&bitstream, &output[frame].bytes)?;
                        let probed = output_within(
                            Command::new(&ffprobe)
                                .args([
                                    "-v",
                                    "error",
                                    "-f",
                                    if codec == 1 { "hevc" } else { "obu" },
                                    "-i",
                                ])
                                .arg(&bitstream)
                                .args(["-show_frames", "-of", "json"])
                                .creation_flags(0x08000000),
                        )?;
                        assert!(
                            probed.status.success(),
                            "{}",
                            String::from_utf8_lossy(&probed.stderr)
                        );
                        let probed: serde_json::Value = serde_json::from_slice(&probed.stdout)?;
                        assert_eq!(probed["frames"].as_array().map(Vec::len), Some(1), "{name}");
                        let first = &probed["frames"][0];
                        assert_eq!(first["key_frame"], 1, "{name}: {first}");
                        assert_eq!(
                            first["color_range"],
                            if full_range { "pc" } else { "tv" },
                            "{name}: {first}"
                        );
                        assert_eq!(first["color_transfer"], "smpte2084", "{name}: {first}");
                        // The matrix is left to the driver to derive from the colour
                        // profile; a client converting with BT.709 would shift colours.
                        assert_eq!(first["color_primaries"], "bt2020", "{name}: {first}");
                        assert_eq!(first["color_space"], "bt2020nc", "{name}: {first}");
                        let side = first["side_data_list"]
                            .as_array()
                            .with_context(|| format!("{name}: no side data in {first}"))?;
                        let mastering = side
                            .iter()
                            .find(|s| s["side_data_type"] == "Mastering display metadata")
                            .with_context(|| format!("{name}: no mastering display metadata"))?;
                        // Mastering peak and BT.2020 red (0.708) in each codec's units.
                        let (luminance, minimum, red) = if codec == 1 {
                            (
                                format!("{}/10000", u32::from(metadata.maximum_nits) * 10000),
                                format!("{}/10000", metadata.minimum),
                                "35400/50000",
                            )
                        } else {
                            (
                                format!("{}/256", u32::from(metadata.maximum_nits) * 256),
                                format!("{}/16384", u32::from(metadata.minimum) * 16384 / 10000),
                                "46399/65536",
                            )
                        };
                        assert_eq!(mastering["max_luminance"], luminance, "{name}: {mastering}");
                        assert_eq!(mastering["min_luminance"], minimum, "{name}: {mastering}");
                        assert_eq!(mastering["red_x"], red, "{name}: {mastering}");
                        let light = side
                            .iter()
                            .find(|s| s["side_data_type"] == "Content light level metadata");
                        if metadata.max_cll == 0 && frame == 0 {
                            assert!(light.is_none(), "{name}: unexpected content light level");
                        } else if metadata.max_cll == 0 {
                            // After a known value, AMF keeps the SEI/OBU and writes
                            // 0/0, which means unknown (CTA-861.3). Only a stale
                            // non-zero level would be wrong.
                            if let Some(light) = light {
                                assert_eq!(
                                    light["max_content"], 0,
                                    "{name}: stale content light level"
                                );
                                assert_eq!(
                                    light["max_average"], 0,
                                    "{name}: stale content light level"
                                );
                            }
                        } else {
                            let light =
                                light.with_context(|| format!("{name}: no content light level"))?;
                            assert_eq!(light["max_content"], metadata.max_cll, "{name}");
                            assert_eq!(light["max_average"], metadata.max_fall, "{name}");
                        }
                    }
                }
            }
        }
    }
    Ok(())
}
#[test]
fn hlg_and_dolby_vision_8_4_signal_arib_std_b67() {
    // rubylight_protocol::hdr: PQ 1, HLG 2, DV_81 4, DV_84 8.
    let stream = |codec, hdr, hdr_format, csc_mode| butterpollo_core::rtsp::Negotiated {
        codec,
        hdr,
        hdr_format,
        csc_mode,
        ..Default::default()
    };
    for codec in [1, 2] {
        assert_eq!(transfer_characteristic(&stream(codec, true, 2, 4)), 18);
        assert_eq!(transfer_characteristic(&stream(codec, true, 0, 4)), 16);
        assert_eq!(transfer_characteristic(&stream(codec, true, 1, 4)), 16);
        // HLG needs an HDR stream: SDR keeps its matrix's curve.
        assert_eq!(transfer_characteristic(&stream(codec, false, 2, 2)), 1);
    }
    assert_eq!(transfer_characteristic(&stream(1, true, 8, 4)), 18);
    assert_eq!(transfer_characteristic(&stream(1, true, 4, 4)), 16);
    assert_eq!(transfer_characteristic(&stream(0, false, 0, 0)), 6);
    assert_eq!(transfer_characteristic(&stream(0, false, 0, 2)), 1);
}
#[test]
#[ignore = "requires AMD AMF and an independent FFprobe"]
fn hlg_and_dolby_vision_8_4_reach_the_bitstream() -> Result<()> {
    use crate::capture::{ComGuard, Pixel};
    use std::{os::windows::process::CommandExt, process::Command};
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let ffprobe =
        std::env::var_os("BUTTERPOLLO_TEST_FFPROBE").context("set BUTTERPOLLO_TEST_FFPROBE")?;
    let directory = tempfile::tempdir()?;
    let (width, height) = (1920u32, 1080u32);
    let pixels = (0..width * height)
        .flat_map(|index| {
            let value =
                half::f16::from_f32([0.05, 2.5, 12.5][(index % width * 3 / width) as usize]);
            [value, value, value, half::f16::ONE]
        })
        .flat_map(|value| value.to_le_bytes())
        .collect();
    let source = GpuImage::upload(
        &gpu,
        &Image {
            width,
            height,
            stride: width as usize * 8,
            bytes: pixels,
            captured: Instant::now(),
            pixel: Pixel::RgbaF16,
        },
    )?;
    source.readback(&mut None)?;
    let options = butterpollo_core::config::Config::default();
    // rubylight_protocol::hdr: HLG 2, DV_84 8 (HEVC only).
    for (codec, hdr_format) in [(1u8, 2u8), (2, 2), (1, 8)] {
        for compute in [false, true] {
            let config = butterpollo_core::rtsp::Negotiated {
                width,
                height,
                codec,
                hdr: true,
                hdr_format,
                csc_mode: 4,
                fps: 60,
                bitrate_kbps: 40000,
                ..Default::default()
            };
            let queue = compute
                .then(|| crate::compute::Compute::for_device(&gpu.device))
                .transpose()?;
            let mut encoder = Encoder::new_gpu(&config, gpu.clone(), &options, queue)?;
            encoder.set_hdr_metadata(butterpollo_core::hdr::Metadata::display(1000., 0.005, 400.));
            let mut output = encoder.encode_gpu(&source, true, config.bitrate_kbps)?;
            let deadline = Instant::now() + Duration::from_secs(2);
            while encoder.pending() {
                output.extend(encoder.poll()?);
                if Instant::now() >= deadline {
                    bail!("HLG probe output timed out");
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            let name = format!("codec{codec}-format{hdr_format}-compute{compute}");
            let frame = output
                .first()
                .with_context(|| format!("{name}: no output"))?;
            assert!(frame.idr, "{name}");
            let bitstream = directory.path().join(format!("{name}.bin"));
            std::fs::write(&bitstream, &frame.bytes)?;
            let probed = output_within(
                Command::new(&ffprobe)
                    .args([
                        "-v",
                        "error",
                        "-f",
                        if codec == 1 { "hevc" } else { "obu" },
                        "-i",
                    ])
                    .arg(&bitstream)
                    .args(["-show_frames", "-of", "json"])
                    .creation_flags(0x08000000),
            )?;
            assert!(
                probed.status.success(),
                "{}",
                String::from_utf8_lossy(&probed.stderr)
            );
            let probed: serde_json::Value = serde_json::from_slice(&probed.stdout)?;
            let first = &probed["frames"][0];
            // VUI / sequence header transfer_characteristics 18.
            assert_eq!(first["color_transfer"], "arib-std-b67", "{name}: {first}");
            assert_eq!(first["color_primaries"], "bt2020", "{name}: {first}");
            assert_eq!(first["color_space"], "bt2020nc", "{name}: {first}");
            // Dolby Vision 8.4 ends the access unit with an RPU, NAL type 62;
            // plain HLG carries none. HEVC payloads never hold a start code.
            if codec == 1 {
                let rpu = frame
                    .bytes
                    .windows(4)
                    .any(|w| w[..3] == [0, 0, 1] && (w[3] >> 1) & 0x3f == 62);
                assert_eq!(rpu, hdr_format == 8, "{name}: RPU");
            }
        }
    }
    Ok(())
}
#[test]
#[ignore = "requires AMD AMF, independent FFprobe and an artifact directory"]
fn native_av1_geometry_and_hdr_are_preserved() -> Result<()> {
    use crate::capture::{ComGuard, Pixel};
    use std::{os::windows::process::CommandExt, path::PathBuf, process::Command};
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let directory = PathBuf::from(
        std::env::var_os("BUTTERPOLLO_TEST_GEOMETRY_DIRECTORY")
            .context("set BUTTERPOLLO_TEST_GEOMETRY_DIRECTORY")?,
    );
    std::fs::create_dir_all(&directory)?;
    let ffprobe =
        std::env::var_os("BUTTERPOLLO_TEST_FFPROBE").context("set BUTTERPOLLO_TEST_FFPROBE")?;
    let ffmpeg =
        std::env::var_os("BUTTERPOLLO_TEST_FFMPEG").context("set BUTTERPOLLO_TEST_FFMPEG")?;
    let options = butterpollo_core::config::Config::parse(
        "amd_usage=ultralowlatency\namd_quality=speed\namd_smart_access_video=disabled\n",
    )?;
    let mut reports = vec![];
    let mut exact = true;
    for (width, height) in [(1920, 1080), (1968, 2184), (2184, 1968)] {
        let mut pixels = Vec::with_capacity(width as usize * height as usize * 8);
        for y in 0..height {
            for x in 0..width {
                let value = [0.1, 0.5, 1., 4.]
                    [usize::from(x >= width / 2) + 2 * usize::from(y >= height / 2)];
                for value in [value, value, value, 1.] {
                    pixels.extend_from_slice(&half::f16::from_f32(value).to_le_bytes());
                }
            }
        }
        let source = GpuImage::upload(
            &gpu,
            &Image {
                width,
                height,
                stride: width as usize * 8,
                bytes: pixels,
                captured: Instant::now(),
                pixel: Pixel::RgbaF16,
            },
        )?;
        for hdr in [false, true] {
            let source = if hdr {
                source.clone()
            } else {
                let bytes = (0..width * height)
                    .flat_map(|index| {
                        let x = index % width;
                        let y = index / width;
                        let value = [25, 128, 255, 255]
                            [usize::from(x >= width / 2) + 2 * usize::from(y >= height / 2)];
                        [value, value, value, 255]
                    })
                    .collect();
                GpuImage::upload(
                    &gpu,
                    &Image {
                        width,
                        height,
                        stride: width as usize * 4,
                        bytes,
                        captured: Instant::now(),
                        pixel: Pixel::Bgra8,
                    },
                )?
            };
            for alignment in [3, 4] {
                let config = butterpollo_core::rtsp::Negotiated {
                    width,
                    height,
                    codec: 2,
                    hdr,
                    fps: 120,
                    bitrate_kbps: 80000,
                    ..Default::default()
                };
                let result = Encoder::new_device_alignment_options(
                    &config,
                    gpu.clone(),
                    &options,
                    alignment,
                );
                let mut encoder = match result {
                    Ok(encoder) => encoder,
                    Err(error) => {
                        reports.push(serde_json::json!({"width":width,"height":height,"hdr":hdr,"alignment":alignment,"init_error":format!("{error:#}")}));
                        exact &= alignment != 3;
                        continue;
                    }
                };
                let read = |name: &str| -> Result<AMFVariantStruct> {
                    let mut value = int(0);
                    // SAFETY: `encoder.component` is the live AMF component the encoder owns; the
                    // name and `value` outlive the call.
                    unsafe {
                        check(((*(*encoder.component).pVtbl).GetProperty.unwrap())(
                            encoder.component,
                            to_wide(name).as_ptr(),
                            &mut value,
                        ))?;
                    }
                    Ok(value)
                };
                let applied_alignment = read("Av1AlignmentMode")?;
                let applied_size = read("Av1FrameSize")?;
                // SAFETY: Every AMFVariant union member read here is plain integers, valid for any
                // bit pattern.
                let properties = unsafe {
                    serde_json::json!({
                        "alignment_variant":applied_alignment.type_,
                        "alignment":applied_alignment.__bindgen_anon_1.int64Value,
                        "frame_size_variant":applied_size.type_,
                        "width":applied_size.__bindgen_anon_1.sizeValue.width,
                        "height":applied_size.__bindgen_anon_1.sizeValue.height,
                    })
                };
                let mut output = vec![];
                for frame in 0..8 {
                    output.extend(encoder.encode_gpu(&source, frame == 0, config.bitrate_kbps)?);
                }
                let deadline = Instant::now() + Duration::from_secs(3);
                while encoder.pending() {
                    output.extend(encoder.poll()?);
                    if Instant::now() >= deadline {
                        bail!("AV1 geometry probe output timed out");
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
                assert_eq!(output.len(), 8);
                let name = format!("av1-{width}x{height}-hdr{hdr}-align{alignment}");
                let bitstream = directory.join(format!("{name}.obu"));
                std::fs::write(
                    &bitstream,
                    output
                        .iter()
                        .flat_map(|packet| packet.bytes.iter().copied())
                        .collect::<Vec<_>>(),
                )?;
                let decoded = output_within(
                    Command::new(&ffprobe)
                        .args(["-v", "error", "-f", "obu", "-i"])
                        .arg(&bitstream)
                        .args(["-show_frames", "-show_streams", "-of", "json"])
                        .creation_flags(0x08000000),
                )?;
                assert!(
                    decoded.status.success(),
                    "{}",
                    String::from_utf8_lossy(&decoded.stderr)
                );
                let decoded: serde_json::Value = serde_json::from_slice(&decoded.stdout)?;
                std::fs::write(
                    directory.join(format!("{name}.decoded.json")),
                    serde_json::to_vec_pretty(&decoded)?,
                )?;
                let frames = decoded["frames"]
                    .as_array()
                    .context("no independently decoded AV1 frames")?;
                let matches = frames.len() == 8
                    && frames.iter().all(|frame| {
                        frame["width"] == width
                            && frame["height"] == height
                            && (!hdr
                                || (frame["color_primaries"] == "bt2020"
                                    && frame["color_transfer"] == "smpte2084"))
                    });
                exact &= alignment != 3 || matches;
                let trace = output_within(
                    Command::new(&ffmpeg)
                        .args(["-hide_banner", "-nostdin", "-f", "obu", "-i"])
                        .arg(&bitstream)
                        .args([
                            "-map",
                            "0:v",
                            "-frames:v",
                            "1",
                            "-c:v",
                            "copy",
                            "-bsf:v",
                            "trace_headers",
                            "-f",
                            "null",
                            "-",
                        ])
                        .creation_flags(0x08000000),
                )?;
                assert!(trace.status.success(), "bitstream trace failed");
                std::fs::write(directory.join(format!("{name}.headers.log")), trace.stderr)?;
                reports.push(serde_json::json!({"width":width,"height":height,"hdr":hdr,"alignment":alignment,
                    "properties":properties,"exact_decoded_geometry_and_hdr":matches,"first_frame":frames.first()}));
            }
        }
    }
    std::fs::write(
        directory.join("geometry.json"),
        serde_json::to_vec_pretty(&reports)?,
    )?;
    assert!(
        exact,
        "AV1 exact decoded geometry failed; inspect geometry.json and encoded headers"
    );
    Ok(())
}

#[test]
#[ignore = "requires AMD AMF, independent FFmpeg and a reference-budget report path"]
fn native_h264_one_reference_budget_decodes_without_ltr_corruption() -> Result<()> {
    use crate::capture::{ComGuard, Pixel};
    use std::{os::windows::process::CommandExt, path::PathBuf, process::Command};
    let _com = ComGuard::new()?;
    let gpu = Device::new("")?;
    let decoder = std::env::var_os("BUTTERPOLLO_TEST_FFMPEG")
        .context("set BUTTERPOLLO_TEST_FFMPEG to an independent decoder")?;
    let report = PathBuf::from(
        std::env::var_os("BUTTERPOLLO_TEST_REFERENCE_BUDGET_REPORT")
            .context("set BUTTERPOLLO_TEST_REFERENCE_BUDGET_REPORT")?,
    );
    let directory = report
        .parent()
        .context("report requires a parent directory")?;
    std::fs::create_dir_all(directory)?;
    let config = butterpollo_core::rtsp::Negotiated {
        width: 640,
        height: 480,
        references: 1,
        ..Default::default()
    };
    let options = butterpollo_core::config::Config::parse(
        "amd_ltr_frames=4\namd_usage=ultralowlatency\namd_quality=speed\namd_lowlatency_mode=enabled\n",
    )?;
    // Synthetic input avoids capturing or changing any user's desktop/window.
    let source = GpuImage::upload(
        &gpu,
        &Image {
            width: config.width,
            height: config.height,
            stride: config.width as usize * 4,
            bytes: (0..config.width * config.height)
                .flat_map(|index| {
                    let x = index % config.width;
                    let y = index / config.width;
                    [x as u8, y as u8, (x ^ y) as u8, 255]
                })
                .collect(),
            captured: Instant::now(),
            pixel: Pixel::Bgra8,
        },
    )?;
    let mut encoder = Encoder::new_device_options(&config, gpu, &options)?;
    let supports_invalidation = encoder.supports_invalidation();
    let mut output = vec![];
    for frame in 0..64 {
        output.extend(encoder.encode_gpu(&source, frame == 0, config.bitrate_kbps)?);
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    while encoder.pending() {
        output.extend(encoder.poll()?);
        if Instant::now() > deadline {
            bail!("AMF reference-budget fixture output timed out");
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let bitstream = directory.join(format!("amf-one-reference-{}.h264", std::process::id()));
    std::fs::write(
        &bitstream,
        output
            .iter()
            .flat_map(|packet| packet.bytes.iter().copied())
            .collect::<Vec<_>>(),
    )?;
    // Inspect the encoder's actual SPS independently. A software-only decode
    // can pass an oversized SPS that Moonlight's one-reference fixup would reject.
    let trace = output_within(
        Command::new(&decoder)
            .args(["-hide_banner", "-nostdin", "-f", "h264", "-i"])
            .arg(&bitstream)
            .args([
                "-map",
                "0:v",
                "-c:v",
                "copy",
                "-bsf:v",
                "trace_headers",
                "-f",
                "null",
                "-",
            ])
            .creation_flags(0x08000000),
    )?;
    let headers = String::from_utf8_lossy(&trace.stderr);
    let values = |field: &str| -> Vec<u32> {
        headers
            .lines()
            .filter(|line| line.contains(field))
            .filter_map(|line| line.rsplit_once('=')?.1.trim().parse().ok())
            .collect()
    };
    let max_num_ref_frames = values("max_num_ref_frames");
    let max_dec_frame_buffering = values("max_dec_frame_buffering");
    std::fs::write(bitstream.with_extension("headers.log"), &trace.stderr)?;
    let decoded = output_within(
        Command::new(&decoder)
            .args([
                "-hide_banner",
                "-nostdin",
                "-v",
                "error",
                "-xerror",
                "-err_detect",
                "explode",
                "-f",
                "h264",
                "-i",
            ])
            .arg(&bitstream)
            .args([
                "-an",
                "-progress",
                "pipe:1",
                "-nostats",
                "-fps_mode",
                "passthrough",
                "-f",
                "null",
                "-",
            ])
            .creation_flags(0x08000000),
    )?;
    std::fs::write(bitstream.with_extension("decode.log"), &decoded.stderr)?;
    let decoded_frames = String::from_utf8_lossy(&decoded.stdout)
        .lines()
        .filter_map(|line| line.strip_prefix("frame=")?.trim().parse::<u32>().ok())
        .next_back()
        .unwrap_or(0);
    let observation = serde_json::json!({
        "negotiated_references": config.references,
        "requested_ltr_frames": 4,
        "supports_invalidation": supports_invalidation,
        "applied_max_num_ref_frames": encoder.read("MaxNumRefFrames"),
        "applied_max_ltr_frames": encoder.read("MaxOfLTRFrames"),
        "encoded_frames": output.len(),
        "decoded_frames": decoded_frames,
        "sps_max_num_ref_frames": max_num_ref_frames,
        "vui_max_dec_frame_buffering": max_dec_frame_buffering,
        "header_trace_success": trace.status.success(),
        "decode_success": decoded.status.success(),
        "decode_errors": String::from_utf8_lossy(&decoded.stderr),
        "bitstream": bitstream,
    });
    std::fs::write(&report, serde_json::to_vec_pretty(&observation)?)?;
    eprintln!("{observation}");
    assert!(
        !supports_invalidation,
        "one-reference clients must use IDR recovery"
    );
    assert_eq!(output.len(), 64);
    assert!(trace.status.success(), "independent SPS trace failed");
    assert!(
        !max_num_ref_frames.is_empty(),
        "trace contained no SPS reference count"
    );
    assert!(
        max_num_ref_frames.iter().all(|&count| count <= 1),
        "oversized SPS reference budget"
    );
    assert!(
        max_dec_frame_buffering.iter().all(|&count| count <= 1),
        "oversized VUI decoder buffer budget"
    );
    assert!(
        decoded.status.success() && decoded.stderr.is_empty(),
        "independent decoder rejected stream"
    );
    assert_eq!(decoded_frames, 64);
    Ok(())
}

#[test]
#[ignore = "requires a live AMD D3D11 adapter and AMF runtime"]
fn native_reference_recovery_decodes_after_dropping_the_invalidated_packets() -> Result<()> {
    let _com = crate::capture::ComGuard::new()?;
    let mut capture = crate::capture::Capture::new_format("", "wgc", false)?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let image = loop {
        if let Some(image) = capture.next_gpu()? {
            break image;
        }
        if Instant::now() > deadline {
            bail!("capture fixture timed out");
        }
        std::thread::sleep(Duration::from_millis(1));
    };
    let options = butterpollo_core::config::Config::parse("amd_ltr_frames=4\n")?;
    let mut reports = vec![];
    for codec in 0..3 {
        let config = butterpollo_core::rtsp::Negotiated {
            width: 640,
            height: 480,
            codec,
            ..Default::default()
        };
        let mut encoder = Encoder::new_device_options(&config, image.gpu.clone(), &options)?;
        let supported = encoder.supports_invalidation();
        let mut output = vec![];
        for frame in 1..=64 {
            let mut idr = frame == 1;
            if frame == 9 && supported {
                assert!(encoder.invalidate_ref_frames(5, 8));
            }
            if frame == 21 && supported {
                idr = !encoder.invalidate_ref_frames(17, 20);
                assert_eq!(idr, codec == 0);
            }
            output.extend(encoder.encode_gpu(&image, idr, config.bitrate_kbps)?);
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while encoder.pending() {
            output.extend(encoder.poll()?);
            if Instant::now() > deadline {
                bail!("AMF output fixture timed out");
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(output.len(), 64);
        if supported {
            assert!(output[8].after_invalidation);
            assert!(!output[8].idr);
            assert_eq!(output[20].after_invalidation, codec != 0);
            assert_eq!(output[20].idr, codec == 0);
        }
        use std::os::windows::process::CommandExt;
        let decoder = std::env::var_os("BUTTERPOLLO_TEST_FFMPEG")
            .context("set BUTTERPOLLO_TEST_FFMPEG to an independent decoder")?;
        let report = std::path::PathBuf::from(
            std::env::var_os("BUTTERPOLLO_TEST_RFI_REPORT")
                .context("set BUTTERPOLLO_TEST_RFI_REPORT")?,
        );
        let directory = report
            .parent()
            .context("report requires a parent directory")?;
        let format = match codec {
            0 => "h264",
            1 => "hevc",
            _ => "obu",
        };
        let path = directory.join(format!("amf-rfi-{}-{codec}.{format}", std::process::id()));
        let bitstream: Vec<_> = output
            .iter()
            .enumerate()
            .filter(|(index, _)| {
                !supported || !((4..8).contains(index) || (16..20).contains(index))
            })
            .flat_map(|(_, packet)| packet.bytes.iter().copied())
            .collect();
        std::fs::write(&path, bitstream)?;
        std::fs::write(
            path.with_extension(format!("full.{format}")),
            output
                .iter()
                .flat_map(|packet| packet.bytes.iter().copied())
                .collect::<Vec<_>>(),
        )?;
        eprintln!(
            "codec={codec}, supported={supported}, fixture={}",
            path.display()
        );
        let decoded_output = output_within(
            std::process::Command::new(&decoder)
                .args([
                    "-hide_banner",
                    "-nostdin",
                    "-v",
                    "error",
                    "-xerror",
                    "-err_detect",
                    "explode",
                    "-f",
                    format,
                    "-i",
                ])
                .arg(&path)
                .args([
                    "-an",
                    "-progress",
                    "pipe:1",
                    "-nostats",
                    "-fps_mode",
                    "passthrough",
                    "-f",
                    "null",
                    "-",
                ])
                .creation_flags(0x08000000),
        )?;
        let stderr = String::from_utf8_lossy(&decoded_output.stderr);
        assert!(
            decoded_output.status.success() && stderr.is_empty(),
            "codec={codec}, {stderr}"
        );
        let decoded = String::from_utf8_lossy(&decoded_output.stdout)
            .lines()
            .filter_map(|line| line.strip_prefix("frame=")?.trim().parse::<u32>().ok())
            .next_back()
            .context("decoder reported no frames")?;
        assert_eq!(decoded, if supported { 56 } else { 64 });
        reports.push(serde_json::json!({"codec":codec,"frames":output.len(),"decoded":decoded,"ltr_supported":supported,"recovery_frames":output.iter().filter(|frame|frame.after_invalidation).count(),"wrap_idr_fallback":supported&&codec==0,"dropped_frames":if supported{8}else{0}}));
    }
    if let Some(path) = std::env::var_os("BUTTERPOLLO_TEST_RFI_REPORT") {
        std::fs::write(path, serde_json::to_vec_pretty(&reports)?)?;
    }
    eprintln!("{}", serde_json::to_string(&reports)?);
    Ok(())
}

#[test]
fn amf_errors_name_the_result_and_say_when_the_hardware_cannot() {
    let text = super::amf_error_text(AMF_RESULT_AMF_NOT_SUPPORTED);
    assert!(text.starts_with("AMF error AMF_NOT_SUPPORTED ("), "{text}");
    assert!(text.contains("no encoder for the request"), "{text}");
    let text = super::amf_error_text(AMF_RESULT_AMF_ENCODER_NOT_PRESENT);
    assert!(text.contains("AMF_ENCODER_NOT_PRESENT"), "{text}");
    assert!(text.contains("no encoder for the request"), "{text}");
    // A code without a name keeps the number, as before.
    assert_eq!(super::amf_error_text(9999), "AMF error 9999");
    assert_eq!(
        super::check(AMF_RESULT_AMF_FAIL).unwrap_err().to_string(),
        "AMF error AMF_FAIL (1)"
    );
    assert!(super::check(AMF_RESULT_AMF_OK).is_ok());
}

#[test]
fn only_an_inter_frame_past_the_full_fec_bound_is_encoded_again_once() {
    // The October 28 run: a scene cut after steady 380 KB frames came out as
    // a 2.4 MB inter frame against a 1.17 MB bound and went without parity.
    let limit = Some(1_170_000);
    assert!(reencodes_cut(false, 2_400_000, limit, true));
    // A frame within the bound, a requested keyframe, the re-encode itself
    // (also a keyframe, so never a second time) and a stream without the
    // bound go as AMF made them.
    assert!(!reencodes_cut(false, 1_170_000, limit, true));
    assert!(!reencodes_cut(false, 380_000, limit, true));
    assert!(!reencodes_cut(true, 2_400_000, limit, true));
    assert!(!reencodes_cut(false, 2_400_000, None, true));
    // A later submission already references the frame, or one is being
    // prepared against the references as they are: it stays.
    assert!(!reencodes_cut(false, 2_400_000, limit, false));
}

#[test]
fn a_keyframe_is_over_its_cap_past_one_tenth_more() {
    // #59 rc.4 run 1: a 1,908,217 B recovery keyframe against a 1,166,893 B cap.
    assert!(over_cap_target(1_908_217, Some(1_166_893)));
    assert!(over_cap_target(1_100_001, Some(1_000_000)));
    assert!(!over_cap_target(1_100_000, Some(1_000_000)));
    assert!(!over_cap_target(400_000, Some(1_000_000)));
    // No target, or the driver's unrestricted one: nothing to be over.
    assert!(!over_cap_target(1_908_217, None));
    assert!(!over_cap_target(1_908_217, Some(0)));
}

#[test]
fn only_an_automatic_cap_is_a_keyframe_target() {
    assert_eq!(frame_cap_target(true, 1_166_893), Some(1_166_893));
    assert_eq!(frame_cap_target(true, 0), None);
    // An explicit amd_max_frame_size, or driver default: nothing to be over.
    assert_eq!(frame_cap_target(false, 1_166_893), None);
    assert!(!over_cap_target(
        1_908_217,
        frame_cap_target(false, 1_166_893)
    ));
}

#[test]
#[ignore = "requires a live AMD D3D11 adapter and AMF runtime"]
fn native_an_oversize_scene_cut_goes_out_once_as_a_keyframe_at_its_own_index() -> Result<()> {
    let _com = crate::capture::ComGuard::new()?;
    let device = Device::new("")?;
    let options = butterpollo_core::config::Config::parse("amd_ltr_frames=4\n")?;
    for codec in 0..3 {
        let config = butterpollo_core::rtsp::Negotiated {
            width: 1280,
            height: 720,
            codec,
            ..Default::default()
        };
        let image = |noise: bool| {
            let mut seed = 0x2545_f491_4f6c_dd1d_u64;
            let bytes = (0..config.width as usize * config.height as usize * 4)
                .map(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    if noise { seed as u8 } else { 128 }
                })
                .collect();
            GpuImage::upload(
                &device,
                &Image {
                    width: config.width,
                    height: config.height,
                    stride: config.width as usize * 4,
                    bytes,
                    captured: Instant::now(),
                    pixel: crate::capture::Pixel::Bgra8,
                },
            )
        };
        let (flat, noise) = (image(false)?, image(true)?);
        let mut encoder = Encoder::new_device_options(&config, device.clone(), &options)?;
        assert!(encoder.supports_invalidation(), "codec {codec}: LTR");
        // Flat frames are a few hundred bytes; the cut to noise is far past it.
        encoder.set_cut_limit(100_000);
        let mut output = vec![];
        for frame in 1..=30 {
            let image = if frame < 21 { &flat } else { &noise };
            output.extend(encoder.encode_gpu(image, frame == 1, config.bitrate_kbps)?);
            let deadline = Instant::now() + Duration::from_secs(2);
            while encoder.pending() && Instant::now() < deadline {
                output.extend(encoder.poll()?);
            }
        }
        // One output a frame, the cut a keyframe in place of its inter frame,
        // and the static noise after it inter frames that reference it.
        let keyframes: Vec<_> = output
            .iter()
            .enumerate()
            .filter(|(_, frame)| frame.idr)
            .map(|(index, _)| index + 1)
            .collect();
        assert_eq!(output.len(), 30, "codec {codec}");
        assert_eq!(keyframes, [1, 21], "codec {codec}");
        assert_eq!(encoder.reencoded_cuts(), 1, "codec {codec}");
        // The keyframe is held to the frame cap, as a requested one is.
        let sizes: Vec<_> = output[19..].iter().map(|frame| frame.bytes.len()).collect();
        println!(
            "codec {codec}: frames 20 to 30 {sizes:?}, cap {:?}",
            encoder.frame_cap()
        );
        assert!(output[21..].iter().all(|frame| frame.bytes.len() < 100_000));
        // A loss after the cut is repaired from a reference since the keyframe
        // (whatever it costs: noise makes a repair as large as a keyframe).
        encoder.references.repair_whatever_it_costs();
        assert!(encoder.invalidate_ref_frames(25, 26), "codec {codec}");
        assert!(!encoder.recover);
    }
    Ok(())
}
