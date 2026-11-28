//! Fake-driver unit tests for NVENC optimizations N1–N6:
//! async output events, unconditional `doNotWait`, VBV initial delay and
//! coalesced bitrate/cap reconfiguration, CUDA IO stream binding and skipped
//! reencode copy, pre-drain RFI fast paths, and dynamic HDR headroom.

use super::*;
use butterpollo_win_abi::cuda::CUstream;

pub(super) unsafe extern "C" fn set_io_cuda_streams(
    _raw: *mut c_void,
    input: NV_ENC_CUSTREAM_PTR,
    output: NV_ENC_CUSTREAM_PTR,
) -> NVENCSTATUS {
    assert!(!input.is_null(), "inputStream must be a non-null CUstream*");
    assert!(
        !output.is_null(),
        "outputStream must be a non-null CUstream*"
    );
    // SAFETY: Caller passes pointers to a live `CUstream` field (`nvEncodeAPI.h:116,3532`).
    let (in_stream, out_stream) =
        unsafe { (*input.cast::<CUstream>(), *output.cast::<CUstream>()) };
    state(|d| {
        assert!(
            d.initialized.is_some(),
            "nvEncSetIOCudaStreams must run after nvEncInitializeEncoder"
        );
        d.calls.push("set_io_cuda_streams");
        d.cuda_streams
            .push((in_stream as usize, out_stream as usize));
        if d.reject_cuda_streams {
            _NVENCSTATUS_NV_ENC_ERR_UNSUPPORTED_PARAM
        } else {
            SUCCESS
        }
    })
}

fn open_cuda(config: &Negotiated, ctx: Rc<cuda::Context>) -> Result<Session> {
    let tuning = Tuning::new(&Config::default(), config)?;
    Session::open(
        Arc::new(Runtime {
            _library: None,
            create,
            maximum: ApiVersion(13, 0),
        }),
        ctx.raw(),
        _NV_ENC_DEVICE_TYPE_NV_ENC_DEVICE_TYPE_CUDA,
        Some(ctx),
        None,
        config,
        &tuning,
    )
}

#[test]
fn async_output_event_resets_on_reuse_and_avoids_lock_busy_spin() -> Result<()> {
    reset();
    state(|d| {
        d.asynchronous = true;
        d.hold_event = true;
    });
    let mut encoder = session(&Negotiated::default(), ApiVersion(13, 0))?;
    assert!(encoder.async_output_event().is_none());

    let slot = input(&mut encoder, 11)?;
    encoder.submit(slot, None, true, Instant::now())?;
    let event = encoder
        .async_output_event()
        .expect("async pending frame exposes completion event");
    assert!(
        !event.is_set(),
        "event starts unsignaled before driver sets it"
    );

    // Simulate GPU completion followed by a transient LOCK_BUSY: once `collect`
    // observes the signaled event, `async_output_event` returns `None` so the
    // caller does not busy-spin on an already-set manual-reset handle.
    event.set()?;
    assert!(event.is_set());
    state(|d| d.lock_busy = 1);
    encoder.collect()?;
    assert_eq!(encoder.pending.len(), 1);
    assert!(
        encoder.async_output_event().is_none(),
        "already-signaled slot must not re-wake caller on LOCK_BUSY"
    );

    let frames = encoder.poll()?;
    assert_eq!(frames.len(), 1);
    assert!(encoder.async_output_event().is_none());

    // Reusing the same slot must reset its manual-reset event before encode.
    encoder.submit(slot, None, false, Instant::now())?;
    let event = encoder
        .async_output_event()
        .expect("reused async slot exposes completion event");
    assert!(
        !event.is_set(),
        "manual-reset slot event must be reset before resubmission"
    );
    event.set()?;
    assert_eq!(encoder.poll()?.len(), 1);

    drop(encoder);
    assert_clean();
    Ok(())
}

#[test]
fn lock_bitstream_sets_do_not_wait_on_sync_sessions() -> Result<()> {
    reset();
    state(|d| {
        d.asynchronous = false;
        d.lock_busy = 1;
    });
    let mut encoder = session(&Negotiated::default(), ApiVersion(13, 0))?;
    assert_eq!(encoder.initialize.enableEncodeAsync, 0);
    assert!(!encoder.subframe);

    let slot = input(&mut encoder, 22)?;
    encoder.submit(slot, None, true, Instant::now())?;
    assert!(encoder.poll()?.is_empty(), "LOCK_BUSY keeps frame pending");
    assert_eq!(encoder.poll()?.len(), 1);
    state(|d| {
        assert_eq!(d.lock_do_not_wait, vec![1, 1]);
    });

    drop(encoder);
    assert_clean();
    Ok(())
}

