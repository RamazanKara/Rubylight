//! Per-peer state for the ENet control worker: crypto, cached effective config, display
//! scale lookup, and text-focus watcher lifecycle.

use super::super::*;
use super::haptics::RumbleForwarded;
use super::input::PadsWait;
use super::socket::ControlSocket;

/// Reusable per-loop buffers: clock syncs, control packet, legacy input.
pub(super) type ReceiveScratch = (Vec<(PeerID, u16, Instant, Vec<u8>)>, Vec<u8>, Vec<u8>);

pub(in crate::stream) struct ControlPeer {
    #[cfg(any(test, feature = "test-support"))]
    pub(super) _fixture_input: Option<super::super::reconnect_harness::Slot>,
    pub(in crate::stream) id: String,
    pub(super) injector: Option<Injector>,
    /// Whether making the injector was tried.
    pub(super) injector_tried: bool,
    /// Controllers the injector was made without, and their input meanwhile.
    pub(super) pads_wait: PadsWait,
    /// A co-op player's slot, which takes its input instead of an injector.
    pub(super) coop: Option<crate::nucleus::Player>,
    /// Whether finding the co-op player was tried.
    pub(super) coop_tried: bool,
    pub(super) seq_state: Arc<Mutex<super::super::ControlSeqState>>,
    /// The session key's control cipher, expanded once rather than per message.
    pub(super) ciphers: butterpollo_media_policy::packet::ControlCiphers,
    pub(super) hdr_metadata: Option<[u8; 27]>,
    /// The session's `hdr_revision` the metadata above was read at: the loop
    /// reads the metadata only when the number moved.
    pub(super) hdr_seen: u64,
    pub(super) legacy: butterpollo_media_policy::packet::LegacyInput,
    pub(super) seen: Instant,
    pub(super) input_released: bool,
    pub(super) session: Option<Arc<Session>>,
    pub(super) inputs: Vec<input::Input>,
    /// How evenly its pointer moves arrive, for the log.
    pub(super) pointer_gaps: butterpollo_media_policy::arrival_gaps::ArrivalGaps,
    /// The longest injection pass since the last "pointer arrivals" line.
    pub(super) inject_max: Duration,
    /// Input permissions this device lacked and was warned about.
    pub(super) denied: u32,
    /// Whether this device was warned about refused Steam Controller attributes.
    pub(super) steam_attributes_refused: bool,
    pub(super) controller_audio: [u8; 16],
    pub(super) controller_pcm: controller_audio::PcmTraffic,
    /// Controllers bound to Nonary's speaker PCM (0x5602) instead of `controller_audio`.
    pub(in crate::stream) ps_speaker: butterpollo_input_policy::ps_speaker::Speakers,
    /// When the bound controllers' speaker status last went to the injector.
    pub(super) ps_speaker_status_at: Option<Instant>,
    pub(super) rumble: RumbleForwarded,
    pub(super) command_at: Option<Instant>,
    /// Whether the session is in the control socket's roaming table.
    pub(super) roaming: bool,
    /// The couch menu's filter for this peer's session.
    pub(super) couch: crate::couch_menu::PeerMenu,
    /// Haptics from game audio for this peer's controllers.
    pub(super) audio_haptics: audio_haptics::PeerHaptics,
    /// The keyboard layout keys go with (0x553E, `keyboard_layout`).
    pub(super) keyboard_layout: keyboard_layout::PeerLayout,
    /// This peer's index, which owns a client-drawn cursor session it enabled.
    pub(super) cursor_owner: usize,
    /// The text field watcher, from the client's first text input request (0x553B); paused
    /// while the client turned it off, and dropped with the peer at session end.
    pub(super) focus_watch: Option<butterpollo_win_io::text_focus::FocusWatch>,
    /// Clipboard sync (0x5508), from the device's first frame; its worker ends with the peer.
    pub(super) clipboard_sync: clipboard_sync::PeerSync,
    /// The session's settings for control packets that read them.
    pub(super) config: PeerConfig,
    /// The display scale a client-drawn cursor's shapes are sized for, read off this thread.
    pub(super) cursor_scale: ScaleLookup,
    /// Raw control packets received while the launch was still pending in `sessions.pending`,
    /// replayed once `sessions.active` gets this peer's session.
    pub(super) pre_active_packets: std::collections::VecDeque<Vec<u8>>,
}

