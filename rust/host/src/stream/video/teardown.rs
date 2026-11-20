//! A session thread's teardown: the failure report, the session's end, and its workers,
//! records and leases released in a fixed order.
use super::*;
use butterpollo_core::stutter_report::Stage;

// The moved code still borrows `m`, `h`, `s` and the other values it now receives by
// reference, as it did when it owned them.
#[allow(clippy::needless_borrow)]
impl SessionThread<'_> {
    /// Ends the session with the setup and loop's `result`. The workers and leases are
    /// bound in the order the thread created them, so what is not released explicitly
    /// (the co-op watch) drops last, as before.
    #[rustfmt::skip]
    pub(super) fn teardown(&self, result: Result<()>, workers: Workers, leases: Leases) {
        let SessionThread { m, h, s } = *self;
        #[cfg(test)]
        if let Some(fixture) = &h.reconnect_fixture {
            fixture.count_teardown();
        }
        let Leases { com, stream_preparation, app_window, latest, encoder } = leases;
        let Workers {
            client_commands,
            audio,
            pyrowave_sender,
            webrtc_sender,
            keyframe_loss_memory,
            timing_record,
            #[cfg(test)]
            fixture_preparation,
            coop_watch: _coop_watch,
        } = workers;
        if let Err(e) = result {
            s.launch.warnings.set("stream_failure", format!("Stream ended: {e:#}"));
            s.fail_with(&e);
            tracing::error!(error=%format!("{e:#}"),client=%s.launch.client.name,"session failed; check the reported encoder, capture or socket error before reconnecting");
        }
        // The control stream records it as it tells the client; this covers a client it no
        // longer reaches.
        h.record_failure(&s);
        s.stop();
        // Congestion control over the whole stream, for the A/B (`congestion_ab.py`).
        if s.congestion_active() {
            let cc = s.congestion_snapshot();
            tracing::info!(client=%s.launch.client.name, shadow=cc["shadow"].as_bool().unwrap_or(false), reports=cc["reports"].as_u64(), packets=cc["packets"].as_u64(), lost=cc["lost"].as_u64(), late=cc["late"].as_u64(), unmatched=cc["unmatched"].as_u64(), overuse_cuts=cc["overuse_cuts"].as_u64(), overuse_held=cc["overuse_held"].as_u64(), loss_cuts=cc["loss_cuts"].as_u64(), queue_p50_ms=cc["queue_p50_ms"].as_f64(), queue_p95_ms=cc["queue_p95_ms"].as_f64(), recovery_p50_ms=cc["recovery_p50_ms"].as_u64(), recovery_max_ms=cc["recovery_max_ms"].as_u64(), feedback_bytes=cc["feedback_bytes"].as_u64(), "congestion summary");
        }
        // Where this stream's pictures lost time, from the stamps the send path
        // took: one line for the log and the support bundle, naming the stage.
        let period = s.stream_mode().period();
        let stutter = s.stats.stutter_window().report(period);
        if stutter.pictures() > 0 {
            let ms = |us: u64| (us as f64 / 100.0).round() / 10.0;
            let dominant = stutter.dominant();
            let counts = s.pipeline_counts();
            tracing::info!(
                client = %s.launch.client.name,
                windows = stutter.windows(),
                pictures = stutter.pictures(),
                late_pictures = stutter.late_pictures(),
                lost_ms = ms(stutter.total_lost_us()),
                dominant = dominant.map_or("none", Stage::name),
                dominant_percent = dominant.map_or(0, |stage| stutter.share_percent(stage)),
                capture_ms = ms(stutter.lost_us(Stage::Capture)),
                convert_ms = ms(stutter.lost_us(Stage::Convert)),
                encode_ms = ms(stutter.lost_us(Stage::Encode)),
                packetize_ms = ms(stutter.lost_us(Stage::Packetize)),
                send_ms = ms(stutter.lost_us(Stage::Send)),
                source_frames = counts.source,
                repeated_frames = counts.repeat,
                "stream stutter report"
            );
        }
        // A failed stream must not count as a healthy fast-rate probe.
        drop(keyframe_loss_memory);
        h.sessions.lock().unwrap().begin_teardown(&s);
        // For the Sessions page: one entry per stream, with why it ended.
        let reason = if s.failed() {
            butterpollo_core::session_history::REASON_FAILED
        } else {
            butterpollo_core::session_history::REASON_ENDED
        };
        h.record_session(&s, reason);
        h.session_webhook("session.ended", &s);
        for (audio, peer) in m.peers.lock().unwrap().remove(&s.launch.id) {
            if audio { &m.audio } else { &m.video }.forget(peer);
        }
        // Keep capture and display leases owned until the encoder has
        // terminated, even if a driver blocks its drop indefinitely.
        if let Some(audio) = audio {
            finish_worker(audio, "audio");
        }
        drop(pyrowave_sender);
        // Frees the WebRTC media port; a link the worker never took goes with it.
        drop(webrtc_sender);
        h.webrtc_links.discard(&s.launch.id);
        drop(timing_record);
        drop(client_commands);
        drop(encoder);
        drop(latest);
        drop(app_window);
        drop(stream_preparation);
        #[cfg(test)]
        drop(fixture_preparation);
        drop(com);
        // The encoders' reused frame buffers go with the last stream that used them; this
        // session's streaming scope ended with its setup and loop.
        butterpollo_windows::frame_pool::clear_after_stream();
        if s.launch.role == Role::RemoteMonitor
            && h.config
                .read()
                .unwrap()
                .boolean("remote_monitor_disconnect_on_stream_end", false)
        {
            crate::remote_display::disconnect(&h, Some(&s.launch.client.uuid));
        }
        h.sessions.lock().unwrap().teardown.remove(&s.launch.id);
        tracing::info!(client=%s.launch.client.name,"CLIENT DISCONNECTED");
    }
}
