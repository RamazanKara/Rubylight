//! A session thread's setup phases, in the order `spawn_session` runs them: the client's
//! last-seen time, the session's configuration (or a whole input-only session), the
//! display, the capture with the encoder configured from its first frame, and the start
//! of the media: client commands, audio, FEC and pacing settings, the video timeline and
//! the WebRTC or PyroWave sender.
use super::*;

/// The session's configuration and the stream thread's timer. Fields are declared in
/// reverse order of creation: a struct drops its fields in declaration order, so an early
/// return drops them as it dropped the locals of the single function they came from.
pub(super) struct Configured {
    pub(super) timer: butterpollo_windows::timing::Timer,
    pub(super) c: Config,
    pub(super) config_version: u64,
}

/// The display the stream shows. Fields in reverse order of creation, as in [`Configured`].
pub(super) struct StreamDisplay {
    pub(super) output: String,
    pub(super) prepared: Arc<crate::display_session::Ready>,
}

/// The open capture and the encoder's focus. Fields in reverse order of creation, as in
/// [`Configured`].
pub(super) struct Captured<'a> {
    pub(super) focus_verify: Option<u32>,
    pub(super) focus: crate::stream_focus::Focus,
    pub(super) capture_wake: Arc<capture::Consumer>,
    pub(super) latest: &'a mut Arc<Source>,
}

/// The video timeline the session loop continues. Fields in reverse order of creation, as
/// in [`Configured`].
pub(super) struct Started {
    pub(super) present_stamper: Option<butterpollo_windows::present_timing::Stamper>,
    pub(super) start: Instant,
    pub(super) claims: std::cell::RefCell<butterpollo_core::performance::Claims>,
    pub(super) next_wire_frame: std::cell::Cell<u64>,
    pub(super) packetizer: VideoPacketizer,
    pub(super) automatic_pacing: bool,
}

