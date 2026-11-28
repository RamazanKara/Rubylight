//! Mid-stream reconfigure requests, encoder rebuild and compute-demotion policy, frame-cap
//! sizing, and encode-error recovery for the video session loop.

use super::super::*;
use super::inline_send::InlineSender;

/// Applies a pending client reconfigure request (`0x5532` / `0x553C`) if due.
#[allow(clippy::too_many_arguments)]
pub(super) fn apply_pending_reconfigure(
    thread: &SessionThread<'_>,
    runtime_config: &Config,
    prepared: &Arc<crate::display_session::Ready>,
    stream: &mut butterpollo_media_policy::rtsp::Negotiated,
    resolution_cap: Option<(u32, u32)>,
    switching: &mut Option<(butterpollo_media_policy::reconfigure::Mode, Instant)>,
    rebuild_encoder: &mut bool,
) {
    let s = thread.s;
    let requested = {
        let mut reconfigure = s.reconfigure.lock().unwrap();
        let requested = reconfigure.take_due(Instant::now());
        if let Some(to) = requested {
            s.rate_changes.taken(to);
        }
        requested
    };
    let Some(to) = requested else { return };
    let from = butterpollo_media_policy::reconfigure::Mode::of(stream);
    let enabled = config::stream_reconfigure_enabled(runtime_config);
    match butterpollo_media_policy::reconfigure::refusal(stream, enabled) {
        Some(reason) => {
            refuse_switch(
                s,
                from,
                butterpollo_media_policy::reconfigure::Mode::applied(stream),
                butterpollo_media_policy::rate_change::refusal_reason(enabled),
            );
            tracing::info!(
                client = %s.launch.client.name,
                from = %from,
                to = %to,
                reason,
                "reconfigure refused"
            );
        }
        None => {
            tracing::info!(
                client = %s.launch.client.name,
                from = %from,
                to = %to,
                "reconfigure requested"
            );
            to.apply_to(stream, resolution_cap);
            report_stream_resolution_cap(stream, resolution_cap, &s.launch.warnings);
            if (to.width, to.height) != (from.width, from.height) {
                thread.follow_size(prepared, stream.display_size());
            }
            s.rebuild_link_rate(
                stream,
                runtime_config.link_rate_adaptation(),
                max_bitrate_kbps(s, runtime_config),
            );
            rebuild_congestion(s, stream, runtime_config);
            s.reconfigure
                .lock()
                .unwrap()
                .applied_now(butterpollo_media_policy::reconfigure::Mode::applied(stream));
            switching.get_or_insert((from, Instant::now()));
            *rebuild_encoder = true;
        }
    }
}

/// Rolls back a mid-stream mode switch when the encoder fails to create or encode at the
/// requested mode.
#[allow(clippy::too_many_arguments)]
pub(super) fn rollback_mode_switch(
    thread: &SessionThread<'_>,
    c: &Config,
    prepared: &Arc<crate::display_session::Ready>,
    stream: &mut butterpollo_media_policy::rtsp::Negotiated,
    resolution_cap: Option<(u32, u32)>,
    from: butterpollo_media_policy::reconfigure::Mode,
    error: &anyhow::Error,
    message: &'static str,
) {
    let s = thread.s;
    let to = butterpollo_media_policy::reconfigure::Mode::of(stream);
    from.apply_to(stream, resolution_cap);
    report_stream_resolution_cap(stream, resolution_cap, &s.launch.warnings);
    // The link rate, and the display or co-op window, were
    // already set for the refused mode.
    s.rebuild_link_rate(stream, c.link_rate_adaptation(), max_bitrate_kbps(s, c));
    rebuild_congestion(s, stream, c);
    if (to.width, to.height) != (from.width, from.height) {
        thread.follow_size(prepared, stream.display_size());
    }
    refuse_switch(
        s,
        from,
        butterpollo_media_policy::reconfigure::Mode::applied(stream),
        butterpollo_media_policy::rate_change::Reason::EncoderFailed,
    );
    tracing::info!(
        client = %s.launch.client.name,
        from = %from,
        to = %to,
        error = %format!("{error:#}"),
        "{message}"
    );
}

