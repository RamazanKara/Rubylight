//! The audio stream's threads: UDP receive with the 100 ms poll timeouts and the 30-packet
//! queue to the renderer's thread (or direct submission on the receive thread). The socket
//! and its pings start during the RTSP handshake, as GFE 3.22 answers PLAY only after an
//! audio ping.
//!
//! Ported from moonlight-common-c's `AudioStream.c` (GPL-3.0, Cameron Gutman and
//! contributors).

use std::collections::VecDeque;
use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;

use super::net;
use super::{now_us, AudioRenderer, Shared};
use crate::audio::{AudioOutput, AudioReceiver, AudioStats};

/// `packetQueue`'s bound; a full queue is emptied before the next packet goes in.
pub const AUDIO_QUEUE_BOUND: usize = 30;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

/// An Opus packet, or `None` for one lost (packet loss concealment).
type Item = Option<Vec<u8>>;

#[derive(Default)]
struct QueueState {
    items: VecDeque<Item>,
    shutdown: bool,
}

#[derive(Default)]
pub struct PacketQueue {
    state: Mutex<QueueState>,
    ready: Condvar,
    pub overflows: AtomicU64,
}

impl PacketQueue {
    /// Queues an item; a full queue is flushed first, as the C does.
    pub fn offer(&self, item: Item) -> bool {
        let mut state = lock(&self.state);
        if state.shutdown {
            return false;
        }
        if state.items.len() >= AUDIO_QUEUE_BOUND {
            state.items.clear();
            self.overflows.fetch_add(1, Ordering::Relaxed);
        }
        state.items.push_back(item);
        self.ready.notify_one();
        true
    }

    fn wait(&self) -> Option<Item> {
        let mut state = lock(&self.state);
        loop {
            if state.shutdown {
                return None;
            }
            if let Some(item) = state.items.pop_front() {
                return Some(item);
            }
            state = self.ready.wait(state).unwrap_or_else(|p| p.into_inner());
        }
    }

    pub fn len(&self) -> usize {
        lock(&self.state).items.len()
    }

    fn shutdown(&self) {
        lock(&self.state).shutdown = true;
        self.ready.notify_all();
    }
}

/// State shared with the session for statistics.
#[derive(Default)]
pub struct AudioShared {
    pub queue: PacketQueue,
    pub stats: Mutex<AudioStats>,
    pub packets: AtomicU64,
}

pub struct AudioStream {
    shared: Arc<AudioShared>,
    stop: Arc<AtomicBool>,
    receive: Option<JoinHandle<Option<Box<dyn AudioRenderer>>>>,
    decode: Option<JoinHandle<Box<dyn AudioRenderer>>>,
}

impl AudioStream {
    pub fn start(
        socket: &UdpSocket,
        receiver: AudioReceiver,
        renderer: Box<dyn AudioRenderer>,
        direct_submit: bool,
        shared: Arc<AudioShared>,
        session: Arc<Shared>,
    ) -> std::io::Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let (direct, queued) = if direct_submit {
            (Some(renderer), None)
        } else {
            (None, Some(renderer))
        };
        let socket = socket.try_clone()?;
        let mut stream = Self {
            shared: shared.clone(),
            stop: stop.clone(),
            receive: None,
            decode: None,
        };
        let receive_shared = shared.clone();
        let decode_session = session.clone();
        stream.receive = Some(std::thread::Builder::new().name("AudioRecv".into()).spawn(
            move || {
                session.prioritize_thread();
                receive_thread(socket, receiver, direct, receive_shared, session, stop)
            },
        )?);
        if let Some(mut renderer) = queued {
            stream.decode = Some(std::thread::Builder::new().name("AudioDec".into()).spawn(
                move || {
                    decode_session.prioritize_thread();
                    while let Some(item) = shared.queue.wait() {
                        renderer.play(item.as_deref());
                    }
                    renderer
                },
            )?);
        }
        Ok(stream)
    }

    /// Stops and joins the threads and returns the renderer for `stop` and `cleanup`.
    pub fn stop(mut self) -> Option<Box<dyn AudioRenderer>> {
        self.stop.store(true, Ordering::Release);
        self.shared.queue.shutdown();
        let mut renderer = self.receive.take().and_then(|t| t.join().ok()).flatten();
        if let Some(thread) = self.decode.take() {
            renderer = thread.join().ok().or(renderer);
        }
        renderer
    }
}

fn receive_thread(
    socket: UdpSocket,
    mut receiver: AudioReceiver,
    mut direct: Option<Box<dyn AudioRenderer>>,
    shared: Arc<AudioShared>,
    session: Arc<Shared>,
    stop: Arc<AtomicBool>,
) -> Option<Box<dyn AudioRenderer>> {
    let mut buffer = vec![0u8; 65536];
    let mut received = false;
    while !stop.load(Ordering::Acquire) {
        let length = match net::receive(&socket, &mut buffer) {
            Ok(Some(length)) => length,
            Ok(None) => {
                receiver.receive_timeout();
                continue;
            }
            Err(e) => {
                session.log(&format!("Audio receive failed: {e}"));
                session.terminate(e.raw_os_error().unwrap_or(-1));
                break;
            }
        };
        if !received && length >= 12 {
            received = true;
            session.log("Received first audio packet");
        }
        receiver.receive(&buffer[..length], now_us(), &mut |output| {
            shared.packets.fetch_add(1, Ordering::Relaxed);
            let item = match output {
                AudioOutput::Packet(opus) => Some(opus),
                AudioOutput::Lost => None,
            };
            match direct.as_mut() {
                Some(renderer) => renderer.play(item),
                None => {
                    shared.queue.offer(item.map(<[u8]>::to_vec));
                }
            }
        });
        *lock(&shared.stats) = receiver.stats();
    }
    direct
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_queue_is_flushed_before_the_next_packet() {
        let queue = PacketQueue::default();
        for i in 0..AUDIO_QUEUE_BOUND {
            assert!(queue.offer(Some(vec![i as u8])));
        }
        assert_eq!(queue.len(), AUDIO_QUEUE_BOUND);
        queue.offer(None);
        assert_eq!(queue.len(), 1);
        assert_eq!(queue.overflows.load(Ordering::Relaxed), 1);
        assert_eq!(queue.wait(), Some(None));
        queue.shutdown();
        assert_eq!(queue.wait(), None);
    }
}
