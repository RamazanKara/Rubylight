//! qWAVE setup and removal may wait for the Windows service. Keep both off
//! the video/audio threads, including when a client changes its UDP port.
use butterpollo_windows::net::ReplySocket;
use std::{
    net::SocketAddr,
    sync::{Arc, Condvar, Mutex},
    thread,
};

#[derive(Clone)]
struct Request {
    socket: Arc<ReplySocket>,
    peer: SocketAddr,
    voice: bool,
}
impl Request {
    fn matches(&self, socket: &Arc<ReplySocket>, peer: SocketAddr, voice: bool) -> bool {
        self.peer == peer && self.voice == voice && Arc::ptr_eq(&self.socket, socket)
    }
}

#[derive(Default)]
struct Pending {
    latest: Option<Request>,
    stopped: bool,
}
#[derive(Default)]
struct Queue {
    pending: Mutex<Pending>,
    wake: Condvar,
}
struct Worker(Arc<Queue>);
impl Worker {
    fn spawn<F: Send + 'static>(
        mut create: impl FnMut(&Request) -> Option<F> + Send + 'static,
    ) -> std::io::Result<Self> {
        let queue = Arc::new(Queue::default());
        let worker = queue.clone();
        thread::Builder::new()
            .name("stream qos".into())
            .spawn(move || {
                // Tuple fields drop in order: remove the flow before closing
                // its socket, even after the media session has already ended.
                let mut active: Option<(F, Arc<ReplySocket>)> = None;
                loop {
                    let request = {
                        let mut pending = worker.pending.lock().unwrap();
                        while pending.latest.is_none() && !pending.stopped {
                            pending = worker.wake.wait(pending).unwrap();
                        }
                        if pending.stopped {
                            break;
                        }
                        pending.latest.take().unwrap()
                    };
                    let flow = create(&request);
                    // Replacing active can block in QosFlow::drop. It must not
                    // hold the queue lock or make follow/teardown wait for it.
                    active = flow.map(|flow| (flow, request.socket));
                }
                drop(active);
            })?;
        Ok(Self(queue))
    }

    fn follow(&self, request: Request) {
        self.0.pending.lock().unwrap().latest = Some(request);
        self.0.wake.notify_one();
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        let mut pending = self.0.pending.lock().unwrap();
        pending.stopped = true;
        pending.latest = None;
        drop(pending);
        self.0.wake.notify_one();
        // A Windows service call cannot be cancelled safely. The worker owns
        // its flow/socket until it returns and performs its own cleanup.
    }
}

