//! Stream DVR's tap on a session's encoded stream: the session thread and the audio thread copy
//! each encoded picture and Opus packet to the host's DVR worker while instant replay runs.
//!
//! Off (the default, and always for a session the host never armed) a tap costs one relaxed
//! atomic load. Armed, a copy goes over a bounded channel with `try_send` and never waits: when
//! the worker is behind, the copy is dropped and counted, and the worker resumes at the next
//! keyframe. The worker asks for a keyframe every few seconds with
//! [`Session::request_dvr_keyframe`] so a clip can start on one.
//!
//! The tap also queues the session's outgoing 0x554A frames (DVR state, clip results), which
//! the control loop sends to the session's device when it wakes.
use super::Session;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

/// Copies the channel holds before the tap drops them: about two seconds at 120 fps with audio
/// packets beside.
pub const CAPACITY: usize = 256;
/// Outgoing frames kept for the control loop; the oldest goes first when it falls behind.
pub const OUTGOING: usize = 32;

type Wake = Arc<dyn Fn() + Send + Sync>;

/// How a tapped picture relates to a frame: whole, or a slice of one (slice delivery).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TapUnit {
    Whole,
    Slice(u32),
    Last(u32),
}

/// One copy for the DVR worker.
#[derive(Debug)]
pub enum TapItem {
    /// An encoded picture (or slice) as the encoder returned it.
    Video {
        bytes: Vec<u8>,
        unit: TapUnit,
        idr: bool,
        /// When the picture was captured, or encoded when the capture time is unknown.
        at: Instant,
        /// The stream was in HDR when the picture was encoded.
        hdr: bool,
    },
    /// One Opus packet.
    Audio {
        bytes: Vec<u8>,
        /// When its samples were captured.
        at: Instant,
        /// The stream's 48 kHz sample position of the packet.
        position: u64,
        /// Interleaved samples the packet encodes (frames times channels).
        interleaved: u32,
    },
    /// The Opus layout the audio thread opened its encoder with.
    OpusLayout {
        channels: u8,
        streams: u8,
        coupled: u8,
        mapping: Vec<u8>,
    },
}

/// The session's tap; see the module documentation.
#[derive(Default)]
pub struct DvrTap {
    armed: AtomicBool,
    sender: OnceLock<SyncSender<TapItem>>,
    drops: AtomicU64,
    outgoing: Mutex<VecDeque<Vec<u8>>>,
    outgoing_pending: AtomicBool,
    wake: Mutex<Option<Wake>>,
    layout: Mutex<Option<OpusLayout>>,
}

/// An Opus layout: channels, streams, coupled streams and the channel mapping.
pub type OpusLayout = (u8, u8, u8, Vec<u8>);

impl DvrTap {
    /// Arms the tap and returns the worker's end of its channel; `None` when it was armed
    /// before (a session arms at most once).
    pub fn arm(&self) -> Option<Receiver<TapItem>> {
        let (sender, receiver) = sync_channel(CAPACITY);
        self.sender.set(sender).ok()?;
        self.armed.store(true, Ordering::Release);
        Some(receiver)
    }

    /// Stops copying; nothing more is sent.
    pub fn disarm(&self) {
        self.armed.store(false, Ordering::Release);
    }

    /// Whether copies go to the worker. The only cost of a tap that is off.
    #[inline]
    pub fn is_armed(&self) -> bool {
        self.armed.load(Ordering::Relaxed)
    }

    /// Copies dropped because the worker was behind.
    pub fn drops(&self) -> u64 {
        self.drops.load(Ordering::Relaxed)
    }

    /// Sends `item` without waiting; a full channel drops it and counts the drop, a closed one
    /// (the worker ended) disarms the tap.
    pub fn send(&self, item: TapItem) {
        let Some(sender) = self.sender.get() else {
            return;
        };
        match sender.try_send(item) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                self.drops.fetch_add(1, Ordering::Relaxed);
            }
            Err(TrySendError::Disconnected(_)) => self.disarm(),
        }
    }

    /// Copies one Opus packet while armed.
    #[inline]
    pub fn audio(&self, packet: &[u8], at: Instant, position: u64, interleaved: usize) {
        if self.is_armed() {
            self.send(TapItem::Audio {
                bytes: packet.to_vec(),
                at,
                position,
                interleaved: interleaved as u32,
            });
        }
    }

    /// Keeps the Opus layout the audio thread opened its encoder with, and tells the worker
    /// while armed. Called when the encoder opens, never per packet.
    pub fn opus_layout(&self, channels: usize, streams: i32, coupled: i32, mapping: &[u8]) {
        let layout = (
            channels as u8,
            streams as u8,
            coupled as u8,
            mapping.to_vec(),
        );
        *self.layout.lock().unwrap() = Some(layout.clone());
        if self.is_armed() {
            let (channels, streams, coupled, mapping) = layout;
            self.send(TapItem::OpusLayout {
                channels,
                streams,
                coupled,
                mapping,
            });
        }
    }

    /// The Opus layout last opened, for a worker that armed after the audio thread started.
    pub fn last_opus_layout(&self) -> Option<OpusLayout> {
        self.layout.lock().unwrap().clone()
    }
}

