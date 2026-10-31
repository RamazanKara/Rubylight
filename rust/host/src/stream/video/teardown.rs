//! A session thread's teardown: the failure report, the session's end, and its workers,
//! records and leases released in a fixed order.
use super::*;

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
        let Leases { com, stream_preparation, latest, encoder } = leases;
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
            s.fail();
            tracing::error!(error=%format!("{e:#}"),client=%s.launch.client.name,"session failed; check the reported encoder, capture or socket error before reconnecting");
        }
        s.stop();
        // A failed stream must not count as a healthy fast-rate probe.
        drop(keyframe_loss_memory);
        h.sessions.lock().unwrap().begin_teardown(&s);
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
        drop(stream_preparation);
        #[cfg(test)]
        drop(fixture_preparation);
        drop(com);
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