impl Drop for ControlPeer {
    fn drop(&mut self) {
        if let Some(injector) = &mut self.injector {
            injector.detach_gamepads();
        }
        // The cursor goes back into the video when its device leaves.
        if let Some(s) = &self.session {
            s.client_cursor.end(self.cursor_owner);
        }
    }
}

impl ControlPeer {
    /// Creates per-peer state for a newly accepted ENet control connection.
    pub(super) fn new(
        h: &Shared,
        launch: &Launch,
        peer_id: PeerID,
        session: Option<Arc<Session>>,
        control_seqs: &super::super::ControlSeqMemory,
    ) -> Self {
        let _ = h;
        Self {
            #[cfg(any(test, feature = "test-support"))]
            _fixture_input: h.reconnect_fixture.as_ref().map(|f| f.input()),
            id: launch.id.clone(),
            injector: None,
            injector_tried: false,
            pads_wait: PadsWait::default(),
            coop: None,
            coop_tried: false,
            seq_state: control_seqs.for_launch(&launch.id, launch.key),
            ciphers: Default::default(),
            hdr_metadata: None,
            hdr_seen: 0,
            legacy: butterpollo_media_policy::packet::LegacyInput::new(launch.key_id),
            seen: Instant::now(),
            input_released: false,
            session,
            inputs: vec![],
            pointer_gaps: Default::default(),
            inject_max: Duration::ZERO,
            denied: 0,
            steam_attributes_refused: false,
            controller_audio: [0; 16],
            controller_pcm: Default::default(),
            ps_speaker: Default::default(),
            ps_speaker_status_at: None,
            rumble: Default::default(),
            command_at: None,
            roaming: false,
            couch: Default::default(),
            audio_haptics: Default::default(),
            keyboard_layout: Default::default(),
            cursor_owner: peer_id.0,
            focus_watch: None,
            clipboard_sync: Default::default(),
            config: Default::default(),
            cursor_scale: Default::default(),
            pre_active_packets: std::collections::VecDeque::new(),
        }
    }

    /// The session's effective configuration, made once per settings change (see
    /// `PeerConfig`).
    pub(super) fn config(&mut self, h: &Shared, s: &Session) -> Result<Arc<Config>> {
        // As the stream loop: an HDR calibration's peak is read again every 30 s.
        let refresh = s
            .launch
            .client
            .extra
            .get("hdr_profile")
            .and_then(serde_json::Value::as_str)
            .filter(|profile| !profile.is_empty())
            .map(|_| CALIBRATION_REFRESH);
        self.config.get(
            h.config_version.load(Ordering::Acquire),
            Instant::now(),
            refresh,
            || effective_config(h, &s.launch),
        )
    }

    /// Starts, pauses or resumes the text field watcher after a client's text input request
    /// (0x553B): one thread per peer however often the client toggles. The watcher notes the
    /// newest focus in the session and wakes this loop, which sends the notice; one that was
    /// paused or stopped reports nothing more.
    pub(super) fn watch_focus(
        &mut self,
        s: &Arc<Session>,
        watch: TextWatch,
        wake: &Arc<UdpSocket>,
    ) {
        if let Some(watcher) = &self.focus_watch {
            watcher.set(watch);
            return;
        }
        let TextWatch::On { max_text } = watch else {
            return;
        };
        let session = Arc::downgrade(s);
        let failed = session.clone();
        let wake = wake.clone();
        let started = butterpollo_win_io::text_focus::FocusWatch::start(
            max_text,
            move |focused| {
                if let Some(s) = session.upgrade() {
                    s.text_input.note(focused, Instant::now());
                    let _ = wake.send(&[1]);
                }
            },
            move |error| {
                if let Some(s) = failed.upgrade() {
                    focus_watch_failed(&s, &error);
                }
            },
        );
        match started {
            Ok(watcher) => self.focus_watch = Some(watcher),
            Err(error) => focus_watch_failed(s, &error),
        }
    }

    /// Speaker and haptics waveforms go unreliable on their own channel: a
    /// late one is useless.
    pub(super) fn send_waveform(
        &mut self,
        host: &mut Host<ControlSocket>,
        peer: PeerID,
        s: &Session,
        kind: u16,
        payload: &[u8],
    ) {
        if let Ok(message) = self.encrypt(s, kind, payload) {
            let _ = host.peer_mut(peer).send(
                haptics::CHANNEL,
                &Packet::new(message, PacketKind::AlwaysUnreliable { sequenced: false }),
            );
        }
    }