#[test]
fn vbv_initial_delay_matches_vbv_buffer_size_and_coalesces_rate_and_cap() -> Result<()> {
    reset();
    let mut encoder = session(&Negotiated::default(), ApiVersion(13, 0))?;
    let initial_vbv = encoder.config.rcParams.vbvBufferSize;
    assert!(initial_vbv > 0);
    assert_eq!(
        initialized().1.rcParams.vbvInitialDelay,
        initial_vbv,
        "configure must set vbvInitialDelay == vbvBufferSize"
    );

    // Lowering frame cap directly via `set_frame_cap` keeps vbvInitialDelay == vbvBufferSize.
    let frame = encoder.frame_cap().unwrap();
    encoder.set_frame_cap(frame / 2)?;
    state(|d| {
        let (_, cfg) = d.reconfigured.take().unwrap();
        assert_eq!(cfg.rcParams.vbvInitialDelay, cfg.rcParams.vbvBufferSize);
        assert!(cfg.rcParams.vbvBufferSize < initial_vbv);
        d.reconfigure_count = 0;
    });

    // Coalesced path: `queue_frame_cap` performs zero driver calls, and
    // `apply_bitrate(rate, true)` applies both new rate and frame cap in ONE reconfigure.
    // Pick a cap strictly between `MIN_VBV_BITS` (100_000 bits) and the 10 Mbps
    // one-frame ceiling (~166_666 bits) so clamping neither hides the cap nor floors it.
    let new_cap = 16_000;
    assert!(new_cap * 8 > MIN_VBV_BITS && new_cap * 8 < u64::from(initial_vbv / 2));
    let _ = encoder.queue_frame_cap(new_cap, 0);
    state(|d| assert_eq!(d.reconfigure_count, 0));

    let cap_result = encoder.apply_bitrate(10_000, true)?;
    assert!(cap_result.is_ok());
    state(|d| {
        assert_eq!(
            d.reconfigure_count, 1,
            "bitrate + frame cap must coalesce into one nvEncReconfigureEncoder call"
        );
        let (_, cfg) = d.reconfigured.take().unwrap();
        assert_eq!(cfg.rcParams.averageBitRate, 10_000 * 1000);
        assert_eq!(cfg.rcParams.vbvBufferSize, (new_cap * 8) as u32);
        assert_eq!(cfg.rcParams.vbvInitialDelay, cfg.rcParams.vbvBufferSize);
    });

    // Repeating the same bitrate and cap produces zero further reconfigures.
    let _ = encoder.queue_frame_cap(new_cap, 0);
    assert!(encoder.apply_bitrate(10_000, true)?.is_ok());
    state(|d| assert_eq!(d.reconfigure_count, 1));

    drop(encoder);
    assert_clean();
    Ok(())
}

#[test]
fn cuda_session_binds_io_streams_after_init_and_skips_reencode_copy() -> Result<()> {
    reset();
    cuda::tests::reset();
    let config = Negotiated {
        codec: 1,
        hdr: true,
        yuv444: true,
        ..Negotiated::default()
    };
    let ctx = cuda::tests::fake_context(true);
    let mut encoder = open_cuda(&config, ctx.clone())?;

    state(|d| {
        assert!(
            d.calls.contains(&"set_io_cuda_streams"),
            "nvEncSetIOCudaStreams called"
        );
        assert_eq!(
            d.cuda_streams,
            vec![(cuda::tests::FAKE_STREAM, cuda::tests::FAKE_STREAM)]
        );
    });

    // Submit a frame with a CUDA input and force a scene-cut reencode on the
    // second picture: `reencode` must skip the redundant ~50 MB CUDA copy.
    let cuda_in = cuda::tests::fake_input(&ctx, 0x7777)?;
    let slot = encoder.slot(Input {
        key: 0x7777,
        raw: cuda_in.pointer as usize as *mut c_void,
        kind: _NV_ENC_INPUT_RESOURCE_TYPE_NV_ENC_INPUT_RESOURCE_TYPE_CUDADEVICEPTR,
        pitch: cuda_in.pitch,
        _texture: None,
        cuda: Some(cuda_in),
        d3d12: None,
    })?;
    encoder.submit(slot, None, true, Instant::now())?;
    assert_eq!(encoder.poll()?.len(), 1);

    encoder.cut_limit = Some(100);
    state(|d| d.picture = 500);
    encoder.submit(slot, None, false, Instant::now())?;
    let out = encoder.poll()?;
    assert_eq!(out.len(), 1);
    assert!(out[0].reencoded);
    assert_eq!(encoder.reencoded_cuts, 1);

    // Total copies for 2 frames (1 IDR + 1 P reencoded as IDR) must be 2, not 3!
    cuda::tests::with_fake(|f| {
        assert_eq!(
            f.async_copy_streams,
            vec![cuda::tests::FAKE_STREAM, cuda::tests::FAKE_STREAM],
            "reencode must skip redundant CUDA memcpy"
        );
        assert_eq!(f.sync_copies, 0);
    });

    drop(encoder);
    drop(ctx);
    assert_clean();
    Ok(())
}

