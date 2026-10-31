//! Launches: a prepared stream waiting for its client's RTSP handshake.
use super::{Role, Warnings};
use crate::state::Client;
use anyhow::{Result, bail};
use std::{
    collections::BTreeMap,
    net::IpAddr,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    time::{Duration, Instant},
};

/// A platform preparation that can be borrowed or moved into its stream.
pub trait Preparation<T>: Send {
    fn prepared(&self) -> &T;
    fn into_prepared(self: Box<Self>) -> T;
}

/// A prepared audio route shared with the app and its stream.
pub trait AudioPreparation<T>: Send + Sync {
    fn route(&self) -> Arc<T>;
}

/// How a session's media reaches the client. GameStream is Moonlight's RTSP, RTP and ENet;
/// WebRTC is the signalling server's session (see `webrtc::session` in the host).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Transport {
    #[default]
    GameStream,
    WebRtc,
}

pub struct Launch<P = (), A = ()> {
    pub warnings: Arc<Warnings>,
    pub id: String,
    pub client: Client,
    pub peer: IpAddr,
    pub app_id: u32,
    /// Fixed before publishing the launch so live configuration edits cannot change isolation.
    pub background: Option<bool>,
    pub key: [u8; 16],
    pub key_id: u32,
    pub ping: String,
    pub connect_data: u32,
    pub role: Role,
    pub created: Instant,
    pub rtsp_encrypted: bool,
    pub rtsp_counter: Arc<AtomicU32>,
    pub rtsp_received: Arc<std::sync::Mutex<crate::packet::ReplayWindow>>,
    /// The platform host owns launch preparation. Keeping it on the launch
    /// gives expired, rejected and disconnected requests the same RAII teardown.
    pub preparation: Arc<std::sync::Mutex<Option<Box<dyn Preparation<P>>>>>,
    pub vrr_requested: bool,
    pub host_audio: bool,
    pub requested_rate: u32,
    pub options: BTreeMap<String, String>,
    pub audio_preparation: Arc<std::sync::Mutex<Option<Box<dyn AudioPreparation<A>>>>>,
    /// While the host still prepares the launch (displays, audio, the app's
    /// own commands, which can take minutes), it does not expire: the client
    /// connects only after the launch reply.
    pub preparing: Arc<AtomicBool>,
    pub transport: Transport,
    /// The DESCRIBE answer offered client roaming (`crate::roaming::SDP_ATTRIBUTE`).
    pub roaming_offered: Arc<AtomicBool>,
    /// Until when an RTSP handshake holds this launch past its 30 s
    /// ([`Sessions::claim`]); `None` until its first RTSP request.
    pub claimed_until: Option<Instant>,
    /// The Nucleus Co-op session and slot this launch streams, fixed when the
    /// host admits it; `None` for every other launch.
    pub nucleus: Option<Seat>,
    /// What the launch holds for the host (a co-op launch's slot), given back
    /// when its last copy is dropped: an expired, replaced or failed launch
    /// at once, a started one with its stream's session. RTSP's snapshots
    /// never keep it.
    pub held: Option<Arc<dyn std::any::Any + Send + Sync>>,
}
/// One player's place in a Nucleus Co-op session (see `crate::nucleus::coop`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Seat {
    pub session: u32,
    pub slot: usize,
}
// Launch clones share the slots; the platform preparations need not be Clone.
impl<P, A> Clone for Launch<P, A> {
    fn clone(&self) -> Self {
        Self {
            warnings: self.warnings.clone(),
            id: self.id.clone(),
            client: self.client.clone(),
            peer: self.peer,
            app_id: self.app_id,
            background: self.background,
            key: self.key,
            key_id: self.key_id,
            ping: self.ping.clone(),
            connect_data: self.connect_data,
            role: self.role,
            created: self.created,
            rtsp_encrypted: self.rtsp_encrypted,
            rtsp_counter: self.rtsp_counter.clone(),
            rtsp_received: self.rtsp_received.clone(),
            preparation: self.preparation.clone(),
            vrr_requested: self.vrr_requested,
            host_audio: self.host_audio,
            requested_rate: self.requested_rate,
            options: self.options.clone(),
            audio_preparation: self.audio_preparation.clone(),
            preparing: self.preparing.clone(),
            transport: self.transport,
            roaming_offered: self.roaming_offered.clone(),
            claimed_until: self.claimed_until,
            nucleus: self.nucleus,
            held: self.held.clone(),
        }
    }
}
impl<P, A> Launch<P, A> {
    /// Whether a 10-bit format negotiated by RTSP is 10-bit SDR. The protocol
    /// has no flag for it, so a client says so with `hdrMode=0` on /launch or
    /// /resume. Without `hdrMode` the format stays HDR, as older and
    /// third-party clients expect.
    pub fn ten_bit_sdr_requested(&self) -> bool {
        self.options.get("hdrMode").is_some_and(|v| v == "0")
    }
    /// RTSP may wait for network input after authorization. Its identity
    /// snapshot must not keep a canceled launch's display, limiter or audio
    /// route alive while a replacement launch prepares new resources.
    pub(super) fn rtsp_snapshot(&self) -> Self {
        let mut snapshot = self.clone();
        snapshot.preparation = Default::default();
        snapshot.audio_preparation = Default::default();
        snapshot.held = None;
        snapshot
    }
    /// Still waiting for its client: being prepared, or able to take its
    /// client's RTSP requests ([`Launch::claimable`]).
    pub(super) fn live(&self) -> bool {
        self.preparing.load(Ordering::Acquire) || self.claimable().is_ok()
    }
    /// Whether RTSP may use this launch now: only once the host has prepared
    /// it and sent the reply (the client connects only then), and only within
    /// [`LAUNCH_WAIT`] of that, or while a claimed handshake's lease runs,
    /// never past [`CLAIM_LIMIT`].
    pub(super) fn claimable(&self) -> Result<()> {
        if self.preparing.load(Ordering::Acquire) {
            bail!("the launch is still being prepared; the device connects after its reply");
        }
        let age = self.created.elapsed();
        let leased = self
            .claimed_until
            .is_some_and(|until| Instant::now() < until && age < CLAIM_LIMIT);
        if age >= LAUNCH_WAIT && !leased {
            bail!(
                "the launch expired: the device did not finish its RTSP handshake within {} s",
                age.as_secs()
            );
        }
        Ok(())
    }
}
/// How long a prepared launch waits for its client's first RTSP request.
pub const LAUNCH_WAIT: Duration = Duration::from_secs(30);
/// How long each RTSP request on a claimed launch keeps it waiting for the next.
pub const RTSP_LEASE: Duration = Duration::from_secs(10);
/// The longest a claimed launch waits for PLAY, counted from its reply.
pub const CLAIM_LIMIT: Duration = Duration::from_secs(60);
impl<P, A> Launch<P, A> {
    /// The lease an RTSP request at `now` gives this launch.
    pub(super) fn lease(&self, now: Instant) -> Instant {
        (now + RTSP_LEASE).min(self.created + CLAIM_LIMIT)
    }
}