    pub(super) fn encrypt(&mut self, s: &Session, kind: u16, payload: &[u8]) -> Result<Vec<u8>> {
        let seq = {
            let mut state = self.seq_state.lock().unwrap();
            let seq = state.sequence;
            state.sequence = seq.checked_add(1).context("control nonce exhausted")?;
            seq
        };
        self.ciphers
            .get(&s.launch.key, s.config.encryption & 1 != 0)
            .seal(seq, kind, payload)
    }
}

/// How often a cached configuration of a device with an HDR calibration is made again.
pub(super) const CALIBRATION_REFRESH: Duration = Duration::from_secs(30);

/// A peer's effective configuration for its control packets, made again when the
/// host's settings or apps change (`config_version`) and, with `refresh`, after that
/// long. Text input, clipboard, keyboard layout, cursor, rate change and controller
/// audio packets each cloned the whole configuration, and with an HDR calibration read
/// its file, on the input thread. A failure is not kept: the next packet tries again.
#[derive(Default)]
pub(in crate::stream) struct PeerConfig {
    cached: Option<(u64, Instant, Arc<Config>)>,
}

impl PeerConfig {
    pub(in crate::stream) fn get(
        &mut self,
        version: u64,
        now: Instant,
        refresh: Option<Duration>,
        load: impl FnOnce() -> Result<Config>,
    ) -> Result<Arc<Config>> {
        if let Some((cached_version, made, config)) = &self.cached
            && *cached_version == version
            && refresh.is_none_or(|refresh| now.saturating_duration_since(*made) < refresh)
        {
            return Ok(config.clone());
        }
        self.cached = None;
        let config = Arc::new(load()?);
        self.cached = Some((version, now, config.clone()));
        Ok(config)
    }
}

/// A display's scale for the client-drawn cursor, looked up on its own thread: the
/// lookup enumerates every display (1.6 ms, once 21 ms) and held up the control
/// thread's input on the first shape and after display changes. The thread stores its
/// result before it wakes the control loop, whose next pass sends the shape.
#[derive(Default)]
pub(in crate::stream) struct ScaleLookup {
    running: Option<(String, ScaleSlot)>,
}

/// Where a lookup thread leaves its result for the control loop.
type ScaleSlot = Arc<std::sync::Mutex<Option<Result<u32>>>>;

impl ScaleLookup {
    /// `output`'s scale once a lookup for it finished; None while it runs, after
    /// starting it with `lookup`, which runs here only if no thread can start.
    pub(in crate::stream) fn poll(
        &mut self,
        output: &str,
        lookup: fn(&str) -> Result<u32>,
        wake: impl FnOnce() + Send + 'static,
    ) -> Option<Result<u32>> {
        if let Some((looked_up, result)) = &self.running
            && looked_up == output
        {
            let scale = result.lock().unwrap().take();
            if scale.is_some() {
                self.running = None;
            }
            return scale;
        }
        let name = output.to_owned();
        let result: ScaleSlot = Arc::new(std::sync::Mutex::new(None));
        let slot = result.clone();
        let started = thread::Builder::new()
            .name("cursor scale".into())
            .spawn(move || {
                let scale = std::panic::catch_unwind(|| lookup(&name))
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("the display scale lookup panicked")));
                *slot.lock().unwrap() = Some(scale);
                wake();
            });
        match started {
            Ok(_) => {
                self.running = Some((output.to_owned(), result));
                None
            }
            Err(error) => {
                tracing::warn!(%error, "display scale lookup thread did not start; looked up inline");
                self.running = None;
                Some(lookup(output))
            }
        }
    }
}

/// Says on the stream card that the device's keyboard cannot follow text fields.
fn focus_watch_failed(s: &Session, error: &anyhow::Error) {
    tracing::warn!(client = %s.launch.client.name, error = %format!("{error:#}"), "focus watcher failed");
    s.launch.warnings.set(
        "text_input",
        format!(
            "The device's keyboard cannot follow text fields: the focus watcher failed ({error:#}). Type with the device's keyboard button instead."
        ),
    );
}
