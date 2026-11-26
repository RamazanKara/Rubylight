//! Stream sessions: launches, live sessions and their statistics, and the
//! user-facing warnings a session raises.
use butterpollo_media_policy::rtsp::Negotiated;
use serde::Serialize;
use std::{
    net::IpAddr,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

mod congestion;
pub mod dvr_tap;
mod fec;
mod frame_warp;
mod hdr;
mod launch;
mod link;
mod pacing;
mod registry;
mod stats;
mod warnings;
pub use frame_warp::{FrameWarpState, Outgoing as FrameWarpOutgoing, Params as FrameWarpParams};
pub use launch::{
    AudioPreparation, CLAIM_LIMIT, LAUNCH_WAIT, Launch, Preparation, RTSP_LEASE, Seat, Transport,
};
pub use link::LINK_WARNING;
pub use registry::Sessions;
pub use stats::Stats;
pub use warnings::{EVENT_PERIOD, Warning, Warnings};

#[derive(Clone, Copy, Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Stream,
    RemoteMonitor,
    InputOnly,
}

impl From<Role> for butterpollo_input_policy::controller_audio::AudioRole {
    fn from(role: Role) -> Self {
        match role {
            Role::Stream => Self::Stream,
            Role::RemoteMonitor => Self::RemoteMonitor,
            Role::InputOnly => Self::InputOnly,
        }
    }
}

/// The longest failure text a client is sent, in bytes.
pub const FAILURE_TEXT_MAX: usize = 1024;

/// `reason` as a client can be sent it in `/serverinfo` XML: control
/// characters (from driver or FFI strings) other than tab and line breaks
/// dropped, at most [`FAILURE_TEXT_MAX`] bytes, cut at a character boundary.
pub fn failure_text(reason: &str) -> String {
    let mut text = String::new();
    for c in reason
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\t' | '\r' | '\n'))
    {
        if text.len() + c.len_utf8() > FAILURE_TEXT_MAX {
            break;
        }
        text.push(c);
    }
    text
}

/// An error that ended the stream because the encoder failed: as the error itself
/// or as context on the cause, it reads as its message, and
/// [`Session::fail_with`] sends the client the encoder's termination code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncoderFailure(pub &'static str);

impl std::fmt::Display for EncoderFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for EncoderFailure {}