/// Outcome of attempting to rebuild the encoder inside the session loop.
pub(super) enum RebuildOutcome {
    /// Encoder rebuilt and a fresh image matching it is ready to encode.
    Ready {
        raw_image: Arc<GpuImage>,
        image: Arc<GpuImage>,
    },
    /// Caller should `continue` to the next loop iteration (retry or wait for frame).
    Continue,
}

/// Rebuilds the encoder with compute-demotion and VPE-fallback tuning, refreshing `image`
/// after creation.
#[allow(clippy::too_many_arguments)]
pub(super) fn rebuild_encoder_now(
    thread: &SessionThread<'_>,
    encoder: &mut Option<Encoder>,
    runtime_config: &Config,
    runtime_revision: u64,
    prepared: &Arc<crate::display_session::Ready>,
    timer: &butterpollo_win_graphics::timing::Timer,
    progress: &crate::stall_watch::Progress,
    latest: &Source,
    stream: &mut butterpollo_media_policy::rtsp::Negotiated,
    resolution_cap: Option<(u32, u32)>,
    switching: &mut Option<(butterpollo_media_policy::reconfigure::Mode, Instant)>,
    recovery: &mut EncoderRecovery,
    counted_failures: &mut CountedFailures,
    compute_demotion: &mut ComputeDemotion,
    converting_on_compute: &mut bool,
    nvenc_on_compute: &mut bool,
    vpe_gave_way: bool,
    inline_sender: &InlineSender,
    focus: &mut crate::stream_focus::Focus,
    enhancer: &mut crate::nvidia_enhance::Enhancer,
    pacer: &mut butterpollo_media_policy::stream_policy::Pacer,
    arrival_pacing: bool,
    image: &Arc<GpuImage>,
) -> Result<RebuildOutcome> {
    let s = thread.s;
    *encoder = None;
    // Failures while compute converted can move conversion
    // to the graphics queue for a while ([`ComputeDemotion`]).
    // Each failure event counts once, not each rebuild: a
    // failure whose first rebuild yields no frame yet, or an
    // enhancement change, must not count twice. An NVENC
    // D3D12 path that fails, rebuilds on D3D12 and fails again
    // counts both failures.
    if count_rebuild_failure(
        recovery,
        counted_failures,
        compute_demotion,
        Instant::now(),
        *converting_on_compute || *nvenc_on_compute,
        *nvenc_on_compute,
    ) {
        tracing::warn!(
            client = %s.launch.client.name,
            window_secs = COMPUTE_FAILURE_WINDOW.as_secs(),
            retry_quiet_secs = COMPUTE_RETRY_QUIET.as_secs(),
            sticky = *nvenc_on_compute,
            "encoder conversion moves to the graphics queue: a second encoder failure within the window while the compute queue converted"
        );
    }
    let mut tuning = runtime_config.clone();
    if vpe_gave_way {
        tuning.values.insert("amd_vpe".into(), "false".into());
    }
    if compute_demotion.demoted() {
        tuning
            .values
            .insert("gpu_compute_conversion".into(), "false".into());
        if butterpollo_win_graphics::compute::enabled(runtime_config) {
            let retry = if compute_demotion.sticky() {
                "Compute is not tried again for this stream."
            } else {
                "Compute is tried again after 5 minutes without a failure."
            };
            s.launch.warnings.set(
                "encoder_compute_recovery",
                format!("Compute conversion paused after an encoder failure on the compute path; converting on the graphics queue, where a busy game can delay frames. {retry} Lower game GPU load or update the graphics driver if this repeats."),
            );
        }
    }
    progress.mark(crate::stall_watch::Phase::EncoderCreate);
    match crate::win_bridge::new_encoder_gpu(stream, image, &tuning, &s.launch.warnings) {
        Ok(mut created) => {
            if created.compute_fell_back() {
                compute_demotion.demote_sticky(Instant::now());
                tracing::warn!(
                    client = %s.launch.client.name,
                    "NVENC's D3D12 session failed at creation; keeping the graphics queue for this stream"
                );
            }
            *converting_on_compute = created.converts_on_compute();
            *nvenc_on_compute = *converting_on_compute && created.backend() == "nvenc";
            *s.encoder.write().unwrap() = created.backend().into();
            created.set_hdr_target(s.hdr_target_nits());
            // A mode switch can turn `auto` slices on or off.
            if inline_sender
                .delivery
                .follow(usize::from(created.slice_delivery()))
            {
                tracing::info!(
                    client = %s.launch.client.name,
                    slice_blocks = inline_sender.delivery.blocks(),
                    width = stream.width,
                    height = stream.height,
                    "video slice delivery follows the rebuilt encoder"
                );
            }
            if let Some(sender) = inline_sender.video_sender.get() {
                sender.follow_slice_blocks(inline_sender.delivery.blocks());
            }
            *encoder = Some(created);
            focus.encoder_recreated();
        }
        Err(error) => {
            if let Some(loss) =
                DeviceLost::d3d11(&image.gpu.device).or_else(|| DeviceLost::from_error(&error))
            {
                recovery.device_loss = Some(loss);
                recovery.record_failure(Instant::now());
                counted_failures.device_loss(recovery.failure_epoch);
                return Ok(RebuildOutcome::Continue);
            }
            // The encoder cannot make the size or rate the client
            // asked for: the stream goes back to the mode it had.
            if let Some((from, _)) = switching.take() {
                rollback_mode_switch(
                    thread,
                    runtime_config,
                    prepared,
                    stream,
                    resolution_cap,
                    from,
                    &error,
                    "reconfigure refused: the encoder cannot make this mode; keeping the previous one",
                );
                return Ok(RebuildOutcome::Continue);
            }
            let since = recovery.record_failure(Instant::now());
            if since.elapsed() >= ENCODER_RECOVERY {
                return Err(error.context(EncoderFailure("the encoder could not be recreated")));
            }
            s.launch.warnings.set(
                "encoder_recovery",
                format!("Encoder recreation failed ({error:#}); retrying the same backend while the picture freezes. Check the driver and lower game GPU load; the session will fail if frames do not resume."),
            );
            timer.until(Instant::now() + Duration::from_millis(100));
            return Ok(RebuildOutcome::Continue);
        }
    }
    // Encoder creation can outlast several source frames.
    let Some(raw_image) = latest.current()? else {
        return Ok(RebuildOutcome::Continue);
    };
    // As in the loop above: read before the enhancement
    // replaces the image, and a capture that only extends
    // the focus window's picture teaches no phase.
    let extends_capture = latest.extends_capture(&raw_image);
    let (current, enhancement_changed) =
        enhancer.apply(s, runtime_config, runtime_revision, raw_image.clone());
    if enhancement_changed || !encoder.as_ref().unwrap().accepts_gpu_device(&current) {
        return Ok(RebuildOutcome::Continue);
    }
    if arrival_pacing && extends_capture == Some(false) {
        pacer.observe_source(raw_image.captured);
    }
    recovery.recreated(s);
    Ok(RebuildOutcome::Ready {
        raw_image,
        image: current,
    })
}