impl DvrTap {
    /// Queues a 0x554A frame for the session's device and wakes the control loop.
    pub fn post(&self, frame: Vec<u8>) {
        {
            let mut outgoing = self.outgoing.lock().unwrap();
            if outgoing.len() >= OUTGOING {
                outgoing.pop_front();
            }
            outgoing.push_back(frame);
            self.outgoing_pending.store(true, Ordering::Release);
        }
        let wake = self.wake.lock().unwrap().clone();
        if let Some(wake) = wake {
            wake();
        }
    }

    /// How the control loop is woken when a frame is queued.
    pub fn set_wake(&self, wake: impl Fn() + Send + Sync + 'static) {
        *self.wake.lock().unwrap() = Some(Arc::new(wake));
    }

    /// Whether frames wait; one relaxed load.
    #[inline]
    pub fn has_outgoing(&self) -> bool {
        self.outgoing_pending.load(Ordering::Relaxed)
    }

    /// The queued frames, oldest first.
    pub fn take_outgoing(&self) -> Vec<Vec<u8>> {
        let mut outgoing = self.outgoing.lock().unwrap();
        self.outgoing_pending.store(false, Ordering::Release);
        outgoing.drain(..).collect()
    }
}

impl<P, A> Session<P, A> {
    /// A keyframe Stream DVR asks for while armed, so a clip can start on one. It is the host's
    /// own keyframe: never counted as a client request or as loss. Intra-only streams need none.
    pub fn request_dvr_keyframe(&self) {
        if self.config.codec != 3 {
            self.keyframe_loss.lock().unwrap().host_keyframe();
            self.idr.store(true, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unarmed_tap_sends_nothing() {
        let tap = DvrTap::default();
        assert!(!tap.is_armed());
        tap.audio(&[1, 2, 3], Instant::now(), 0, 960);
        assert_eq!(tap.drops(), 0);
    }

    #[test]
    fn a_full_channel_drops_and_counts_without_waiting() {
        let tap = DvrTap::default();
        let receiver = tap.arm().unwrap();
        assert!(tap.arm().is_none(), "a session arms once");
        for position in 0..(CAPACITY as u64 + 5) {
            tap.audio(&[0xFC], Instant::now(), position * 960, 1920);
        }
        assert_eq!(tap.drops(), 5);
        assert_eq!(receiver.try_iter().count(), CAPACITY);
    }

    #[test]
    fn a_closed_worker_disarms_the_tap() {
        let tap = DvrTap::default();
        drop(tap.arm().unwrap());
        tap.audio(&[0xFC], Instant::now(), 0, 1920);
        assert!(!tap.is_armed());
        tap.disarm();
        tap.opus_layout(2, 1, 1, &[0, 1]);
        assert_eq!(tap.drops(), 0);
    }

    #[test]
    fn outgoing_frames_wake_the_loop_and_keep_the_newest() {
        let tap = DvrTap::default();
        let woken = Arc::new(AtomicU64::new(0));
        let count = woken.clone();
        tap.set_wake(move || {
            count.fetch_add(1, Ordering::Relaxed);
        });
        assert!(!tap.has_outgoing());
        for n in 0..(OUTGOING as u8 + 2) {
            tap.post(vec![n]);
        }
        assert!(tap.has_outgoing());
        let frames = tap.take_outgoing();
        assert_eq!(frames.len(), OUTGOING);
        assert_eq!(frames[0], vec![2]);
        assert!(!tap.has_outgoing());
        assert_eq!(woken.load(Ordering::Relaxed), OUTGOING as u64 + 2);
    }
}