#[test]
fn cuda_session_falls_back_to_default_stream_when_driver_refuses_io_streams() -> Result<()> {
    reset();
    cuda::tests::reset();
    state(|d| d.reject_cuda_streams = true);
    let config = Negotiated {
        codec: 1,
        hdr: true,
        yuv444: true,
        ..Negotiated::default()
    };
    let ctx = cuda::tests::fake_context(true);
    let mut encoder = open_cuda(&config, ctx.clone())?;
    let cuda_in = cuda::tests::fake_input(&ctx, 0x8888)?;
    let slot = encoder.slot(Input {
        key: 0x8888,
        raw: cuda_in.pointer as usize as *mut c_void,
        kind: _NV_ENC_INPUT_RESOURCE_TYPE_NV_ENC_INPUT_RESOURCE_TYPE_CUDADEVICEPTR,
        pitch: cuda_in.pitch,
        _texture: None,
        cuda: Some(cuda_in),
        d3d12: None,
    })?;
    encoder.submit(slot, None, true, Instant::now())?;
    assert_eq!(encoder.poll()?.len(), 1);
    cuda::tests::with_fake(|f| {
        assert_eq!(f.map_streams, vec![0]);
        assert_eq!(f.sync_copies, 1);
        assert!(f.async_copy_streams.is_empty());
    });
    drop(encoder);
    drop(ctx);
    assert_clean();
    Ok(())
}

#[test]
fn rfi_fast_paths_skip_drain_without_falsely_rejecting_pending_frames() -> Result<()> {
    reset();
    state(|d| {
        d.asynchronous = true;
        d.hold_event = true;
    });
    let mut encoder = session(&Negotiated::default(), ApiVersion(13, 0))?;
    let slot = input(&mut encoder, 33)?;

    // Complete frame 1 (IDR) and frame 2 (P).
    encoder.submit(slot, None, true, Instant::now())?;
    encoder.slots[slot].event.as_ref().unwrap().set()?;
    assert_eq!(encoder.poll()?.len(), 1);

    encoder.submit(slot, None, false, Instant::now())?;
    encoder.slots[slot].event.as_ref().unwrap().set()?;
    assert_eq!(encoder.poll()?.len(), 1);

    // Leave frame 3 pending (unsignaled) so any unnecessary drain would fail/block.
    encoder.submit(slot, None, false, Instant::now())?;
    assert_eq!(encoder.pending.len(), 1);

    // Fast-path rejections must not drain frame 3.
    assert!(!encoder.invalidate(0, 1)?);
    assert!(!encoder.invalidate(3, 2)?);
    assert!(!encoder.invalidate(4, 4)?);
    assert!(!encoder.invalidate(1, 2)?);
    assert_eq!(encoder.pending.len(), 1);

    // Signal frame 3: requesting RFI for frame 3 (which is in `pending`, not yet
    // in `references.last`) must drain and invalidate it rather than falsely rejecting.
    encoder.slots[slot].event.as_ref().unwrap().set()?;
    assert!(encoder.invalidate(3, 3)?);
    assert!(encoder.pending.is_empty());
    state(|d| assert_eq!(d.invalidated, vec![3]));

    // With frame 4 held pending, re-requesting the already-covered range [3, 3]
    // succeeds immediately without draining frame 4.
    encoder.submit(slot, None, false, Instant::now())?;
    assert_eq!(encoder.pending.len(), 1);
    assert!(encoder.invalidate(3, 3)?);
    assert_eq!(encoder.pending.len(), 1);

    // Complete frame 4 and an IDR on frame 5, then hold frame 6 pending:
    // old-GOP feedback [2, 3] succeeds without draining frame 6.
    encoder.slots[slot].event.as_ref().unwrap().set()?;
    assert_eq!(encoder.poll()?.len(), 2); // frames 3 (already in ready) + 4
    encoder.submit(slot, None, true, Instant::now())?;
    encoder.slots[slot].event.as_ref().unwrap().set()?;
    assert_eq!(encoder.poll()?.len(), 1);

    encoder.submit(slot, None, false, Instant::now())?;
    assert_eq!(encoder.pending.len(), 1);
    assert!(encoder.invalidate(2, 3)?);
    assert_eq!(encoder.pending.len(), 1);

    encoder.slots[slot].event.as_ref().unwrap().set()?;
    assert_eq!(encoder.poll()?.len(), 1);
    drop(encoder);
    assert_clean();
    Ok(())
}