/// One worker and at most one pending change per session's media socket.
#[derive(Default)]
pub(super) struct Tagged {
    worker: Option<Worker>,
    requested: Option<Request>,
    unavailable: bool,
}
impl Tagged {
    pub(super) fn follow(&mut self, socket: &Arc<ReplySocket>, peer: SocketAddr, voice: bool) {
        if self.unavailable
            || self
                .requested
                .as_ref()
                .is_some_and(|request| request.matches(socket, peer, voice))
        {
            return;
        }
        if self.worker.is_none() {
            match Worker::spawn(|request| {
                let (peer, voice) = (request.peer, request.voice);
                let class = if voice { "voice" } else { "video" };
                match butterpollo_windows::net::QosFlow::new(&request.socket, peer, voice) {
                    Ok(Some(flow)) => {
                        tracing::info!(%peer, class, "stream traffic tagged for QoS");
                        Some(flow)
                    }
                    Ok(None) => None,
                    Err(error) => {
                        tracing::warn!(%peer, class, error = %format!("{error:#}"), "could not tag stream traffic for QoS");
                        None
                    }
                }
            }) {
                Ok(worker) => self.worker = Some(worker),
                Err(error) => {
                    self.unavailable = true;
                    tracing::warn!(%error, "QoS worker unavailable; streaming without traffic tags");
                    return;
                }
            }
        }
        let request = Request {
            socket: socket.clone(),
            peer,
            voice,
        };
        self.worker.as_ref().unwrap().follow(request.clone());
        self.requested = Some(request);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use std::{sync::mpsc, time::Duration};

    enum Action {
        Follow(Arc<ReplySocket>, SocketAddr, bool),
        Stop,
    }
    fn media(
        mut tagged: Tagged,
    ) -> (
        mpsc::Sender<Action>,
        mpsc::Receiver<()>,
        thread::JoinHandle<()>,
    ) {
        let (send, commands) = mpsc::channel();
        let (done, completed) = mpsc::channel();
        let thread = thread::spawn(move || {
            while let Ok(action) = commands.recv() {
                match action {
                    Action::Follow(socket, peer, voice) => tagged.follow(&socket, peer, voice),
                    Action::Stop => {
                        drop(tagged);
                        let _ = done.send(());
                        return;
                    }
                }
                let _ = done.send(());
            }
        });
        (send, completed, thread)
    }
    fn completed(done: &mpsc::Receiver<()>) {
        done.recv_timeout(Duration::from_secs(2))
            .expect("media operation waited for the blocked QoS worker");
    }

    #[test]
    fn slow_setup_does_not_block_media_or_teardown_and_keeps_the_socket_alive() -> Result<()> {
        struct Flow {
            socket: std::sync::Weak<ReplySocket>,
            removed: mpsc::Sender<bool>,
        }
        impl Drop for Flow {
            fn drop(&mut self) {
                let _ = self.removed.send(self.socket.upgrade().is_some());
            }
        }
        let (started, starting) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        let (removed, removal) = mpsc::channel();
        let worker = Worker::spawn(move |request| {
            let _ = started.send(());
            let _ = blocked.recv();
            Some(Flow {
                socket: Arc::downgrade(&request.socket),
                removed: removed.clone(),
            })
        })?;
        let (send, done, thread) = media(Tagged {
            worker: Some(worker),
            ..Default::default()
        });
        let socket = Arc::new(ReplySocket::bind("127.0.0.1:0")?);
        send.send(Action::Follow(
            socket.clone(),
            "192.0.2.1:1".parse()?,
            false,
        ))?;
        completed(&done);
        starting.recv_timeout(Duration::from_secs(2))?;
        // Setup remains blocked while the media thread follows another peer
        // and ends its session. Neither operation may enter the native call.
        send.send(Action::Follow(socket.clone(), "192.0.2.1:2".parse()?, true))?;
        completed(&done);
        send.send(Action::Stop)?;
        completed(&done);
        thread.join().unwrap();
        drop(socket);
        release.send(())?;
        assert!(removal.recv_timeout(Duration::from_secs(2))?);
        // The obsolete pending peer must not start another native setup.
        assert!(starting.recv_timeout(Duration::from_secs(2)).is_err());
        Ok(())
    }

    #[test]
    fn rebinding_coalesces_pending_peers_while_removal_is_blocked() -> Result<()> {
        struct Flow {
            peer: SocketAddr,
            socket: std::sync::Weak<ReplySocket>,
            removing: mpsc::Sender<SocketAddr>,
            removed: mpsc::Sender<(SocketAddr, bool)>,
            blocked: Option<mpsc::Receiver<()>>,
        }
        impl Drop for Flow {
            fn drop(&mut self) {
                let alive = self.socket.upgrade().is_some();
                let _ = self.removing.send(self.peer);
                if let Some(blocked) = self.blocked.take() {
                    let _ = blocked.recv();
                }
                let _ = self
                    .removed
                    .send((self.peer, alive && self.socket.upgrade().is_some()));
            }
        }
        let first = "192.0.2.1:1".parse()?;
        let second = "192.0.2.1:2".parse()?;
        let last = "192.0.2.1:99".parse()?;
        let (created, creations) = mpsc::channel();
        let (removing, removals) = mpsc::channel();
        let (removed, finished) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        let (finish, finishing) = mpsc::channel();
        let mut block_first_removal = Some(blocked);
        let mut block_final_removal = Some(finishing);
        let worker = Worker::spawn(move |request| {
            let _ = created.send((request.peer, request.voice, thread::current().id()));
            Some(Flow {
                peer: request.peer,
                socket: Arc::downgrade(&request.socket),
                removing: removing.clone(),
                removed: removed.clone(),
                blocked: if request.peer == last {
                    block_final_removal.take()
                } else {
                    block_first_removal.take()
                },
            })
        })?;
        let (send, done, thread) = media(Tagged {
            worker: Some(worker),
            ..Default::default()
        });
        let socket = Arc::new(ReplySocket::bind("127.0.0.1:0")?);
        send.send(Action::Follow(socket.clone(), first, false))?;
        completed(&done);
        let (peer, voice, worker_id) = creations.recv_timeout(Duration::from_secs(2))?;
        assert_eq!((peer, voice), (first, false));
        send.send(Action::Follow(socket.clone(), first, false))?;
        completed(&done);
        send.send(Action::Follow(socket.clone(), second, true))?;
        completed(&done);
        assert_eq!(
            creations.recv_timeout(Duration::from_secs(2))?,
            (second, true, worker_id)
        );
        assert_eq!(removals.recv_timeout(Duration::from_secs(2))?, first);
        // The single worker is now in native removal. Repeated peer updates
        // must remain bounded, with only the newest request retained.
        for port in 3..=99 {
            let peer = SocketAddr::new(first.ip(), port);
            send.send(Action::Follow(socket.clone(), peer, true))?;
            completed(&done);
        }
        release.send(())?;
        assert_eq!(
            creations.recv_timeout(Duration::from_secs(2))?,
            (last, true, worker_id)
        );
        assert_eq!(removals.recv_timeout(Duration::from_secs(2))?, second);
        assert_eq!(
            finished.recv_timeout(Duration::from_secs(2))?,
            (first, true)
        );
        assert_eq!(
            finished.recv_timeout(Duration::from_secs(2))?,
            (second, true)
        );
        send.send(Action::Follow(socket, last, true))?;
        completed(&done);
        send.send(Action::Stop)?;
        completed(&done);
        thread.join().unwrap();
        assert_eq!(removals.recv_timeout(Duration::from_secs(2))?, last);
        // Session teardown returned while its final native removal was still
        // blocked, with the worker retaining the last socket until it finishes.
        finish.send(())?;
        assert_eq!(finished.recv_timeout(Duration::from_secs(2))?, (last, true));
        assert!(creations.recv_timeout(Duration::from_secs(2)).is_err());
        Ok(())
    }
}