// The moved code still borrows `m`, `h`, `s` and the other values it now receives by
// reference, as it did when it owned them.
#[allow(clippy::needless_borrow)]
impl SessionThread<'_> {
    /// The paired client's last-seen time, saved on a thread of its own.
    #[rustfmt::skip]
    pub(super) fn record_last_seen(&self) {
        let SessionThread { h, s, .. } = *self;
        let paired_h = h.clone();
        let client_uuid = s.launch.client.uuid.clone();
        let last_seen = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if let Err(error) = thread::Builder::new().name("client last seen".into()).spawn(move || {
            let mut paired = paired_h.paired.write().unwrap();
            if let Some(client) = paired
                .clients
                .iter_mut()
                .find(|c| c.uuid == client_uuid)
            {
                let last_seen = last_seen.max(client.extra.get("last_seen").and_then(serde_json::Value::as_u64).unwrap_or(0));
                client.extra.insert(
                    "last_seen".into(),
                    serde_json::json!(last_seen),
                );
                if let Err(error) = paired.save(&paired_h.paired_path) {
                    tracing::warn!(%error,"client last-seen persistence failed");
                }
            }
        }) {
            tracing::warn!(%error, "client last-seen worker failed");
        }
    }

    /// The session's configuration and timer. `None` once an input-only session, which has
    /// no video, has ended.
    #[rustfmt::skip]
    pub(super) fn configure(&self, workers: &mut Workers) -> Result<Option<Configured>> {
        let SessionThread { h, s, .. } = *self;
        let config_version = h.config_version.load(Ordering::Acquire);
        let mut c = effective_config(&h, &s.launch)?;
        if s.config.vrr_low_latency {
            c.values.insert("wgc_slot_aligned_publish".into(), "false".into());
        }
        let timer = butterpollo_windows::timing::Timer::new()?;
        let output = s
            .launch
            .client
            .extra
            .get("output_name_override")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or(c.get("output_name", ""));
        if s.launch.role == Role::InputOnly {
            let monitors = butterpollo_windows::input::on_input_desktop(
                butterpollo_windows::display::monitors,
            )?;
            let monitor = monitors
                .iter()
                .find(|m| m.matches(output))
                .or_else(|| monitors.iter().find(|m| m.primary))
                .context("input display unavailable")?;
            *s.output.write().unwrap() = monitor.display_name.clone();
            workers.client_commands = Some(crate::process::ClientCommands::start(&h, &s)?);
            while !s.stopping() && !h.stop.load(Ordering::Acquire) {
                thread::sleep(Duration::from_millis(10));
            }
            return Ok(None);
        }
        Ok(Some(Configured { timer, c, config_version }))
    }

    /// The display to stream: the launch's, retimed or prepared again for the negotiated
    /// mode, and kept for the device's reconnect.
    #[rustfmt::skip]
    pub(super) fn prepare_display(
        &self,
        c: &Config,
        stream_preparation: &mut Option<crate::display_session::StreamPreparation>,
    ) -> Result<StreamDisplay> {
        let SessionThread { h, s, .. } = *self;
        let initial = s
            .launch
            .preparation
            .lock()
            .unwrap()
            .take();
        // A stream request at another rate than the launch keeps an
        // owned virtual display and raises its refresh where needed.
        // A co-op player's window follows the stream's size instead.
        let initial = match initial {
            Some(p) if s.launch.nucleus.is_none() && !p.prepared().display.matches(&s.config) => Some(
                crate::display_session::retime(p.into_prepared(), &h, &s.launch, &s.config, &c)?,
            ),
            Some(p) => Some(Ok(p.into_prepared())),
            None => None,
        };
        let stream_preparation = stream_preparation.insert(match initial {
            Some(Ok(prepared)) => prepared,
            previous => {
                if previous.is_some() {
                    tracing::info!(client = %s.launch.client.name, width = s.config.width, height = s.config.height, fps_millihz = s.config.fps_millihz(), hdr = s.config.hdr, "negotiated mode differs from launch; preparing display again");
                }
                drop(previous);
                let previous = h.app_display.lock().unwrap().remove(&s.launch.client.uuid);
                drop(previous);
                match s.launch.nucleus {
                    Some(_) => crate::nucleus::prepare(&h, &s.launch, &s.config, &c, crate::nucleus::WINDOW_WAIT)?,
                    None => crate::display_session::prepare_stream(
                        &h, &s.launch, &s.config, &c,
                    )?,
                }
            }
        });
        stream_preparation.report_limiter(&s.launch.warnings, &c);
        let prepared = stream_preparation.display.clone();
        // A game window is no display to keep for the device.
        if s.launch.role == Role::Stream
            && s.launch.nucleus.is_none()
            && prepared.kept_for_reconnect()
        {
            let mut displays = h.app_display.lock().unwrap();
            if !s.stopping() {
                displays.insert(s.launch.client.uuid.clone(), (prepared.clone(), None));
            }
        }
        let output = prepared.output();
        *s.output.write().unwrap() = output.clone();
        Ok(StreamDisplay { output, prepared })
    }

    /// Opens the capture, waits for its first frame and configures the encoder and its focus
    /// from it. `None` when the stream stopped first.
    #[rustfmt::skip]
    pub(super) fn start_capture<'a>(
        &self,
        c: &Config,
        timer: &butterpollo_windows::timing::Timer,
        prepared: &Arc<crate::display_session::Ready>,
        latest: &'a mut Option<Arc<Source>>,
        mut encoder: &mut Option<Encoder>,
    ) -> Result<Option<Captured<'a>>> {
        let SessionThread { m, h, s } = *self;
        let (latest, capture_wake) = m.open_session_capture(&s, &c, &prepared, latest)?;
        let first = {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                // No frame, encoder or filter is owned during startup.
                latest.release_generation(&capture_wake);
                if let Some(image) = latest.current()? { break image; }
                if s.stopping() || h.stop.load(Ordering::Acquire) {
                    return Ok(None);
                }
                if Instant::now() >= deadline {
                    anyhow::bail!("capture produced no GPU frame on {} using {} within 10 seconds; check that the selected display is powered on, or select a virtual display for headless streaming", prepared.output(), prepared.capture());
                }
                if let Some(image) = latest.wait_for_frame(&timer, &capture_wake, (Instant::now() + Duration::from_millis(50)).min(deadline))? { break image; }
            }
        };
        require_hdr_source(&s, || first.gpu.display_in_hdr(), false)?;
        s.source_checked.store(true, Ordering::Release);
        m.configure_encoder(&s, &c, &first, &prepared, &mut encoder)?;
        drop(first);
        let focus = crate::stream_focus::Focus::start(
            &c,
            butterpollo_core::encoder_focus::Context {
                codec: s.config.codec,
                ten_bit: s.config.ten_bit(),
                pyrowave: s.config.codec == 3,
                packed_444: s.config.chroma_packing != 0,
                d3d12: butterpollo_windows::compute::enabled(&c),
                preanalysis: crate::stream_focus::preanalysis(&c, s.config.ten_bit()),
                support: h.focus_support(s.config.codec, s.config.ten_bit()),
            },
            &s.encoder_focus,
            &s.encoder_focus_state,
            &s.launch.warnings,
        );
        // One frame a second gets QP feedback for `qp_delta`.
        let focus_verify = c
            .boolean("encoder_focus_verify", false)
            .then(|| s.config.fps.max(1));
        Ok(Some(Captured { focus_verify, focus, capture_wake, latest }))
    }

    /// Starts the client commands and audio, applies the FEC, bitrate and pacing settings,
    /// and starts the video timeline, the timing record and the WebRTC or PyroWave sender.
    #[rustfmt::skip]
    pub(super) fn start_media(
        &self,
        c: &Config,
        prepared: &Arc<crate::display_session::Ready>,
        workers: &mut Workers,
    ) -> Result<Started> {
        let SessionThread { m, h, s } = *self;
        workers.client_commands = Some(crate::process::ClientCommands::start(&h, &s)?);
        workers.audio = Some(m.spawn_audio(&h, &s)?);
        configure_video_fec(&s, &c)?;
        configure_adaptive_bitrate(&s, &c);
        s.configure_link_rate(c.link_rate_adaptation(), max_bitrate_kbps(&s, &c));
        // Packet adaptation is opt-in, or automatic for a client that
        // reports a wireless route. Explicit pacing and host bitrate
        // adaptation keep their own policies.
        let keyframe_loss_pacing = crate::video_send::recovery::configure(&s, &c);
        let automatic_pacing = s.automatic_pacing_enabled();
        workers.keyframe_loss_memory = Some(h.configure_keyframe_loss(&s, keyframe_loss_pacing));
        let packetizer = VideoPacketizer {
            sequence: 0,
            iv_counter: 0,
            frame: 1,
            packet_size: s.config.packet_size,
            fec_percent: c.integer("fec_percentage", 20).clamp(0, 100) as usize,
            min_fec: s.config.min_fec,
            key: if s.config.encryption & 2 != 0 {
                Some(s.launch.key)
            } else {
                None
            },
        };
        let next_wire_frame = std::cell::Cell::new(u64::from(packetizer.frame));
        // New pictures claimed and not yet sent, for the stage timings.
        let claims = std::cell::RefCell::new(butterpollo_core::performance::Claims::default());
        let start = Instant::now();
        let start_qpc_ms = butterpollo_windows::timing::qpc_ms();
        *s.media_epoch.write().unwrap() = Some(start);
        // Present times come from ETW only if the stamper really has a tracker.
        let mut present_stamper = (prepared.capture() == "wgc").then(butterpollo_windows::present_timing::Stamper::default);
        let tracks_presents = present_stamper.as_ref().is_some_and(|stamper| stamper.tracks_presents());
        // The opt-in host-side record: off, and no thread or file, unless the
        // environment names a directory.
        *s.timing_record.write().unwrap() = crate::timing_record::attach(
            crate::timing_record::directory().as_deref(),
            &s.launch.id,
            butterpollo_core::timing_record::Meta {
                codec: butterpollo_core::timing_record::codec_name(s.config.codec),
                capture: prepared.capture().to_string(),
                present_source: butterpollo_core::timing_record::present_source(tracks_presents),
                media_epoch_qpc_ms: start_qpc_ms,
            },
        );
        workers.webrtc_sender = if s.launch.transport == butterpollo_core::session::Transport::WebRtc {
            Some(crate::webrtc::session::Sender::new(h.webrtc_links.take(&s.launch.id).context("the WebRTC media link is missing for this session")?))
        } else { None };
        workers.pyrowave_sender = if s.config.codec == 3 { Some(crate::pyrowave_send::Sender::new(m.video.clone(),s.clone(),c.clone(),h.clone(),start,present_stamper.take())?) } else { None };
        Ok(Started { present_stamper, start, claims, next_wire_frame, packetizer, automatic_pacing })
    }
}