#[test]
fn dynamic_headroom_reserves_capacity_for_hdr_metadata_splicing() -> Result<()> {
    reset();
    let mut encoder = session(&Negotiated::default(), ApiVersion(13, 0))?;
    encoder.dynamic_headroom = DYNAMIC_HEADROOM;
    let slot = input(&mut encoder, 44)?;
    encoder.submit(slot, None, true, Instant::now())?;
    let frames = encoder.poll()?;
    assert_eq!(frames.len(), 1);
    assert!(
        frames[0].bytes.capacity() >= frames[0].bytes.len() + DYNAMIC_HEADROOM,
        "capacity {} must leave at least {DYNAMIC_HEADROOM} bytes of headroom over len {}",
        frames[0].bytes.capacity(),
        frames[0].bytes.len()
    );
    drop(encoder);
    assert_clean();
    Ok(())
}

#[test]
fn vbv_ceiling_recomputes_from_tuning_without_ratcheting_across_bitrate_dips() -> Result<()> {
    reset();
    let mut encoder = session(&Negotiated::default(), ApiVersion(13, 0))?;
    let initial_ceiling = encoder.vbv_ceiling;
    assert!(initial_ceiling > 0);
    assert_eq!(encoder.config.rcParams.vbvBufferSize, initial_ceiling);

    // Dip far enough that the VBV would clamp to MIN_VBV_BITS (65536 bits). Scaling the clamped
    // ceiling back up used to ratchet the ceiling ~40x higher when bitrate recovered.
    encoder.bitrate(100)?;
    assert_eq!(encoder.vbv_ceiling, MIN_VBV_BITS as u32);
    assert_eq!(encoder.config.rcParams.vbvBufferSize, MIN_VBV_BITS as u32);

    encoder.bitrate(20_000)?;
    assert_eq!(encoder.vbv_ceiling, initial_ceiling);
    assert_eq!(encoder.config.rcParams.vbvBufferSize, initial_ceiling);
    assert_eq!(encoder.config.rcParams.vbvInitialDelay, initial_ceiling);

    // With custom headroom (`nvenc_vbv_increase = 400`), recovery also stays anchored to `Tuning`.
    drop(encoder);
    reset();
    let mut wide = session_with(
        &Negotiated::default(),
        ApiVersion(13, 0),
        "nvenc_vbv_increase = 400\n",
    )?;
    let wide_ceiling = wide.vbv_ceiling;
    wide.bitrate(200)?;
    assert!(wide.vbv_ceiling < wide_ceiling);
    wide.bitrate(20_000)?;
    assert_eq!(wide.vbv_ceiling, wide_ceiling);
    assert_eq!(wide.config.rcParams.vbvBufferSize, wide_ceiling);

    drop(wide);
    assert_clean();
    Ok(())
}

#[test]
fn async_teardown_sends_eos_with_registered_completion_event() -> Result<()> {
    reset();
    state(|d| d.asynchronous = true);
    let mut encoder = session(&Negotiated::default(), ApiVersion(13, 0))?;
    let slot = input(&mut encoder, 77)?;
    encoder.submit(slot, None, true, Instant::now())?;
    encoder.close();
    state(|d| {
        let eos = d
            .eos_pictures
            .last()
            .expect("EOS submitted on async teardown with pending work");
        assert_ne!(
            eos.encodePicFlags & _NV_ENC_PIC_FLAGS_NV_ENC_PIC_FLAG_EOS,
            0
        );
        assert!(
            !eos.completionEvent.is_null(),
            "async EOS must pass a non-null registered completionEvent"
        );
    });
    assert_clean();
    Ok(())
}