/// Computes and sets the recovery frame cap and scene-cut limit on `active` for non-PyroWave
/// streams, updating `frame_checks` with the route lookup cost.
#[allow(clippy::too_many_arguments)]
pub(super) fn apply_frame_cap(
    s: &Session,
    c: &Config,
    active: &mut Encoder,
    peer: SocketAddr,
    period: Duration,
    bitrate: u32,
    link_watch: &crate::video_send::link_watch::LinkWatch,
    recovery_packetizer: &mut VideoPacketizer,
    fec_whole: &mut ((usize, usize, bool), usize),
    fec_whole_due: &mut Instant,
    frame_checks: &mut Duration,
) {
    if s.config.codec == 3 {
        return;
    }
    let now = Instant::now();
    let encode_route = link_watch.get(peer);
    *frame_checks += now.elapsed();
    // Budget for the parity a recovery picture uses,
    // even while ordinary frames use adaptive FEC.
    recovery_packetizer.fec_percent = s.recovery_fec_percent();
    let pace = crate::video_send::recovery::preview_pace(
        s,
        c,
        recovery_packetizer,
        period,
        peer,
        encode_route,
    );
    let safe = butterpollo_media_policy::network_pacing::rate_bps(
        0,
        bitrate,
        encode_route.bps,
        true,
        pace.needed,
    );
    let recovery_bps = butterpollo_media_policy::network_pacing::recovery_pace_bps(
        pace.bps,
        safe,
        c.manual_pacing_kbps().unwrap_or(0),
        butterpollo_media_policy::network_pacing::route_is_wireless(
            encode_route.wireless,
            s.config.client_wireless,
        ),
    );
    let cap = butterpollo_media_policy::network_pacing::recovery_frame_cap(
        recovery_bps,
        bitrate,
        period,
        recovery_packetizer.fec_percent,
        recovery_packetizer.packet_size,
        peer.ip().to_canonical().is_ipv6(),
    );
    // Moonlight takes four FEC blocks a frame: a recovery keyframe
    // past them leaves with less parity or none, and one lost
    // packet costs it. Without FEC the cap is the four-block packet
    // limit, so a cut exposes as few packets as it can. Statistical
    // FEC picks parity by frame size; its search runs once a second.
    let statistical = s.statistical_fec();
    let key = (
        recovery_packetizer.fec_percent,
        recovery_packetizer.packet_size,
        statistical,
    );
    if fec_whole.0 != key || (statistical && now >= *fec_whole_due) {
        let whole = s
            .statistical_fec_whole_payload(now)
            .unwrap_or_else(|| recovery_packetizer.fec_whole_payload_limit());
        *fec_whole = (key, whole);
        *fec_whole_due = now + Duration::from_secs(1);
    }
    let cap = butterpollo_media_policy::network_pacing::fec_whole_frame_cap(
        cap,
        fec_whole.1,
        bitrate,
        period,
    );
    let _ = active.set_frame_cap(
        cap,
        butterpollo_media_policy::network_pacing::two_frames(bitrate, period),
    );
    // AMF does not hold a scene cut's inter frame to the cap: one
    // past this bound is encoded again as a keyframe, on NVENC too.
    active.set_cut_limit(
        butterpollo_media_policy::network_pacing::fec_whole_frame_limit(
            fec_whole.1,
            bitrate,
            period,
        )
        .unwrap_or(0),
    );
}