#[cfg(test)]
mod tests;
/// The encoder's share of a video `budget` that also carries `fec` percent
/// parity: budget × 100 / (100 + fec), at least 1 Kbps of a non-zero budget.
/// Points a statistical FEC share must fall before the encoder's split
/// follows it between fences.
const FEC_SPLIT_HYSTERESIS: u32 = 5;
/// The host's FEC share inside the bitrate a stream launched with: what
/// `stream_policy::apply` took off the bitrate the client's user set.
fn launch_fec_share(stream: &Negotiated) -> u32 {
    if stream.configured_bitrate_kbps == 0 || stream.codec == 3 {
        0
    } else {
        u32::from(stream.launch_fec.min(80))
    }
}
/// The latched split while host bitrate adaptation budgets FEC itself: the
/// encoder gets the bitrate as it is.
const FEC_SPLIT_NONE: u32 = u32::MAX;
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
    /// Set with `failed` when the error was the encoder's ([`EncoderFailure`]).
    pub encoder_failed: AtomicBool,
    /// The host's words for the failure, from the first `fail`: what the
    /// client reads as `RustHostLastFailure`.
    failure: std::sync::Mutex<Option<String>>,
    /// Set by the stream worker once the capture's first frame passed the checks a WebRTC HDR
    /// stream needs (the display is in HDR). The transport holds back its "streaming" status
    /// until then, so a refused stream is never announced first.
    pub source_checked: AtomicBool,
    pub idr: AtomicBool,
    pub invalidation: std::sync::Mutex<Option<(u64, u64)>>,
    pub recovery_wake: std::sync::Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    pub bitrate: AtomicU32,
    /// All bitrate writers take this lock, including client ownership handoff.
    pub adaptive_bitrate:
        std::sync::Mutex<butterpollo_media_policy::adaptive_bitrate::AdaptiveBitrate>,
    /// Lowers the encoder bitrate when the link cannot carry the stream; see
    /// [`butterpollo_media_policy::link_rate`]. Off for a client that sets the bitrate.
    pub link_rate: std::sync::Mutex<butterpollo_media_policy::link_control::LinkControl>,
    /// What the video sender sent, for `link_rate`; one store per frame.
    pub link_sent: butterpollo_media_policy::link_control::SentLog,
    /// Packet-level congestion control from the client's arrival feedback (0x5547); see
    /// [`butterpollo_media_policy::congestion`]. Take after `link_rate`.
    pub congestion: std::sync::Mutex<butterpollo_media_policy::congestion::Control>,
    /// What the video sender sent, by packet id, for `congestion`: one store per packet, and
    /// only once the client's first report came.
    pub transport_sent: butterpollo_media_policy::congestion::SendLedger,
    /// The pace `congestion` set, bps; 0 for none.
    congestion_pacing_bps: AtomicU64,
    /// The route policy paced the last frame faster than the Wi-Fi rate, so
    /// a latch that slows it may be the whole fix (see `LinkRate::on_latch`).
    pace_was_fast: AtomicBool,
    pub adaptive_video_pacing_bps: AtomicU64,
    pub automatic_pacing:
        std::sync::Mutex<butterpollo_media_policy::adaptive_pacing::AdaptivePacing>,
    /// Whether `automatic_pacing` is enabled, set with it at stream start.
    automatic_pacing_on: AtomicBool,
    /// Slower pacing once the client's link has dropped a keyframe.
    pub keyframe_loss: std::sync::Mutex<butterpollo_media_policy::keyframe_loss::KeyframeLoss>,
    /// FEC percentage for the next video frame; the video sender reads it per
    /// frame, and `adaptive_fec` moves it from the client's 0x5502 reports.
    pub fec_percent: AtomicU32,
    pub adaptive_fec: std::sync::Mutex<butterpollo_media_policy::adaptive_fec::AdaptiveFec>,
    /// Statistical FEC from the client's 0x5539 reports, for a client that
    /// launched with `rlFecPercentage=auto`; `adaptive_fec` is off then.
    pub client_fec:
        std::sync::Mutex<Option<butterpollo_media_policy::statistical_fec::StatisticalFec>>,
    /// Whether `client_fec` is set: frames skip its lock otherwise.
    client_fec_on: AtomicBool,
    /// Bumped whenever `client_fec` changes so whole-payload sizing caches outside the lock.
    fec_revision: AtomicU64,
    last_fec_report_at: std::sync::Mutex<Option<Instant>>,
    whole_payload_cache: std::sync::Mutex<Option<fec::WholePayloadCache>>,
    /// Whether the encoder's bitrate leaves room for the client-chosen FEC
    /// share in `fec_split`, latched at the last fence; see [`Self::encoder_bitrate`].
    fec_split_on: AtomicBool,
    fec_split: AtomicU32,
    /// The host's FEC share that is inside `bitrate`, which the split puts
    /// back before it takes the client's: the launch's
    /// ([`launch_fec_share`]) from the start and after a bitrate in the
    /// launch's unit, 0 after one that is the encoder's own. Written with
    /// `bitrate` and read with it ([`Self::encoder_bitrate`]).
    fec_split_launch: AtomicU32,
    /// The bitrate from before a cut by the link-rate or congestion
    /// controller, 0 while none is in force. They size a cut as the encoder's
    /// own rate, the FEC share in use counted as parity
    /// (`WireBudget::encoder_kbps`): the share comes off this bitrate, not off
    /// the cut.
    fec_split_full: AtomicU32,
    /// Capture timing locked to the client's display, from its 0x5530 reports.
    pub phase_sync: std::sync::Mutex<butterpollo_media_policy::phase_sync::PhaseSync>,
    /// The stream's current mode and the client's 0x5532 requests to change it.
    pub reconfigure: std::sync::Mutex<butterpollo_media_policy::reconfigure::Reconfiguration>,
    /// Bitrate and rate changes for a client that launched with `rlRateChange=1` (0x553C):
    /// their switches and answers, sent by the control loop. Take after `reconfigure`.
    pub rate_changes: butterpollo_media_policy::rate_change::RateChanges,
    pub stats: Stats,
    pub started: Instant,
    pub output: std::sync::RwLock<String>,
    /// Whether the stream is currently sending HDR; starts at `config.hdr`,
    /// drops to `false` if a GameStream display leaves HDR, and returns to
    /// `true` once display HDR becomes active again so the encoder, `0x010e`
    /// HDR control packet, and status JSON stay consistent.
    pub hdr_active: AtomicBool,
    /// The display's HDR metadata, once the encoder has it. Clients are told
    /// the HDR state only then, so they hear it once with the real values.
    pub hdr_metadata: std::sync::RwLock<Option<butterpollo_media_policy::hdr::Metadata>>,
    /// Bumped after every `hdr_metadata` write, with release: the control
    /// loop reads it to skip the lock while the metadata stands. Starts above
    /// a new peer's zero, so its first pass reads the metadata.
    pub hdr_revision: AtomicU64,
    /// Source HDR metadata and diagnostic client display luminance (0x5531).
    pub hdr_source: std::sync::Mutex<butterpollo_media_policy::display_caps::HdrSource>,
    /// Per-picture present-to-sent reports for a client that asked (0x5536).
    pub host_timings: butterpollo_media_policy::host_timing::HostTimings,
    /// Tells a client that asked (0x5538) when the host holds the bitrate down for the
    /// network; fed by `tick_link_rate`, sent by the control loop.
    pub link_notices: butterpollo_media_policy::link_notice::LinkNotices,
    /// Pipeline frame rates for a client that asked (0x553D), sent by the control loop.
    pub pipeline_rates: butterpollo_media_policy::pipeline_rates::Reporter,
    /// Keyboard follows focus for a client that asked (0x553B): fed by the focus watcher,
    /// sent by the control loop.
    pub text_input: butterpollo_input_policy::text_input::TextInputs,
    /// The layout id the device's newest keyboard layout report (0x553E) resolved to, on
    /// arrival; None when it named none the host knows, or sent none.
    pub keyboard_layout: std::sync::Mutex<Option<u32>>,
    /// The cursor a client that asked (0x5540) draws itself: noted by the video stream,
    /// which then leaves the pointer out, and sent by the control loop.
    pub client_cursor: butterpollo_input_policy::client_cursor::ClientCursor,
    /// Frame Warp for a client that asked (0x5543): fed by the control thread's input,
    /// the video thread's captures and the packetizer's frame numbers, sent by the control
    /// loop.
    pub frame_warp: FrameWarpState,
    /// Zero of the video stream's RTP clock, set when its sender starts; clock sync
    /// replies (0x5537) are on this clock.
    pub media_epoch: std::sync::RwLock<Option<Instant>>,
    /// The host-side timing record (`BUTTERPOLLO_TIMING_RECORD`), when it is on.
    pub timing_record:
        std::sync::RwLock<Option<Arc<butterpollo_media_policy::timing_record::Sink>>>,
    /// The requested `encoder_focus` ([`butterpollo_media_policy::encoder_focus::Requested::wire`]). The stream
    /// stores the configured value at start; the encoder reads it per frame.
    pub encoder_focus: AtomicU8,
    /// What the encoder runs, written by the encoder thread on change.
    pub encoder_focus_state: std::sync::Mutex<butterpollo_media_policy::encoder_focus::FocusState>,
    /// Where the client is after a roam (`crate::roaming`); see [`Self::roamed`].
    roam: std::sync::Mutex<Roam>,
    /// Roams so far, for senders that keep state about the client's path.
    roams: AtomicU32,
    /// Night mode and headphone surround: the live choice and why either
    /// doesn't apply. Written through `audio_dsp::{state, apply}`.
    pub audio: butterpollo_media_policy::audio_dsp::Controls,
    /// The sent audio's copy for haptics from game audio, filled only while a
    /// controller wants it (`audio_haptics`). Shared so a control peer that
    /// wanted it can stop the copy when it goes.
    pub audio_haptics: Arc<butterpollo_input_policy::audio_haptics::Tap>,
    /// The host-drawn couch menu: open flag, revision and panel, shared by the
    /// control loop, the menu worker, the video loop and the native API.
    pub couch_menu: Arc<butterpollo_input_policy::couch_menu::Shared>,
    /// Stream DVR's copy of the encoded stream; off unless the host armed it for this stream.
    pub dvr: dvr_tap::DvrTap,
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
    pub fn set_encoder_focus(&self, requested: butterpollo_media_policy::encoder_focus::Requested) {
        self.encoder_focus
            .store(requested.wire(), std::sync::atomic::Ordering::Relaxed);
    }
    pub fn new(launch: Launch<P, A>, config: Negotiated) -> Arc<Self> {
        let launch_peer = launch.peer.to_canonical();
        let bitrate = config.bitrate_kbps;
        let launch_share = launch_fec_share(&config);
        let started = Instant::now();
        let picture = (
            u16::try_from(config.width).unwrap_or(u16::MAX),
            u16::try_from(config.height).unwrap_or(u16::MAX),
        );
        let reconfigure = butterpollo_media_policy::reconfigure::Reconfiguration::of(&config);
        let hdr = config.hdr;
        Arc::new(Self {
            encoder: Default::default(),
            capture_warnings: Default::default(),
            launch,
            config,
            stop: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            encoder_failed: AtomicBool::new(false),
            failure: std::sync::Mutex::new(None),
            source_checked: AtomicBool::new(false),
            idr: AtomicBool::new(true),
            invalidation: Default::default(),
            recovery_wake: Default::default(),
            bitrate: AtomicU32::new(bitrate),
            adaptive_bitrate: Default::default(),
            link_rate: Default::default(),
            link_sent: butterpollo_media_policy::link_control::SentLog::new(started),
            congestion: Default::default(),
            transport_sent: butterpollo_media_policy::congestion::SendLedger::new(started),
            congestion_pacing_bps: AtomicU64::new(0),
            pace_was_fast: AtomicBool::new(false),
            adaptive_video_pacing_bps: AtomicU64::new(0),
            automatic_pacing: Default::default(),
            automatic_pacing_on: AtomicBool::new(false),
            keyframe_loss: Default::default(),
            fec_percent: AtomicU32::new(
                butterpollo_media_policy::adaptive_fec::AdaptiveFec::default().percent() as u32,
            ),
            adaptive_fec: Default::default(),
            client_fec: Default::default(),
            client_fec_on: AtomicBool::new(false),
            fec_revision: AtomicU64::new(1),
            last_fec_report_at: Default::default(),
            whole_payload_cache: Default::default(),
            fec_split_on: AtomicBool::new(false),
            fec_split: AtomicU32::new(0),
            fec_split_launch: AtomicU32::new(launch_share),
            fec_split_full: AtomicU32::new(0),
            phase_sync: Default::default(),
            reconfigure: std::sync::Mutex::new(reconfigure),
            rate_changes: Default::default(),
            stats: Stats::default(),
            started,
            output: std::sync::RwLock::new(String::new()),
            hdr_active: AtomicBool::new(hdr),
            hdr_metadata: Default::default(),
            hdr_revision: AtomicU64::new(1),
            hdr_source: Default::default(),
            host_timings: Default::default(),
            link_notices: Default::default(),
            pipeline_rates: Default::default(),
            text_input: Default::default(),
            keyboard_layout: Default::default(),
            client_cursor: Default::default(),
            frame_warp: FrameWarpState::new(picture, started),
            media_epoch: Default::default(),
            timing_record: Default::default(),
            encoder_focus: AtomicU8::new(
                butterpollo_media_policy::encoder_focus::Requested::Off.wire(),
            ),
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
            couch_menu: Arc::new(butterpollo_input_policy::couch_menu::Shared::refusing(
                "couch menu: this stream's control connection has not started",
            )),
            dvr: Default::default(),
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
            *loss = butterpollo_media_policy::keyframe_loss::KeyframeLoss::new(loss.enabled());
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
    /// client asks for others (0x5532), coded no larger than the host's cap.
    pub fn stream_mode(&self) -> butterpollo_media_policy::reconfigure::Mode {
        self.reconfigure.lock().unwrap().applied()
    }
    /// The size and rate the client asked for, which its display keeps; the
    /// coded picture is [`Self::stream_mode`] under the host's cap.
    pub fn requested_mode(&self) -> butterpollo_media_policy::reconfigure::Mode {
        self.reconfigure.lock().unwrap().current()
    }
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Release)
    }
    /// Fails the stream with the host's reason; the first reason stays, as
    /// later ones are usually its consequences.
    pub fn fail(&self, reason: impl Into<String>) {
        self.failure
            .lock()
            .unwrap()
            .get_or_insert_with(|| failure_text(&reason.into()));
        self.failed.store(true, Ordering::Release);
        self.stop();
    }
    /// Fails the stream on `error`; an [`EncoderFailure`] anywhere in its
    /// chain tells the client the encoder failed.
    pub fn fail_with(&self, error: &anyhow::Error) {
        if error.downcast_ref::<EncoderFailure>().is_some() {
            self.encoder_failed.store(true, Ordering::Release);
        }
        // The outer two contexts say what failed; deeper ones carry paths and
        // driver detail the host log keeps.
        let text: Vec<String> = error.chain().take(2).map(ToString::to_string).collect();
        self.fail(text.join(": "));
    }
    /// The reason of the first `fail`, if the stream failed.
    pub fn failure(&self) -> Option<String> {
        self.failure.lock().unwrap().clone()
    }
    pub fn failed(&self) -> bool {
        self.failed.load(Ordering::Acquire)
    }
    /// The termination reason the control stream sends. Moonlight closes
    /// quietly on 0x80030023 (the host closed the stream) and shows an error
    /// with the code otherwise: 0x800E9403 is moonlight-common-c's
    /// `ML_ERROR_FRAME_CONVERSION`, for which stock Moonlight suggests
    /// encoder settings; 0x80004005 is the generic failure HRESULT.
    pub fn termination_reason(&self) -> u32 {
        if !self.failed() {
            0x8003_0023
        } else if self.encoder_failed.load(Ordering::Acquire) {
            0x800E_9403
        } else {
            0x8000_4005
        }
    }
    /// The encoder's bitrate for the next frame. With a client-chosen FEC
    /// share F, video and parity together fit the video budget: the encoder
    /// gets budget × 100 / (100 + F). F moves at a `fence`, an IDR or a new
    /// encoder, and under `rlFecPercentage=auto` also after a report (see
    /// [`Self::follow_fec_report`]): the encoder takes the new rate on the next
    /// frame, without a keyframe. Host bitrate adaptation already budgets FEC:
    /// while it runs, the encoder gets `bitrate` as it is. A cut by the
    /// link-rate or congestion controller is the encoder's own rate as well,
    /// its parity already counted: the encoder gets the cut, and never more
    /// than its share of the bitrate from before it (`fec_split_full`). Split
    /// again, a cut sized for 50 % parity left a sixth of what the link
    /// carries unused.
    ///
    /// The budget is `bitrate` with the host's own FEC share put back, where
    /// the launch took one off the bitrate the client's user set
    /// (`fec_split_launch`): the share comes off once. Taken off twice, a
    /// client that chose the host's own 20 % got 12490 Kbps of a 20 Mbps
    /// setting where the host's FEC gives 14988. The encoder never gets more
    /// than `bitrate`, which pacing and the bitrate controllers take as its
    /// most: a client share below the host's leaves the rest unused, as the
    /// host's own adaptive FEC does.
    ///
    /// The split follows Foundation Sunshine's `encoder_bitrate_from_total_bitrate`
    /// in `src/video.cpp` (AlkaidLab/foundation-sunshine 880f1ef, GPL-3).
    pub fn encoder_bitrate(&self, fence: bool) -> u32 {
        if !self.fec_split_on.load(Ordering::Acquire) {
            return self.bitrate.load(Ordering::Acquire);
        }
        if fence {
            self.fec_split
                .store(self.fec_split_share(), Ordering::Release);
        }
        // A client's bitrate writes both (`take_client_bitrate`). Read again when the share
        // moved meanwhile: a frame never pairs one bitrate with the other's share.
        let (bitrate, launch) = loop {
            let launch = self.fec_split_launch.load(Ordering::Acquire);
            let bitrate = self.bitrate.load(Ordering::Acquire);
            if self.fec_split_launch.load(Ordering::Acquire) == launch {
                break (bitrate, u64::from(launch.min(80)));
            }
        };
        let share = self.fec_split.load(Ordering::Acquire);
        if share == FEC_SPLIT_NONE {
            return bitrate;
        }
        let full = match self.fec_split_full.load(Ordering::Acquire) {
            0 => bitrate,
            full => full,
        };
        let budget = (u64::from(full) * 100 / (100 - launch)).min(u64::from(u32::MAX));
        fec_split_kbps(budget as u32, share).min(full).min(bitrate)
    }
    /// Writes the bitrate the link-rate or congestion controller wants: their cut, with
    /// the bitrate from before it as `full`, or that bitrate again (`fec_split_full`). The
    /// encoder reads the two without the controllers' lock, so they are written in the
    /// order that gives a frame in between too little, never a bitrate without its split.
    fn store_limited(&self, current: u32, applied: u32, full: Option<u32>) {
        if full.is_none() {
            self.fec_split_full.store(0, Ordering::Release);
        }
        if applied != current {
            self.bitrate.store(applied, Ordering::Release);
        }
        if let Some(full) = full {
            self.fec_split_full.store(full, Ordering::Release);
        }
    }
    /// The FEC share the encoder's budget leaves room for now, or
    /// [`FEC_SPLIT_NONE`].
    fn fec_split_share(&self) -> u32 {
        if self.adaptive_bitrate.lock().unwrap().active() {
            FEC_SPLIT_NONE
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
        if first
            || share == FEC_SPLIT_NONE
            || latched == FEC_SPLIT_NONE
            || share > latched
            || latched - share > FEC_SPLIT_HYSTERESIS
        {
            self.fec_split.store(share, Ordering::Release);
        }
    }
}