#[test]
fn async_teardown_waits_for_eos_completion_event_before_unregistering() -> Result<()> {
    use std::sync::atomic::{AtomicBool, Ordering};
    // Exercise both the dedicated `drain_event` path (all slots pending) and the idle-slot
    // event path (a second slot's registered event carries EOS).
    for idle_slot in [false, true] {
        reset();
        let signaled = Arc::new(AtomicBool::new(false));
        state(|d| {
            d.asynchronous = true;
            d.eos_delay_ms = 20;
            d.eos_signaled = Some(signaled.clone());
        });
        let mut encoder = session(&Negotiated::default(), ApiVersion(13, 0))?;
        let slot0 = input(&mut encoder, 77)?;
        if idle_slot {
            let _slot1 = input(&mut encoder, 78)?;
        }
        encoder.submit(slot0, None, true, Instant::now())?;
        assert!(
            encoder.close_with_wait(Duration::from_secs(1)),
            "idle_slot={idle_slot}"
        );
        assert!(
            signaled.load(Ordering::Acquire),
            "teardown must wait for EOS completionEvent to signal (idle_slot={idle_slot})"
        );
        state(|d| {
            let eos = d.calls.iter().position(|c| *c == "eos").unwrap();
            let unregister = d
                .calls
                .iter()
                .position(|c| *c == "unregister event")
                .unwrap();
            let destroy = d
                .calls
                .iter()
                .position(|c| *c == "destroy session")
                .unwrap();
            assert!(eos < unregister && unregister < destroy);
        });
        assert_clean();
    }
    Ok(())
}

#[test]
fn async_teardown_retains_events_and_slots_on_eos_timeout() -> Result<()> {
    use windows::Win32::{Foundation::WAIT_TIMEOUT, System::Threading::WaitForSingleObject};
    for idle_slot in [false, true] {
        reset();
        state(|d| {
            d.asynchronous = true;
            d.hold_eos_event = true;
        });
        let mut encoder = session(&Negotiated::default(), ApiVersion(13, 0))?;
        let slot0 = input(&mut encoder, 91)?;
        if idle_slot {
            let _slot1 = input(&mut encoder, 92)?;
        }
        encoder.submit(slot0, None, true, Instant::now())?;
        assert!(
            !encoder.close_with_wait(Duration::from_millis(15)),
            "timed-out async EOS must report unclean close (idle_slot={idle_slot})"
        );
        let eos_handle = state(|d| {
            assert!(
                !d.calls.iter().any(|c| matches!(
                    *c,
                    "unregister event" | "destroy output" | "unregister input"
                )),
                "must not unregister/destroy resources after EOS wait timed out"
            );
            assert!(d.calls.contains(&"destroy session"));
            HANDLE(d.eos_pictures.last().unwrap().completionEvent)
        });
        drop(encoder);
        // SAFETY: `eos_handle` must have been retained rather than closed via CloseHandle, so a
        // zero-timeout wait reports WAIT_TIMEOUT instead of WAIT_FAILED (invalid handle).
        let wait_result = unsafe { WaitForSingleObject(eos_handle, 0) };
        assert_eq!(
            wait_result, WAIT_TIMEOUT,
            "timed-out EOS event handle must stay open until process exit (idle_slot={idle_slot})"
        );
    }
    Ok(())
}

#[test]
fn async_teardown_skips_eos_wait_when_driver_rejects_eos() -> Result<()> {
    reset();
    state(|d| {
        d.asynchronous = true;
        d.hold_eos_event = true;
        d.eos_status = Some(_NVENCSTATUS_NV_ENC_ERR_GENERIC);
    });
    let mut encoder = session(&Negotiated::default(), ApiVersion(13, 0))?;
    let slot = input(&mut encoder, 95)?;
    encoder.submit(slot, None, true, Instant::now())?;
    let started = Instant::now();
    assert!(encoder.close_with_wait(Duration::from_secs(2)));
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "rejected EOS must not stall for the full wait budget"
    );
    drop(encoder);
    assert_clean();
    Ok(())
}