#[cfg(test)]
mod runtime_config_tests {
    use super::*;
    use butterpollo_media_policy::link_rate::State as LinkRateState;

    #[test]
    fn roam_restart_applies_per_device_effective_config_overrides() {
        let f = crate::state::test_support::Fixture::new();
        {
            let mut global = f.host.config.write().unwrap();
            global
                .values
                .insert("link_rate_adaptation".into(), "false".into());
            global
                .values
                .insert("congestion_control".into(), "false".into());
            global.values.insert("max_bitrate".into(), "0".into());
        }
        let mut client = f.client(u32::MAX);
        client.extra.insert(
            "config_overrides".into(),
            serde_json::json!({
                "link_rate_adaptation": "true",
                "max_bitrate": "25000",
            }),
        );
        let session = f.session(client, Role::Stream);
        // Start with global config (link rate off).
        session.configure_link_rate(false, 0);
        assert_eq!(
            session.link_rate.lock().unwrap().rate.state(),
            LinkRateState::Off
        );

        // Roaming must resolve `effective_config` so per-device overrides take effect.
        crate::stream::restart_roamed_session(&f.host, &session);
        assert_ne!(
            session.link_rate.lock().unwrap().rate.state(),
            LinkRateState::Off,
            "roam restart must honor per-device link_rate_adaptation override"
        );
    }

    #[test]
    fn video_session_loop_uses_live_runtime_config_not_startup_snapshot() {
        let run_src = include_str!("../run.rs").replace("\r\n", "\n");
        let loop_start = run_src
            .find("while !s.stopping() && !h.stop.load(Ordering::Acquire)")
            .unwrap();
        let loop_end = run_src.find("fn release_capture_resources").unwrap();
        let loop_body = &run_src[loop_start..loop_end];
        let has_startup_c = loop_body.match_indices("&c").any(|(idx, _)| {
            loop_body[idx + 2..]
                .chars()
                .next()
                .is_none_or(|ch| !ch.is_alphanumeric() && ch != '_')
        });
        assert!(
            !has_startup_c,
            "video session loop must read live &runtime_config rather than startup &c"
        );
    }
}
