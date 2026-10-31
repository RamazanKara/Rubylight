//! Stream sessions: launches, live sessions and their statistics, and the
//! user-facing warnings a session raises.
use crate::rtsp::Negotiated;
use serde::Serialize;
use std::{
    net::IpAddr,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

mod launch;
mod link;
mod registry;
mod warnings;
pub use launch::{
    AudioPreparation, CLAIM_LIMIT, LAUNCH_WAIT, Launch, Preparation, RTSP_LEASE, Seat, Transport,
};
pub use link::LINK_WARNING;
use link::{LinkReport, link_notice, mbps, note_link_change};
pub use registry::Sessions;
pub use warnings::{EVENT_PERIOD, Warning, Warnings};

#[derive(Clone, Copy, Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Stream,
    RemoteMonitor,
    InputOnly,
}

#[cfg(test)]
mod tests;
#[derive(Default)]
pub struct Stats {
    pub frames: AtomicU64,
    pub packets: AtomicU64,
    pub bytes: AtomicU64,
    pub idr_requests: AtomicU64,
    pub reference_invalidations: AtomicU64,
    /// Keyframes the host started itself after it failed to send a frame
    /// that FEC can repair; not counted in `idr_requests`.
    pub send_loss_recoveries: AtomicU64,
    /// Video frames the host could not send whole (a socket refused more
    /// than FEC repairs).
    pub frames_lost_on_send: AtomicU64,
    /// Keyframes too large for full FEC within Moonlight's four blocks.
    pub keyframes_past_fec: AtomicU64,
    /// Streamed video frames whose encoder output passed the byte cap their
    /// block plan relied on (`video_slice_delivery`); each turned it off.
    pub slice_delivery_overflows: AtomicU64,
    pub latency_us: AtomicU64,
    pub frames_replaced: AtomicU64,
    /// Last frame offered to the video socket, plus one; zero means none yet.
    pub video_frame: AtomicU64,
    pub performance: std::sync::Mutex<crate::performance::Performance>,
    /// The stages `frames` doesn't count, for the pipeline frame rates.
    pub pipeline: crate::pipeline_rates::Counters,
    /// The console's pipeline frame rates, over `pipeline_rates::CONSOLE_WINDOW`.
    pub pipeline_console: std::sync::Mutex<crate::pipeline_rates::Sampler>,
}
/// The encoder's share of a video `budget` that also carries `fec` percent
/// parity: budget × 100 / (100 + fec), at least 1 Kbps of a non-zero budget.
/// Points a statistical FEC share must fall before the encoder's split
/// follows it between fences.
const FEC_SPLIT_HYSTERESIS: u32 = 5;
fn fec_split_kbps(budget: u32, fec: u32) -> u32 {
    if budget == 0 {
        return 0;
    }
    (u64::from(budget) * 100 / (100 + u64::from(fec))).max(1) as u32
}
pub struct Session<P = (), A = ()> {
    pub encoder: std::sync::RwLock<String>,
    pub capture_warnings: std::sync::RwLock<Arc<Warnings>>,
    pub launch: Launch<P, A>,
    pub config: Negotiated,
    pub stop: AtomicBool,
    /// Set before `stop` when the stream ends on an error, so the client is
    /// told it failed rather than that the host closed it.
    pub failed: AtomicBool,
    /// Set by the stream worker once the capture's first frame passed the checks a WebRTC HDR
    /// stream needs (the display is in HDR). The transport holds back its "streaming" status
    /// until then, so a refused stream is never announced first.
    pub source_checked: AtomicBool,
    pub idr: AtomicBool,
    pub invalidation: std::sync::Mutex<Option<(u64, u64)>>,
    pub recovery_wake: std::sync::Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    pub bitrate: AtomicU32,
    /// All bitrate writers take this lock, including client ownership handoff.
    pub adaptive_bitrate: std::sync::Mutex<crate::adaptive_bitrate::AdaptiveBitrate>,
    /// Lowers the encoder bitrate when the link cannot carry the stream; see
    /// [`crate::link_rate`]. Off for PyroWave, a client that sets the bitrate
    /// and `host_adaptive_bitrate`.
    pub link_rate: std::sync::Mutex<crate::link_control::LinkControl>,
    /// What the video sender sent, for `link_rate`; one store per frame.
    pub link_sent: crate::link_control::SentLog,
    /// The route policy paced the last frame faster than the Wi-Fi rate, so
    /// a latch that slows it may be the whole fix (see `LinkRate::on_latch`).
    pace_was_fast: AtomicBool,
    pub adaptive_video_pacing_bps: AtomicU64,
    pub automatic_pacing: std::sync::Mutex<crate::adaptive_pacing::AdaptivePacing>,
    /// Whether `automatic_pacing` is enabled, set with it at stream start.
    automatic_pacing_on: AtomicBool,
    /// Slower pacing once the client's link has dropped a keyframe.
    pub keyframe_loss: std::sync::Mutex<crate::keyframe_loss::KeyframeLoss>,
    /// FEC percentage for the next video frame; the video sender reads it per
    /// frame, and `adaptive_fec` moves it from the client's 0x5502 reports.
    pub fec_percent: AtomicU32,
    pub adaptive_fec: std::sync::Mutex<crate::adaptive_fec::AdaptiveFec>,
    /// Statistical FEC from the client's 0x5539 reports, for a client that
    /// launched with `rlFecPercentage=auto`; `adaptive_fec` is off then.
    pub client_fec: std::sync::Mutex<Option<crate::statistical_fec::StatisticalFec>>,
    /// Whether `client_fec` is set: frames skip its lock otherwise.
    client_fec_on: AtomicBool,
    /// Whether the encoder's bitrate leaves room for the client-chosen FEC
    /// share in `fec_split`, latched at the last fence; see [`Self::encoder_bitrate`].
    fec_split_on: AtomicBool,
    fec_split: AtomicU32,
    /// Capture timing locked to the client's display, from its 0x5530 reports.
    pub phase_sync: std::sync::Mutex<crate::phase_sync::PhaseSync>,
    /// The stream's current mode and the client's 0x5532 requests to change it.
    pub reconfigure: std::sync::Mutex<crate::reconfigure::Reconfiguration>,
    /// Bitrate and rate changes for a client that launched with `rlRateChange=1` (0x553C):
    /// their switches and answers, sent by the control loop. Take after `reconfigure`.
    pub rate_changes: std::sync::Mutex<crate::rate_change::RateChanges>,
    pub stats: Stats,
    pub started: Instant,
    pub output: std::sync::RwLock<String>,
    /// The display's HDR metadata, once the encoder has it. Clients are told
    /// the HDR state only then, so they hear it once with the real values.
    pub hdr_metadata: std::sync::RwLock<Option<crate::hdr::Metadata>>,
    /// Source HDR metadata and diagnostic client display luminance (0x5531).
    pub hdr_source: std::sync::Mutex<crate::display_caps::HdrSource>,
    /// Per-picture present-to-sent reports for a client that asked (0x5536).
    pub host_timings: std::sync::Mutex<crate::host_timing::HostTimings>,
    /// Tells a client that asked (0x5538) when the host holds the bitrate down for the
    /// network; fed by `tick_link_rate`, sent by the control loop.
    pub link_notices: std::sync::Mutex<crate::link_notice::LinkNotices>,
    /// Pipeline frame rates for a client that asked (0x553D), sent by the control loop.
    pub pipeline_rates: std::sync::Mutex<crate::pipeline_rates::Reporter>,
    /// Keyboard follows focus for a client that asked (0x553B): fed by the focus watcher,
    /// sent by the control loop.
    pub text_input: std::sync::Mutex<crate::text_input::TextInputs>,
    /// The layout id the device's newest keyboard layout report (0x553E) resolved to, on
    /// arrival; None when it named none the host knows, or sent none.
    pub keyboard_layout: std::sync::Mutex<Option<u32>>,
    /// The cursor a client that asked (0x5540) draws itself: noted by the video stream,
    /// which then leaves the pointer out, and sent by the control loop.
    pub client_cursor: crate::client_cursor::ClientCursor,
    /// Zero of the video stream's RTP clock, set when its sender starts; clock sync
    /// replies (0x5537) are on this clock.
    pub media_epoch: std::sync::RwLock<Option<Instant>>,
    /// The host-side timing record (`BUTTERPOLLO_TIMING_RECORD`), when it is on.
    pub timing_record: std::sync::RwLock<Option<Arc<crate::timing_record::Sink>>>,
    /// The requested `encoder_focus` ([`crate::encoder_focus::Requested::wire`]). The stream
    /// stores the configured value at start; the encoder reads it per frame.
    pub encoder_focus: AtomicU8,
    /// What the encoder runs, written by the encoder thread on change.
    pub encoder_focus_state: std::sync::Mutex<crate::encoder_focus::FocusState>,
    /// Where the client is after a roam (`crate::roaming`); see [`Self::roamed`].
    roam: std::sync::Mutex<Roam>,
    /// Roams so far, for senders that keep state about the client's path.
    roams: AtomicU32,
    /// Night mode and headphone surround: the live choice and why either
    /// doesn't apply. Written through `audio_dsp::{state, apply}`.
    pub audio: crate::audio_dsp::Controls,
    /// The sent audio's copy for haptics from game audio, filled only while a
    /// controller wants it (`audio_haptics`). Shared so a control peer that
    /// wanted it can stop the copy when it goes.
    pub audio_haptics: Arc<crate::audio_haptics::Tap>,
    /// The host-drawn couch menu: open flag, revision and panel, shared by the
    /// control loop, the menu worker, the video loop and the native API.
    pub couch_menu: Arc<crate::couch_menu::Shared>,
}
/// The client's address after its roams.
struct Roam {
    client: IpAddr,
    at: Option<Instant>,
    /// No video ping has come from the path the client roamed to yet.
    new_path: bool,
}
impl<P, A> Session<P, A> {
    /// Request an encoder focus mode, such as from the couch menu; the
    /// encoder resolves it on its next frame.
    pub fn set_encoder_focus(&self, requested: crate::encoder_focus::Requested) {
        self.encoder_focus
            .store(requested.wire(), std::sync::atomic::Ordering::Relaxed);
    }
    /// Notes a new picture whose last packet was just handed to the socket: for a client
    /// that asked for host timing (0x5536) and for the host-side record. `rtp_timestamp` is
    /// the picture's RTP timestamp on the wire and `epoch` the media clock's start.
    pub fn record_sent(
        &self,
        wire_frame: u32,
        rtp_timestamp: u32,
        stamps: &crate::performance::Stamps,
        epoch: Instant,
    ) {
        self.host_timings
            .lock()
            .unwrap()
            .record(wire_frame, stamps, epoch);
        if let Some(sink) = &*self.timing_record.read().unwrap() {
            sink.record(crate::timing_record::Row::new(
                wire_frame,
                rtp_timestamp,
                stamps,
                epoch,
            ));
        }
    }
    pub fn new(launch: Launch<P, A>, config: Negotiated) -> Arc<Self> {
        let launch_peer = launch.peer.to_canonical();
        let bitrate = config.bitrate_kbps;
        let mode = crate::reconfigure::Mode::of(&config);
        let started = Instant::now();
        Arc::new(Self {
            encoder: Default::default(),
            capture_warnings: Default::default(),
            launch,
            config,
            stop: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            source_checked: AtomicBool::new(false),
            idr: AtomicBool::new(true),
            invalidation: Default::default(),
            recovery_wake: Default::default(),
            bitrate: AtomicU32::new(bitrate),
            adaptive_bitrate: Default::default(),
            link_rate: Default::default(),
            link_sent: crate::link_control::SentLog::new(started),
            pace_was_fast: AtomicBool::new(false),
            adaptive_video_pacing_bps: AtomicU64::new(0),
            automatic_pacing: Default::default(),
            automatic_pacing_on: AtomicBool::new(false),
            keyframe_loss: Default::default(),
            fec_percent: AtomicU32::new(
                crate::adaptive_fec::AdaptiveFec::default().percent() as u32
            ),
            adaptive_fec: Default::default(),
            client_fec: Default::default(),
            client_fec_on: AtomicBool::new(false),
            fec_split_on: AtomicBool::new(false),
            fec_split: AtomicU32::new(0),
            phase_sync: Default::default(),
            reconfigure: std::sync::Mutex::new(crate::reconfigure::Reconfiguration::new(mode)),
            rate_changes: Default::default(),
            stats: Stats::default(),
            started,
            output: std::sync::RwLock::new(String::new()),
            hdr_metadata: Default::default(),
            hdr_source: Default::default(),
            host_timings: Default::default(),
            link_notices: Default::default(),
            pipeline_rates: Default::default(),
            text_input: Default::default(),
            keyboard_layout: Default::default(),
            client_cursor: Default::default(),
            media_epoch: Default::default(),
            timing_record: Default::default(),
            encoder_focus: AtomicU8::new(crate::encoder_focus::Requested::Off.wire()),
            encoder_focus_state: Default::default(),
            roam: std::sync::Mutex::new(Roam {
                client: launch_peer,
                at: None,
                new_path: false,
            }),
            roams: AtomicU32::new(0),
            audio: Default::default(),
            audio_haptics: Default::default(),
            // The control peer stores what the stream allows when it
            // connects; WebRTC sessions have none.
            couch_menu: Arc::new(crate::couch_menu::Shared::refusing(
                "couch menu: this stream's control connection has not started",
            )),
        })
    }
    /// The client's IP: the launch's, or the one it last roamed to.
    pub fn client_ip(&self) -> IpAddr {
        self.roam.lock().unwrap().client
    }
    /// The client proved it moved to `client` (`crate::roaming`). Its media pings come
    /// from there from now on.
    pub fn roamed(&self, client: IpAddr, now: Instant) {
        *self.roam.lock().unwrap() = Roam {
            client: client.to_canonical(),
            at: Some(now),
            new_path: true,
        };
        self.roams.fetch_add(1, Ordering::AcqRel);
    }
    /// How often the client roamed.
    pub fn roams(&self) -> u32 {
        self.roams.load(Ordering::Acquire)
    }
    /// The client's link hint (`pyrowaveLinkMbps`), 0 once it roamed: the hint came
    /// with the ANNOUNCE and describes the path the client left.
    pub fn client_link_mbps(&self) -> u32 {
        if self.roams() == 0 {
            self.config.pyrowave_client_link_mbps
        } else {
            0
        }
    }
    /// The client's pace hint (`pyrowavePaceMbps`), 0 once it roamed, as
    /// [`Self::client_link_mbps`].
    pub fn client_pace_mbps(&self) -> u32 {
        if self.roams() == 0 {
            self.config.pyrowave_client_pace_mbps
        } else {
            0
        }
    }
    /// True once per roam, for the first video ping from the new path.
    pub fn take_new_path(&self) -> bool {
        std::mem::take(&mut self.roam.lock().unwrap().new_path)
    }
    /// The client's video now takes the path it roamed to: what pacing learned about
    /// the old path no longer holds. The automatic pacing estimate starts over, host
    /// bitrate adaptation forgets the old path's RTT baseline, and the keyframe-loss
    /// guard starts over without the old path's latch.
    pub fn new_network_path(&self) {
        self.configure_automatic_pacing(self.automatic_pacing_enabled());
        self.adaptive_bitrate.lock().unwrap().new_path();
        {
            let mut loss = self.keyframe_loss.lock().unwrap();
            let reports_losses = loss.reports_losses();
            *loss = crate::keyframe_loss::KeyframeLoss::new(loss.enabled());
            loss.set_reports_losses(reports_losses);
        }
        self.launch.warnings.clear("network_keyframe_loss");
    }
    /// Since when a stream without its client's media ping has waited for one: the
    /// stream's `start`, or the last roam, which drops the old path's media addresses.
    pub fn media_wait_start(&self, start: Instant) -> Instant {
        self.roam
            .lock()
            .unwrap()
            .at
            .map_or(start, |at| at.max(start))
    }
    pub fn stopping(&self) -> bool {
        self.stop.load(Ordering::Acquire)
    }
    /// The size and rate the stream runs at now: the negotiated ones until the
    /// client asks for others (0x5532).
    pub fn stream_mode(&self) -> crate::reconfigure::Mode {
        self.reconfigure.lock().unwrap().current()
    }
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Release)
    }
    pub fn fail(&self) {
        self.failed.store(true, Ordering::Release);
        self.stop();
    }
    pub fn failed(&self) -> bool {
        self.failed.load(Ordering::Acquire)
    }
    /// The termination reason the control stream sends. Moonlight closes
    /// quietly on 0x80030023 (the host closed the stream) and shows an error
    /// with the code otherwise; 0x80004005 is the generic failure HRESULT.
    pub fn termination_reason(&self) -> u32 {
        if self.failed() {
            0x8000_4005
        } else {
            0x8003_0023
        }
    }
    pub fn request_idr(&self) {
        self.stats.idr_requests.fetch_add(1, Ordering::Relaxed);
        // Intra-only frames already recover; feedback must not bypass cadence.
        if self.config.codec != 3 {
            self.idr.store(true, Ordering::Release);
            self.wake_recovery();
        }
    }
    /// A keyframe the host makes on its own initiative (a recreated encoder,
    /// a frame beyond the packet limit). Whatever the client requests around
    /// it is a reaction to the host, not evidence about the network.
    pub fn request_host_keyframe(&self) {
        self.keyframe_loss.lock().unwrap().host_keyframe();
        self.request_idr();
    }
    /// A keyframe a second after a reference invalidation the decoder may not
    /// have followed. It answers loss, so it is counted like loss.
    pub fn request_confirming_keyframe(&self) {
        self.keyframe_loss.lock().unwrap().loss_keyframe();
        self.idr.store(true, Ordering::Release);
    }
    /// A keyframe the client needs after losing frames: its request (0x0302),
    /// or an invalidation the encoder cannot repair with a reference frame.
    pub fn request_recovery_keyframe(&self) {
        let (latched, ignored) = {
            let mut loss = self.keyframe_loss.lock().unwrap();
            let latched = loss.requested(Instant::now());
            if self.config.codec == 3 {
                // Intra-only: no keyframe will answer it.
                loss.unanswerable();
            }
            (latched, loss.take_ignored())
        };
        self.report_keyframe_loss(latched, ignored);
        self.request_idr();
    }
    /// The client asked for a keyframe (0x0302): the same, and evidence for
    /// the bitrate controller when it is the network and not the host or the
    /// client's decode queue that caused the request.
    pub fn request_client_keyframe(&self) {
        self.link_request();
        self.request_recovery_keyframe();
    }
    /// The host could not send enough of a frame for the client to rebuild
    /// it, so the next frame is a keyframe. Moonlight, waiting for one,
    /// then recovers on that frame and sends no request of its own.
    pub fn request_send_loss_recovery(&self) {
        self.stats
            .send_loss_recoveries
            .fetch_add(1, Ordering::Relaxed);
        if self.config.codec != 3 {
            self.keyframe_loss.lock().unwrap().loss_keyframe();
            self.idr.store(true, Ordering::Release);
        }
    }
    pub fn request_invalidation(&self, first: u64, last: u64) {
        self.stats
            .reference_invalidations
            .fetch_add(1, Ordering::Relaxed);
        if self.config.codec == 3 {
            return;
        }
        self.link_invalidation(first, last);
        if first == 0 || first > last {
            self.request_recovery_keyframe();
            return;
        }
        let mut pending = self.invalidation.lock().unwrap();
        *pending = Some(pending.map_or((first, last), |(a, b)| (a.min(first), b.max(last))));
        drop(pending);
        self.wake_recovery();
    }
    fn wake_recovery(&self) {
        if let Some(wake) = self.recovery_wake.lock().unwrap().as_ref() {
            wake();
        }
    }
    pub fn record_fec_status(&self, payload: &[u8]) {
        let last_sent = self
            .stats
            .video_frame
            .load(Ordering::Acquire)
            .checked_sub(1)
            .map(|v| v as u32);
        let block = self
            .stats
            .performance
            .lock()
            .unwrap()
            .record_fec_status(payload, last_sent);
        if let Some(block) = block {
            // Parsing and sent-frame/block validation above succeeded. The
            // additional controller fence rejects feedback from an older rate.
            let frame = u32::from_be_bytes(payload[..4].try_into().unwrap());
            let period = self.stream_mode().period();
            self.link_status(payload, frame, block);
            if matches!(block, crate::adaptive_fec::Block::Unrecoverable) {
                let (latched, ignored) = {
                    let mut loss = self.keyframe_loss.lock().unwrap();
                    let latched =
                        loss.unrecoverable(frame, last_sent.unwrap(), period, Instant::now());
                    (latched, loss.take_ignored())
                };
                self.report_keyframe_loss(latched, ignored);
            }
            self.automatic_pacing.lock().unwrap().on_block(
                frame,
                last_sent.unwrap(),
                period,
                block,
            );
            self.adaptive_bitrate.lock().unwrap().on_block(block);
            let mut fec = self.adaptive_fec.lock().unwrap();
            if fec.enabled() {
                fec.on_block(Instant::now(), block);
                self.fec_percent
                    .store(fec.percent() as u32, Ordering::Release);
            }
        }
    }
    /// Starts the stream's FEC at the configured percentage (`fec_percentage`,
    /// 0-100), which adaptive FEC never exceeds. Off, or with a client that never
    /// reports, every frame uses the configured percentage. Returns it.
    pub fn configure_fec(&self, adaptive: bool, configured: usize) -> usize {
        let mut fec = self.adaptive_fec.lock().unwrap();
        *fec = if self.config.codec == 3 {
            crate::adaptive_fec::AdaptiveFec::pyrowave(adaptive, configured)
        } else {
            crate::adaptive_fec::AdaptiveFec::new(adaptive, configured)
        };
        let percent = fec.percent();
        self.fec_percent.store(percent as u32, Ordering::Release);
        percent
    }
    /// Starts the stream's FEC as the client chose it with `rlFecPercentage`:
    /// the host's own (`adaptive` and `configured`, as [`Self::configure_fec`]),
    /// a fixed share capped at `maximum` (`fec_max_percentage`), or statistical
    /// FEC up to `maximum` that keeps `configured` until the client's reports
    /// arrive. PyroWave keeps its own protection. Returns the first percentage.
    pub fn configure_fec_choice(
        &self,
        choice: crate::extensions::FecChoice,
        adaptive: bool,
        configured: usize,
        maximum: u8,
    ) -> usize {
        use crate::extensions::FecChoice;
        let mut client = self.client_fec.lock().unwrap();
        *client = None;
        self.client_fec_on.store(false, Ordering::Release);
        self.fec_split_on.store(false, Ordering::Release);
        if self.config.codec == 3 || choice == FecChoice::Host {
            drop(client);
            return self.configure_fec(adaptive, configured);
        }
        let fallback = configured.min(255) as u8;
        let percent = match choice {
            FecChoice::Fixed(percent) => percent.min(maximum),
            _ => {
                *client = Some(crate::statistical_fec::StatisticalFec::new(
                    maximum,
                    fallback,
                    self.config.min_fec,
                    Instant::now(),
                ));
                self.client_fec_on.store(true, Ordering::Release);
                fallback
            }
        };
        *self.adaptive_fec.lock().unwrap() = crate::adaptive_fec::AdaptiveFec::fixed(percent);
        self.fec_percent
            .store(u32::from(percent), Ordering::Release);
        self.fec_split.store(u32::from(percent), Ordering::Release);
        self.fec_split_on.store(true, Ordering::Release);
        usize::from(percent)
    }
    /// The encoder's bitrate for the next frame. With a client-chosen FEC
    /// share F, `bitrate` is the video budget and the encoder gets
    /// budget × 100 / (100 + F), so video and parity together fit it. F moves
    /// at a `fence`, an IDR or a new encoder, and under
    /// `rlFecPercentage=auto` also after a report (see
    /// [`Self::follow_fec_report`]): the encoder takes the new rate on the next
    /// frame, without a keyframe. Host bitrate adaptation already budgets FEC:
    /// while it runs, the encoder gets `bitrate` as it is.
    ///
    /// The split follows Foundation Sunshine's `encoder_bitrate_from_total_bitrate`
    /// in `src/video.cpp` (AlkaidLab/foundation-sunshine 880f1ef, GPL-3).
    pub fn encoder_bitrate(&self, fence: bool) -> u32 {
        let budget = self.bitrate.load(Ordering::Acquire);
        if !self.fec_split_on.load(Ordering::Acquire) {
            return budget;
        }
        if fence {
            self.fec_split
                .store(self.fec_split_share(), Ordering::Release);
        }
        fec_split_kbps(budget, self.fec_split.load(Ordering::Acquire))
    }
    /// The FEC share the encoder's budget leaves room for now.
    fn fec_split_share(&self) -> u32 {
        if self.adaptive_bitrate.lock().unwrap().active() {
            0
        } else {
            self.fec_percent() as u32
        }
    }
    /// Moves the split after an accepted statistical FEC report instead of
    /// waiting for the next fence: at the first report, which replaces the
    /// fallback share the stream started with, whenever the share rises (so
    /// video and parity stay within the budget), and when it falls by more
    /// than [`FEC_SPLIT_HYSTERESIS`] points, so a noisy estimate doesn't step
    /// the rate every second.
    fn follow_fec_report(&self, first: bool) {
        if !self.fec_split_on.load(Ordering::Acquire) {
            return;
        }
        let share = self.fec_split_share();
        let latched = self.fec_split.load(Ordering::Acquire);
        if first || share > latched || latched - share > FEC_SPLIT_HYSTERESIS {
            self.fec_split.store(share, Ordering::Release);
        }
    }
    /// Takes a 0x5539 FEC report: the report and whether statistical FEC
    /// used it (only under `rlFecPercentage=auto`), or why it was refused.
    pub fn record_fec_report(
        &self,
        payload: &[u8],
    ) -> std::result::Result<(crate::statistical_fec::FecReport, bool), String> {
        self.record_fec_report_at(payload, Instant::now())
    }
    fn record_fec_report_at(
        &self,
        payload: &[u8],
        now: Instant,
    ) -> std::result::Result<(crate::statistical_fec::FecReport, bool), String> {
        let result = self.take_fec_report(payload, now);
        self.stats
            .performance
            .lock()
            .unwrap()
            .record_fec_loss_report(result.is_ok());
        result
    }
    fn take_fec_report(
        &self,
        payload: &[u8],
        now: Instant,
    ) -> std::result::Result<(crate::statistical_fec::FecReport, bool), String> {
        let report = crate::statistical_fec::FecReport::decode(payload)
            .ok_or_else(|| format!("malformed ({} bytes)", payload.len()))?;
        let (first, pending) = {
            let mut client = self.client_fec.lock().unwrap();
            let Some(fec) = client.as_mut() else {
                return Ok((report, false));
            };
            let first = !fec.reported();
            (first, fec.observe(now, &report).map_err(|e| e.to_string())?)
        };
        // The tables are built without the lock every frame takes: frames keep
        // the previous estimate until they are in.
        let estimate = pending.map(crate::statistical_fec::PendingEstimate::build);
        let mut client = self.client_fec.lock().unwrap();
        let Some(fec) = client.as_mut() else {
            // The stream left statistical FEC meanwhile.
            return Ok((report, true));
        };
        if let Some(estimate) = estimate {
            fec.install(estimate);
        }
        let percent = fec.percent(now, self.typical_fec_shards());
        self.fec_percent.store(percent as u32, Ordering::Release);
        drop(client);
        self.follow_fec_report(first);
        Ok((report, true))
    }
    /// Data shards of an average frame at the current bitrate: statistical
    /// FEC's percentage for frames sent in parts and for pacing estimates.
    fn typical_fec_shards(&self) -> usize {
        self.fec_shards(self.typical_frame_bytes())
    }
    /// Bytes of an average frame at the current bitrate.
    fn typical_frame_bytes(&self) -> usize {
        (u64::from(self.bitrate.load(Ordering::Relaxed)) * 1000
            / 8
            / u64::from(self.config.fps.max(1))) as usize
    }
    /// Data shards of a video frame of `payload_bytes`, as the packetizer splits it.
    fn fec_shards(&self, payload_bytes: usize) -> usize {
        (payload_bytes + 8).div_ceil(self.config.packet_size.saturating_sub(16).max(1))
    }
    /// The FEC percentage for a whole video frame of `payload_bytes` about to
    /// be packetized: under `rlFecPercentage=auto` the statistical choice for
    /// its size, and for a recovery frame at least the configured percentage
    /// as [`Self::frame_fec_percent`] gives it; otherwise
    /// [`Self::frame_fec_percent`].
    pub fn video_fec_percent(&self, recovery: bool, payload_bytes: usize) -> usize {
        if self.client_fec_on.load(Ordering::Acquire) {
            let statistical = self
                .client_fec
                .lock()
                .unwrap()
                .as_ref()
                .map(|fec| fec.percent(Instant::now(), self.fec_shards(payload_bytes)));
            if let Some(percent) = statistical {
                return if recovery {
                    percent.max(self.adaptive_fec.lock().unwrap().ceiling())
                } else {
                    percent
                };
            }
        }
        self.frame_fec_percent(recovery)
    }
    /// Whether the client sizes FEC per frame from its own reports
    /// (`rlFecPercentage=auto`) rather than taking a fixed percentage.
    pub fn statistical_fec(&self) -> bool {
        self.client_fec_on.load(Ordering::Acquire)
    }
    /// Under `rlFecPercentage=auto`, the largest recovery frame whose own
    /// [`Self::video_fec_percent`] still applies in full within Moonlight's
    /// four blocks (with no parity, the four-block packet limit), as
    /// [`crate::packet::VideoPacketizer::fec_whole_payload_limit`] finds it for
    /// a fixed percentage. The statistical choice depends on the frame's size,
    /// so every size is tried, from the largest down. `None` without
    /// statistical FEC.
    pub fn statistical_fec_whole_payload(&self, now: Instant) -> Option<usize> {
        if !self.client_fec_on.load(Ordering::Acquire) {
            return None;
        }
        let slice = self.config.packet_size.saturating_sub(16).max(1);
        let minimum = self.config.min_fec;
        let ceiling = self.adaptive_fec.lock().unwrap().ceiling();
        let client = self.client_fec.lock().unwrap();
        let fec = client.as_ref()?;
        let shards = (1..=4 * 255)
            .rev()
            .find(|&shards| {
                let percent = fec.percent(now, shards).max(ceiling);
                crate::packet::conventional_fec(shards, percent, minimum).1 >= percent
            })
            .unwrap_or(1);
        Some((shards * slice).saturating_sub(8))
    }
    /// The FEC percentage to budget a recovery frame's parity with before it
    /// is encoded: [`Self::video_fec_percent`] for an average frame.
    pub fn recovery_fec_percent(&self) -> usize {
        if self.client_fec_on.load(Ordering::Acquire) {
            return self.video_fec_percent(true, self.typical_frame_bytes());
        }
        self.frame_fec_percent(true)
    }
    /// The FEC percentage for the next video frame.
    pub fn fec_percent(&self) -> usize {
        self.fec_percent.load(Ordering::Acquire) as usize
    }
    /// The FEC percentage for a video frame about to be packetized. A
    /// keyframe or the frame after a reference invalidation (`recovery`)
    /// repairs a loss the client is waiting on, and a keyframe is several
    /// frames' worth of packets: losing it costs another round trip and
    /// another keyframe. Such a frame gets the configured percentage, however
    /// far a clean link let adaptive FEC fall (to 5%) before the loss.
    pub fn frame_fec_percent(&self, recovery: bool) -> usize {
        let current = self.fec_percent();
        if recovery {
            current.max(self.adaptive_fec.lock().unwrap().ceiling())
        } else {
            current
        }
    }
    /// Configures host bitrate adaptation; again after a roam, to forget the old path
    /// and the client's hints about it.
    pub fn configure_adaptive_bitrate(&self, enabled: bool, maximum_kbps: u32) {
        let mut stream = self.config.clone();
        stream.pyrowave_client_link_mbps = self.client_link_mbps();
        stream.pyrowave_client_pace_mbps = self.client_pace_mbps();
        self.adaptive_bitrate.lock().unwrap().configure(
            enabled,
            &stream,
            maximum_kbps,
            self.fec_percent(),
        );
    }
    pub fn set_client_bitrate(&self, kbps: u32) {
        let mut controller = self.adaptive_bitrate.lock().unwrap();
        controller.client_takeover();
        self.adaptive_video_pacing_bps.store(0, Ordering::Release);
        {
            // The link controller's own writes take this lock too, so none
            // lands after the client's value.
            let mut link = self.link_rate.lock().unwrap();
            link.takeover();
            self.link_sent.set_live(false);
            self.bitrate.store(kbps, Ordering::Release);
        }
        self.launch.warnings.clear("host_bitrate_budget");
        self.launch.warnings.clear(LINK_WARNING);
        self.link_notices.lock().unwrap().note_full(kbps);
    }
    pub fn adaptive_pacing_bps(&self, configured_bps: u64) -> u64 {
        let limit = self.adaptive_video_pacing_bps.load(Ordering::Acquire);
        if limit == 0 {
            configured_bps
        } else {
            configured_bps.min(limit)
        }
    }
    /// Starts the link-rate controller for the stream: `enabled` is
    /// `Config::link_rate_adaptation`, `maximum_kbps` the user's `max_bitrate`.
    pub fn configure_link_rate(&self, enabled: bool, maximum_kbps: u32) {
        self.rebuild_link_rate(&self.config, enabled, maximum_kbps);
    }
    /// A controller for a stream whose mode changed (0x5532). A bitrate it had
    /// lowered goes back to the requested one; what it learned about the link
    /// is forgotten, and a client that took the bitrate over keeps it.
    pub fn rebuild_link_rate(&self, stream: &Negotiated, enabled: bool, maximum_kbps: u32) {
        let mut link = self.link_rate.lock().unwrap();
        if let Some(kbps) = link.restore.take() {
            self.bitrate.store(kbps, Ordering::Release);
        }
        let rate = link
            .rate
            .rebuild(enabled, stream, maximum_kbps, self.fec_percent());
        self.link_sent
            .set_live(rate.state() != crate::link_rate::State::Off);
        *link = crate::link_control::LinkControl::new(rate, self.link_sent.head());
        drop(link);
        self.launch.warnings.clear(LINK_WARNING);
        self.link_notices
            .lock()
            .unwrap()
            .note_full(self.bitrate.load(Ordering::Acquire));
    }
    /// A frame the video sender sent: its wire frame number, packets (data and
    /// parity) and wire bytes. Lock-free, so any sender path may call it.
    pub fn record_link_sent(&self, wire_frame: u32, packets: usize, wire_bytes: u64) {
        self.record_link_sent_at(wire_frame, packets, wire_bytes, Instant::now());
    }
    fn record_link_sent_at(&self, wire_frame: u32, packets: usize, wire_bytes: u64, at: Instant) {
        self.link_sent.record(
            wire_frame,
            packets.min(u32::MAX as usize) as u32,
            wire_bytes,
            at,
        );
    }
    /// A client request for a keyframe (E4).
    fn link_request(&self) {
        self.link_request_with(|rate, network, now| rate.on_keyframe_request(network, now));
    }
    /// A client request to invalidate frames `first..=last` (E4), counted once
    /// per incident.
    fn link_invalidation(&self, first: u64, last: u64) {
        self.link_request_with(|rate, network, now| {
            rate.on_invalidation(first, last, network, now)
        });
    }
    fn link_request_with(
        &self,
        count: impl FnOnce(&mut crate::link_rate::LinkRate, bool, Instant),
    ) {
        if !self.link_sent.live() {
            return;
        }
        let now = Instant::now();
        // Every PyroWave picture is intra: no keyframe answers a request, and
        // what it lost is in the FEC status already, so a request is no
        // evidence of its own.
        let network =
            self.config.codec != 3 && self.keyframe_loss.lock().unwrap().network_request(now);
        let mut link = self.link_rate.lock().unwrap();
        link.drain(&self.link_sent);
        count(&mut link.rate, network, now);
    }
    /// One validated, deduplicated FEC block (E1 and E2).
    fn link_status(&self, payload: &[u8], frame: u32, block: crate::adaptive_fec::Block) {
        if !self.link_sent.live() {
            return;
        }
        let Ok(status) = crate::fec_status::Status::parse(payload) else {
            return;
        };
        let now = Instant::now();
        let (network, abandoned, keyframe) = {
            let loss = self.keyframe_loss.lock().unwrap();
            (
                matches!(block, crate::adaptive_fec::Block::Unrecoverable)
                    && loss.network_loss(frame, now),
                loss.is_abandoned(frame),
                loss.is_keyframe(frame),
            )
        };
        let mut link = self.link_rate.lock().unwrap();
        link.drain(&self.link_sent);
        // Packets of a frame the host stopped sending are its own loss,
        // counted once on send (E3), not as the network's (E2).
        if !abandoned {
            link.rate.on_status(&status, now);
        }
        if network && link.rate.frame_lost(&status) {
            link.rate.on_unrecoverable(frame, keyframe, now);
        }
    }
    /// The running counts behind the pipeline frame rates.
    pub fn pipeline_counts(&self) -> crate::pipeline_rates::Counts {
        self.stats
            .pipeline
            .counts(self.stats.frames.load(Ordering::Relaxed))
    }

    /// The console's pipeline frame rates; `None` until a window is complete.
    fn console_pipeline_rates(&self) -> Option<crate::pipeline_rates::PipelineRates> {
        let counts = self.pipeline_counts();
        self.stats
            .pipeline_console
            .lock()
            .unwrap()
            .sample(
                Instant::now(),
                counts,
                crate::pipeline_rates::CONSOLE_WINDOW,
            )
            .as_ref()
            .map(crate::pipeline_rates::PipelineRates::from)
    }

    /// The link-rate state for the session API and the `stream timings` line.
    pub fn link_rate_snapshot(&self) -> serde_json::Value {
        self.link_rate.lock().unwrap().rate.snapshot(Instant::now())
    }
    /// Call from the feedback tick. Applies the controller's bitrate and says
    /// so, loudly: a WARN on leaving the requested rate, a session warning for
    /// the console, an INFO for later changes. Returns `(old, new)` kbps when
    /// the encoder bitrate changed.
    pub fn tick_link_rate(&self, now: Instant, rtt: Duration) -> Option<(u32, u32)> {
        if !self.link_sent.live() {
            return None;
        }
        let mut link = self.link_rate.lock().unwrap();
        link.drain(&self.link_sent);
        let period = self.stream_mode().period();
        let current = self.bitrate.load(Ordering::Acquire);
        let busy = link.busy(
            now,
            self.stats.frames.load(Ordering::Relaxed),
            self.stats.bytes.load(Ordering::Relaxed),
            period,
            current,
            if self.config.codec == 3 {
                crate::link_control::PYROWAVE_BYTE_SHARE
            } else {
                crate::link_control::BYTE_SHARE
            },
        );
        let target = link
            .rate
            .tick(now, rtt, self.fec_percent(), period, current, busy);
        let applied = match target {
            Some(kbps) => {
                link.restore.get_or_insert(current);
                kbps
            }
            None => link.restore.take().unwrap_or(current),
        };
        if applied != current {
            self.bitrate.store(applied, Ordering::Release);
        }
        // Say it once the lock is released: logging and the warning list must
        // not run under it.
        let report = link
            .rate
            .take_change()
            .map(|change| note_link_change(&mut link, now, change));
        drop(link);
        if let Some(report) = report {
            self.link_notices.lock().unwrap().note(link_notice(&report));
            self.announce_link_change(report);
        }
        (applied != current).then_some((current, applied))
    }
    /// The WARN, the console warning or the INFO for a change.
    fn announce_link_change(&self, report: LinkReport) {
        use crate::link_rate::ChangeKind;
        let LinkReport {
            change,
            was_limited,
            warned,
            no_room,
            ceiling_kbps: ceiling,
            loss_share,
            lowest_kbps,
            log_info,
        } = report;
        let e = change.evidence;
        let kind = match change.kind {
            ChangeKind::StepDown => "step_down",
            ChangeKind::Probe => "probe",
            ChangeKind::ProbeFailed => "probe_failed",
            ChangeKind::Floor => "floor",
            ChangeKind::EncoderFloor => "encoder_floor",
            ChangeKind::Full => "full",
        };
        match change.kind {
            ChangeKind::StepDown | ChangeKind::Floor | ChangeKind::EncoderFloor => {
                let message = match change.kind {
                    ChangeKind::Floor if no_room => format!(
                        "This network loses PyroWave video at {}, and PyroWave cannot go lower at this resolution and frame rate and keep a usable picture, so the picture will keep breaking up. HEVC or AV1 will hold up better on this device; or check the network path (Wi-Fi signal, cable, router).",
                        mbps(change.to_kbps)
                    ),
                    ChangeKind::Floor if self.config.codec == 3 => format!(
                        "The network to this device cannot carry even {} of video, the lowest bitrate at which PyroWave keeps a usable picture, so the picture will keep breaking up. Use HEVC or AV1 for this device, or check the network path (Wi-Fi signal, cable, router).",
                        mbps(change.to_kbps)
                    ),
                    ChangeKind::Floor => format!(
                        "The network to this device cannot carry even {} of video, the lowest bitrate the host uses, so the picture will keep breaking up. Check the network path (Wi-Fi signal, cable, router) or lower the resolution or frame rate in Moonlight.",
                        mbps(change.to_kbps)
                    ),
                    ChangeKind::EncoderFloor => format!(
                        "The network to this device still loses video at {}, and the encoder does not go lower for this picture, so the host stopped lowering the bitrate there and the picture will keep breaking up. Check the network path (Wi-Fi signal, cable, router) or lower the resolution or frame rate in Moonlight.",
                        mbps(change.to_kbps)
                    ),
                    _ => {
                        let loss = if change.reason == "queue" {
                            "the network was holding video back in a growing queue".to_string()
                        } else if loss_share > 0. {
                            format!("{:.0} % of packets lost", loss_share * 100.)
                        } else {
                            "video frames were lost".to_string()
                        };
                        format!(
                            "The network to this device could not carry {} of video ({loss}). The host lowered the bitrate, as low as {} so far, and raises it again as the link allows. Lower the bitrate in Moonlight to avoid this.",
                            mbps(ceiling),
                            mbps(lowest_kbps)
                        )
                    }
                };
                self.launch.warnings.set(LINK_WARNING, message);
            }
            ChangeKind::Full => self.launch.warnings.clear(LINK_WARNING),
            ChangeKind::Probe | ChangeKind::ProbeFailed => {}
        }
        match change.kind {
            ChangeKind::StepDown if !was_limited => tracing::warn!(
                stream_id = %self.launch.id,
                link_rate_change = kind,
                reason = change.reason,
                from_kbps = change.from_kbps,
                to_kbps = change.to_kbps,
                ceiling_kbps = ceiling,
                delivered_kbps = change.delivered_kbps,
                unrecoverable_frames = e.unrecoverable_frames,
                send_losses = e.send_losses,
                keyframe_requests = e.keyframe_requests,
                loss_windows = e.loss_windows,
                loss_share_percent = e.loss_share * 100.,
                "the network to this device cannot carry the stream; lowering the video bitrate"
            ),
            ChangeKind::Floor if warned => tracing::warn!(
                stream_id = %self.launch.id,
                link_rate_change = kind,
                reason = change.reason,
                from_kbps = change.from_kbps,
                to_kbps = change.to_kbps,
                delivered_kbps = change.delivered_kbps,
                unrecoverable_frames = e.unrecoverable_frames,
                send_losses = e.send_losses,
                keyframe_requests = e.keyframe_requests,
                loss_share_percent = e.loss_share * 100.,
                pyrowave_no_room = no_room,
                "the network to this device cannot carry the stream even at the lowest video bitrate"
            ),
            ChangeKind::EncoderFloor => tracing::warn!(
                stream_id = %self.launch.id,
                link_rate_change = kind,
                from_kbps = change.from_kbps,
                to_kbps = change.to_kbps,
                delivered_kbps = change.delivered_kbps,
                "the encoder sends more than the video bitrate asked for; treating what it sends as its lowest rate"
            ),
            _ if log_info => {
                if change.kind == ChangeKind::Full {
                    tracing::info!(
                        stream_id = %self.launch.id,
                        link_rate_change = kind,
                        to_kbps = change.to_kbps,
                        "video bitrate back at the requested rate"
                    );
                } else {
                    tracing::info!(
                        stream_id = %self.launch.id,
                        link_rate_change = kind,
                        reason = change.reason,
                        from_kbps = change.from_kbps,
                        to_kbps = change.to_kbps,
                        delivered_kbps = change.delivered_kbps,
                        "video bitrate changed to follow the network"
                    );
                }
            }
            _ => {}
        }
    }
    pub fn configure_automatic_pacing(&self, enabled: bool) {
        *self.automatic_pacing.lock().unwrap() =
            crate::adaptive_pacing::AdaptivePacing::new(enabled);
        self.automatic_pacing_on.store(enabled, Ordering::Release);
    }
    /// Whether the client reports a wireless route (`x-rl-video[0].clientWireless`)
    /// and no adaptive controller owns this stream. Then the route base is
    /// paced as Wi-Fi from the first frame. When the controller owns the stream
    /// the base stays the host route's, so the controller starts conservatively
    /// at its own seed and can probe up toward the real ceiling.
    pub fn client_wireless_sets_route_pacing(&self) -> bool {
        self.config.client_wireless && !self.automatic_pacing_enabled()
    }
    /// Read per frame, so it is the flag set at stream start, not the mutex.
    pub fn automatic_pacing_enabled(&self) -> bool {
        self.automatic_pacing_on.load(Ordering::Acquire)
    }
    pub fn video_pacing_bps(&self, base: u64, needed: u64) -> u64 {
        let paced = self.automatic_pacing.lock().unwrap().rate(
            base,
            needed,
            self.bitrate.load(Ordering::Acquire),
        );
        self.adaptive_pacing_bps(paced)
    }
    /// Encoder-side budget queries must not change the sender's feedback
    /// state when recovery parity differs from the current ordinary-frame FEC.
    pub fn preview_video_pacing_bps(&self, base: u64, needed: u64) -> u64 {
        let paced = self.automatic_pacing.lock().unwrap().preview_rate(
            base,
            needed,
            self.bitrate.load(Ordering::Acquire),
        );
        self.adaptive_pacing_bps(paced)
    }
    /// Pacing slows for the session once the client loses a keyframe or
    /// repeatedly reports unrecoverable ordinary frames
    /// (`keyframe_loss_pacing`), including PyroWave's independent intra frames.
    pub fn configure_keyframe_loss(&self, enabled: bool) {
        self.configure_remembered_keyframe_loss(enabled, false);
    }
    /// Restore only an eligible route's recent loss history. Explicit pacing,
    /// disabled adaptation and PyroWave retain their own policies.
    pub fn configure_remembered_keyframe_loss(&self, enabled: bool, remembered: bool) {
        let remembered = remembered && enabled && self.config.codec != 3;
        let mut loss = if remembered {
            crate::keyframe_loss::KeyframeLoss::remembered("remembered from the previous session")
        } else {
            crate::keyframe_loss::KeyframeLoss::new(enabled)
        };
        loss.set_reports_losses(
            self.config.moonlight_features & crate::rtsp::FEC_STATUS_FEATURE != 0,
        );
        *self.keyframe_loss.lock().unwrap() = loss;
        if remembered {
            tracing::info!(stream_id = %self.launch.id, "pacing video like Wi-Fi from the first frame: remembered from the previous session");
            self.launch.warnings.set("network_keyframe_loss", "This device lost video frames in a previous stream at this address. Video starts at the Wi-Fi pacing rate to limit packet bursts without reducing the encoder bitrate. This memory expires after 24 hours and the host periodically tests the fast rate. Reset it in Devices after changing the network.");
        } else {
            self.launch.warnings.clear("network_keyframe_loss");
        }
    }
    fn report_keyframe_loss(
        &self,
        latched: Option<crate::keyframe_loss::Latched>,
        ignored: Option<String>,
    ) {
        if let Some(why) = ignored {
            tracing::info!(stream_id = %self.launch.id, why, "video loss evidence not counted");
        }
        if let Some(crate::keyframe_loss::Latched {
            reason,
            detail,
            kind,
            confirmed,
            from,
            host_only,
        }) = latched
        {
            use crate::keyframe_loss::LatchKind;
            if self.link_sent.live() {
                let mut link = self.link_rate.lock().unwrap();
                link.drain(&self.link_sent);
                // A provisional latch confirmed by more evidence is the same
                // latch, already counted when it began.
                link.rate.on_latch(
                    Instant::now(),
                    self.pace_was_fast.load(Ordering::Relaxed),
                    !confirmed,
                );
            }
            if confirmed && matches!(from, Some(LatchKind::Send | LatchKind::Sustained)) {
                tracing::warn!(stream_id = %self.launch.id, reason, detail, "the client reported video loss after the host's send loss; pacing video like a Wi-Fi route for the rest of the stream");
            } else if confirmed {
                tracing::warn!(stream_id = %self.launch.id, reason, detail, "more video loss after the start-up keyframe; pacing video like a Wi-Fi route for the rest of the stream");
            } else if kind == LatchKind::Send {
                tracing::warn!(stream_id = %self.launch.id, reason, detail, "the host's socket refused video packets; pacing video like a Wi-Fi route until it has been clean for 10 s");
            } else if kind == LatchKind::Startup {
                tracing::warn!(stream_id = %self.launch.id, reason, detail, "the stream's first keyframe was lost; pacing video like a Wi-Fi route until it has been clean for 10 s");
            } else if host_only {
                // After a lift, or on a remembered route: held, never saved.
                tracing::warn!(stream_id = %self.launch.id, reason, detail, "the host's socket refused video packets in a stream that already slowed down; pacing video like a Wi-Fi route for the rest of the stream");
            } else {
                tracing::warn!(stream_id = %self.launch.id, reason, detail, "video frames are not arriving; pacing video like a Wi-Fi route for the rest of the stream");
            }
            if kind == LatchKind::Startup {
                self.launch.warnings.set("network_keyframe_loss", format!("The first video frame was lost on the way to this device ({reason}), so video is paced like Wi-Fi for now. This limits packet bursts without reducing the encoder bitrate. The host returns to the normal rate after 10 seconds without further loss."));
            } else if kind == LatchKind::Send {
                self.launch.warnings.set("network_keyframe_loss", format!("The host's network adapter refused video packets ({reason}), so video is paced like Wi-Fi for now. This limits packet bursts without reducing the encoder bitrate. The host returns to the normal rate after 10 seconds without further loss."));
            } else if host_only {
                self.launch.warnings.set("network_keyframe_loss", format!("The host's network adapter refused video packets ({reason}), so video is paced like Wi-Fi for the rest of the stream. This limits packet bursts without reducing the encoder bitrate. The device reported no loss; check the host's network adapter and its driver if this continues."));
            } else {
                self.launch.warnings.set("network_keyframe_loss", format!("Video frames were lost on the way to this device ({reason}), so video is now paced like Wi-Fi for the rest of the stream. This limits packet bursts without reducing the encoder bitrate. Check the network path if loss continues."));
            }
        }
    }
    /// Read the loss guard without recording a frame that has not been sent.
    /// Sender selection uses this before deciding whether to send inline.
    pub fn keyframe_loss_rate_bps(&self, base: u64, safe: u64) -> u64 {
        self.keyframe_loss.lock().unwrap().bps(base, safe)
    }
    /// The host stopped sending this frame part way: loss on send. It counts
    /// toward slower pacing under its own reason, and the client's report of
    /// the same frame is not counted again.
    pub fn keyframe_abandoned(&self, wire_frame: u32) {
        self.stats
            .frames_lost_on_send
            .fetch_add(1, Ordering::Relaxed);
        let period = self.stream_mode().period();
        let (latched, ignored) = {
            let mut loss = self.keyframe_loss.lock().unwrap();
            (
                loss.abandoned(wire_frame, period, Instant::now()),
                loss.take_ignored(),
            )
        };
        if self.link_sent.live() {
            let mut link = self.link_rate.lock().unwrap();
            link.drain(&self.link_sent);
            link.rate.on_send_loss(wire_frame, Instant::now());
        }
        self.report_keyframe_loss(latched, ignored);
    }
    pub fn keyframe_loss_active(&self) -> bool {
        self.keyframe_loss.lock().unwrap().latched()
    }
    pub fn keyframe_loss_reason(&self) -> Option<&'static str> {
        self.keyframe_loss.lock().unwrap().reason()
    }
    pub fn keyframe_loss_observed_at(&self) -> Option<Instant> {
        self.keyframe_loss.lock().unwrap().observed_at()
    }
    /// When loss last proved sustained: the only loss the next session
    /// inherits. A provisional (start-up) latch has none.
    pub fn keyframe_loss_sustained_at(&self) -> Option<Instant> {
        self.keyframe_loss.lock().unwrap().sustained_at()
    }
    /// Any latch happened in this stream, even a provisional one that lifted.
    pub fn keyframe_loss_ever_latched(&self) -> bool {
        self.keyframe_loss.lock().unwrap().ever_latched()
    }
    /// The pacing rate for the frame about to be sent as `wire_frame`: the
    /// route policy's `base`, or after confirmed losses at most `safe`, the
    /// Wi-Fi route's rate for this stream.
    pub fn keyframe_loss_bps(&self, base: u64, safe: u64, wire_frame: u32, keyframe: bool) -> u64 {
        self.keyframe_loss_bps_at(base, safe, wire_frame, keyframe, Instant::now())
    }
    fn keyframe_loss_bps_at(
        &self,
        base: u64,
        safe: u64,
        wire_frame: u32,
        keyframe: bool,
        now: Instant,
    ) -> u64 {
        let (bps, lifted) = {
            let mut loss = self.keyframe_loss.lock().unwrap();
            loss.sending_at(wire_frame, keyframe, now);
            (loss.bps(base, safe), loss.take_lifted())
        };
        // Fast is what was applied to this frame, not what the route allows.
        let fast = bps > safe;
        self.pace_was_fast.store(fast, Ordering::Relaxed);
        // Every PyroWave picture is intra and its loss guard is off, so no
        // latch watch needs them, and the sender must not wait for this lock.
        if keyframe && self.config.codec != 3 && self.link_sent.live() {
            self.link_rate
                .lock()
                .unwrap()
                .rate
                .on_keyframe_sent(wire_frame, now);
        }
        if let Some(why) = lifted {
            tracing::info!(stream_id = %self.launch.id, why, "returning to the route's pacing rate: the loss did not recur");
            self.launch.warnings.clear("network_keyframe_loss");
            if self.link_sent.live() {
                let mut link = self.link_rate.lock().unwrap();
                link.drain(&self.link_sent);
                link.rate.on_unlatch(now, fast);
            }
        }
        bps
    }
    pub fn record_control_ping(&self, now: Instant, payload: &[u8]) {
        self.adaptive_bitrate.lock().unwrap().on_ping(now, payload);
        if self.link_sent.live() {
            self.link_rate.lock().unwrap().rate.on_ping(now, payload);
        }
        self.automatic_pacing.lock().unwrap().on_ping(now, payload);
    }
    pub fn tick_automatic_pacing(&self, now: Instant, rtt: Duration) -> Option<(u64, u64)> {
        let period = self.stream_mode().period();
        self.automatic_pacing.lock().unwrap().tick(
            now,
            crate::adaptive_pacing::Traffic {
                frames: self.stats.frames.load(Ordering::Relaxed),
                bytes: self.stats.bytes.load(Ordering::Relaxed),
                packets: self.stats.packets.load(Ordering::Relaxed),
                send_losses: self.stats.send_loss_recoveries.load(Ordering::Relaxed),
            },
            self.stats
                .video_frame
                .load(Ordering::Acquire)
                .checked_sub(1)
                .map(|v| v as u32),
            period,
            self.bitrate.load(Ordering::Acquire),
            rtt,
        )
    }
    pub fn reserve_controller_pcm(&self, streams: usize) {
        let mut controller = self.adaptive_bitrate.lock().unwrap();
        controller.reserve_controller_pcm(streams, self.fec_percent(), self.stream_mode().period());
        self.adaptive_video_pacing_bps
            .store(controller.video_pacing_bps(), Ordering::Release);
    }
    pub fn tick_adaptive_bitrate(&self, now: Instant, rtt: Duration) -> Option<(u32, u32)> {
        let mut controller = self.adaptive_bitrate.lock().unwrap();
        let previous = self.bitrate.load(Ordering::Acquire);
        let target = controller.tick(
            now,
            crate::adaptive_bitrate::Traffic {
                frames: self.stats.frames.load(Ordering::Relaxed),
                bytes: self.stats.bytes.load(Ordering::Relaxed),
                send_losses: self.stats.send_loss_recoveries.load(Ordering::Relaxed),
            },
            rtt,
            self.fec_percent(),
            self.stream_mode().period(),
            previous,
        );
        self.adaptive_video_pacing_bps
            .store(controller.video_pacing_bps(), Ordering::Release);
        if controller.budget_unusable() {
            self.launch.warnings.set("host_bitrate_budget", "Host bitrate adaptation cannot fit the negotiated audio and minimum video packets inside this network budget, so it is inactive. Use stereo audio, reduce FPS or raise the configured bitrate.");
        } else {
            self.launch.warnings.clear("host_bitrate_budget");
        }
        let target = target?;
        if target == previous {
            return None;
        }
        self.bitrate.store(target, Ordering::Release);
        Some((previous, target))
    }
    /// Lets adaptive FEC fall on a clean link; call regularly. Returns the
    /// `(old, new)` percentage to log, at most once a second.
    pub fn tick_fec(&self, now: Instant) -> Option<(usize, usize)> {
        if let Some(fec) = self.client_fec.lock().unwrap().as_mut() {
            // Reports that stopped hand the stream back to the host's percentage.
            let percent = fec.percent(now, self.typical_fec_shards());
            self.fec_percent.store(percent as u32, Ordering::Release);
            return fec.log_due(now, percent);
        }
        let mut fec = self.adaptive_fec.lock().unwrap();
        if !fec.enabled() {
            return None;
        }
        fec.tick(now);
        self.fec_percent
            .store(fec.percent() as u32, Ordering::Release);
        fec.log_due(now)
    }
    /// Records the captured source's HDR metadata for the encoder and client.
    pub fn set_display_hdr_metadata(&self, display: crate::hdr::Metadata) -> crate::hdr::Metadata {
        self.hdr_source.lock().unwrap().display = Some(display);
        *self.hdr_metadata.write().unwrap() = Some(display);
        display
    }
    /// Record receiver capabilities for diagnostics. They describe the output
    /// panel, not the source pixels, so must not update stream HDR metadata;
    /// only the receiver display peak the encoder is told follows them.
    pub fn record_display_caps(&self, payload: &[u8]) -> Option<crate::display_caps::Update> {
        self.hdr_source.lock().unwrap().on_payload(payload)
    }
    /// The receiver's display peak, for dynamic metadata that names a target
    /// display ([`crate::display_caps::HdrSource::target_nits`]).
    pub fn hdr_target_nits(&self) -> u16 {
        self.hdr_source.lock().unwrap().target_nits()
    }
    pub fn info(&self) -> serde_json::Value {
        let mut warnings = self.launch.warnings.snapshot();
        warnings.extend(self.capture_warnings.read().unwrap().snapshot());
        // The size and rate the stream runs at: the negotiated ones until the
        // client switches modes (0x5532). The rate is the one encoded, which
        // `limit_framerate` takes from the launch request; the stream
        // request's own rate is `requested_fps`.
        let mode = self.stream_mode();
        let fps = mode.fps_millihz.saturating_add(500) / 1000;
        let (width, height) = (mode.width, mode.height);
        let requested_fps = if mode == crate::reconfigure::Mode::of(&self.config) {
            self.config.fps
        } else {
            fps
        };
        // Sorted after the lock the video sender records into is released.
        let performance = self
            .stats
            .performance
            .lock()
            .unwrap()
            .window(Instant::now());
        serde_json::json!({"warnings":warnings,"encoder":*self.encoder.read().unwrap(),"uuid":self.launch.client.uuid,"device_name":self.launch.client.name,"width":width,"height":height,"fps":fps,"requested_fps":requested_fps,"video_format":self.config.codec,"hdr":self.config.hdr,"vrr":self.config.vrr_low_latency,"encoder_bitrate_kbps":self.bitrate.load(Ordering::Relaxed),"adaptive_bitrate":self.adaptive_bitrate.lock().unwrap().snapshot(),"link_rate":self.link_rate_snapshot(),"packet_pacing":self.automatic_pacing.lock().unwrap().snapshot(),"fec_percent":self.fec_percent(),"pyrowave_minimum_kbps":(self.config.codec == 3).then(|| crate::pyrowave::minimum_kbps(self.config.width, self.config.height, self.config.fps_millihz())),"pyrowave_recommended_kbps":(self.config.codec == 3).then(|| crate::pyrowave::recommended_kbps(self.config.width, self.config.height, self.config.fps_millihz())),"audio_channels":self.config.audio_channels,"state":if self.stopping(){"STOPPING"}else{"RUNNING"},"frames_sent":self.stats.frames.load(Ordering::Relaxed),"frames_replaced":self.stats.frames_replaced.load(Ordering::Relaxed),"packets_sent":self.stats.packets.load(Ordering::Relaxed),"bytes_sent":self.stats.bytes.load(Ordering::Relaxed),"idr_requests":self.stats.idr_requests.load(Ordering::Relaxed),"reference_invalidations":self.stats.reference_invalidations.load(Ordering::Relaxed),"send_loss_recoveries":self.stats.send_loss_recoveries.load(Ordering::Relaxed),"frames_lost_on_send":self.stats.frames_lost_on_send.load(Ordering::Relaxed),"keyframes_past_fec":self.stats.keyframes_past_fec.load(Ordering::Relaxed),"slice_delivery_overflows":self.stats.slice_delivery_overflows.load(Ordering::Relaxed),"encode_latency_ms":self.stats.latency_us.load(Ordering::Relaxed) as f64/1000.,"performance":performance.snapshot(),"pipeline_rates":self.console_pipeline_rates(),"uptime_seconds":self.started.elapsed().as_secs_f64(),"role":self.launch.role})
    }
}
