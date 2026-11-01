//! The route to the client, looked up on a thread of its own. The lookup
//! (GetBestInterfaceEx and GetIfEntry2Ex, more on a virtual adapter) took
//! the encode thread before an AMF submit and the paced sender between
//! frames; they now read the watcher's latest answer.
use butterpollo_windows::net::Link;
use std::{
    net::{IpAddr, SocketAddr},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

/// How often the route is looked up again, as the stream threads did.
const INTERVAL: Duration = Duration::from_secs(2);

type Query = dyn Fn(SocketAddr) -> Link + Send + Sync;

/// One per stream, shared by its encode loop and video sender.
pub(crate) struct LinkWatch {
    inner: Arc<Inner>,
    watcher: Mutex<Option<thread::JoinHandle<()>>>,
}

struct Inner {
    state: Mutex<State>,
    changed: Condvar,
    query: Box<Query>,
    interval: Duration,
    /// The longest lookup since the timings log last read it, in microseconds.
    slowest: AtomicU64,
}

#[derive(Default)]
struct State {
    /// The client the stream sends to now.
    peer: Option<SocketAddr>,
    /// The latest lookup, for the address it routed and when it was made.
    link: Option<(IpAddr, Link, Instant)>,
    watching: bool,
    /// The watcher thread could not start; told once.
    spawn_failed: bool,
    stopped: bool,
}

impl State {
    /// A lookup made for `peer`; one for an address the stream has left is
    /// stale and dropped.
    fn publish(&mut self, peer: SocketAddr, link: Link, at: Instant) -> bool {
        if self.peer.map(|current| current.ip()) != Some(peer.ip()) {
            return false;
        }
        self.link = Some((peer.ip(), link, at));
        true
    }

    /// The published route to `peer`, while it is still current: the
    /// watcher keeps it so, or without one, the interval bounds its age.
    fn current(&self, peer: SocketAddr, interval: Duration, now: Instant) -> Option<Link> {
        let (routed, link, at) = self.link?;
        (routed == peer.ip() && (self.watching || now.saturating_duration_since(at) < interval))
            .then_some(link)
    }
}

impl Default for LinkWatch {
    fn default() -> Self {
        Self::new(butterpollo_windows::net::routed_link, INTERVAL)
    }
}

impl LinkWatch {
    pub(crate) fn new(
        query: impl Fn(SocketAddr) -> Link + Send + Sync + 'static,
        interval: Duration,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                state: Mutex::default(),
                changed: Condvar::new(),
                query: Box::new(query),
                interval,
                slowest: AtomicU64::new(0),
            }),
            watcher: Mutex::new(None),
        }
    }

    /// The route to `peer`. A new client address is looked up here, once:
    /// its first frame needs the route, as before. The watcher follows it
    /// from then on.
    pub(crate) fn get(&self, peer: SocketAddr) -> Link {
        {
            let mut state = self.inner.state.lock().unwrap();
            if let Some(link) = state.current(peer, self.inner.interval, Instant::now()) {
                state.peer = Some(peer);
                return link;
            }
        }
        let started = Instant::now();
        let link = (self.inner.query)(peer);
        self.inner.note(started);
        let mut state = self.inner.state.lock().unwrap();
        state.peer = Some(peer);
        state.publish(peer, link, Instant::now());
        if !state.watching && !state.spawn_failed && !state.stopped {
            let inner = self.inner.clone();
            match thread::Builder::new()
                .name("link-watch".into())
                .spawn(move || inner.watch())
            {
                Ok(handle) => {
                    state.watching = true;
                    *self.watcher.lock().unwrap() = Some(handle);
                }
                Err(error) => {
                    // Without the thread, this lookup runs here every interval.
                    state.spawn_failed = true;
                    tracing::warn!(%error, "the route watcher did not start; the stream threads look the route up themselves");
                }
            }
        }
        link
    }

    /// The longest lookup since the last call, for the timings log.
    pub(crate) fn take_slowest(&self) -> Duration {
        Duration::from_micros(self.inner.slowest.swap(0, Ordering::Relaxed))
    }
}

impl Inner {
    fn note(&self, started: Instant) {
        let micros = started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64;
        self.slowest.fetch_max(micros, Ordering::Relaxed);
    }

    fn watch(&self) {
        let mut state = self.state.lock().unwrap();
        loop {
            state = self
                .changed
                .wait_timeout_while(state, self.interval, |state| !state.stopped)
                .unwrap()
                .0;
            if state.stopped {
                return;
            }
            let Some(peer) = state.peer else { continue };
            drop(state);
            let started = Instant::now();
            let link = (self.query)(peer);
            self.note(started);
            state = self.state.lock().unwrap();
            state.publish(peer, link, Instant::now());
        }
    }
}

impl Drop for LinkWatch {
    fn drop(&mut self) {
        self.inner.state.lock().unwrap().stopped = true;
        self.inner.changed.notify_all();
        if let Some(handle) = self.watcher.get_mut().unwrap().take() {
            let _ = handle.join();
        }
    }
}

/// The time a frame's remaining checks took on the encode thread.
#[derive(Default)]
pub(crate) struct CheckTimes {
    total: Duration,
    max: Duration,
    frames: u32,
}

impl CheckTimes {
    pub(crate) fn record(&mut self, took: Duration) {
        self.total += took;
        self.max = self.max.max(took);
        self.frames += 1;
    }

    /// Mean/max ms since the last call.
    pub(crate) fn take(&mut self) -> String {
        let taken = std::mem::take(self);
        let ms = |d: Duration| d.as_secs_f64() * 1000.;
        format!(
            "{:.3}/{:.3}",
            ms(taken.total) / f64::from(taken.frames.max(1)),
            ms(taken.max)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn check_times_report_mean_and_max_once() {
        let mut checks = CheckTimes::default();
        assert_eq!(checks.take(), "0.000/0.000");
        checks.record(Duration::from_micros(100));
        checks.record(Duration::from_micros(300));
        assert_eq!(checks.take(), "0.200/0.300");
        assert_eq!(checks.take(), "0.000/0.000");
    }

    struct Fake {
        lookups: AtomicUsize,
        bps: AtomicU64,
    }

    fn watch(interval: Duration) -> (LinkWatch, Arc<Fake>) {
        let fake = Arc::new(Fake {
            lookups: AtomicUsize::new(0),
            bps: AtomicU64::new(1_000_000_000),
        });
        let query = fake.clone();
        let watch = LinkWatch::new(
            move |peer: SocketAddr| {
                query.lookups.fetch_add(1, Ordering::SeqCst);
                Link {
                    bps: query.bps.load(Ordering::SeqCst),
                    wireless: false,
                    interface: u64::from(peer.port()),
                }
            },
            interval,
        );
        (watch, fake)
    }

    fn peer(address: &str) -> SocketAddr {
        address.parse().unwrap()
    }

    fn eventually(mut done: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if done() {
                return true;
            }
            thread::sleep(Duration::from_millis(5));
        }
        false
    }

    #[test]
    fn hot_path_reads_the_published_route_without_a_lookup() {
        let (watch, fake) = watch(Duration::from_secs(3600));
        let client = peer("192.168.4.20:47998");
        assert_eq!(watch.get(client).bps, 1_000_000_000);
        assert_eq!(fake.lookups.load(Ordering::SeqCst), 1);
        fake.bps.store(100_000_000, Ordering::SeqCst);
        for _ in 0..1000 {
            assert_eq!(watch.get(client).bps, 1_000_000_000);
        }
        // Another port on the same address takes the same route.
        watch.get(peer("192.168.4.20:50000"));
        assert_eq!(fake.lookups.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn the_watcher_publishes_a_changed_link() {
        let (watch, fake) = watch(Duration::from_millis(10));
        let client = peer("192.168.4.20:47998");
        assert_eq!(watch.get(client).bps, 1_000_000_000);
        fake.bps.store(100_000_000, Ordering::SeqCst);
        assert!(eventually(|| watch.get(client).bps == 100_000_000));
        assert!(fake.lookups.load(Ordering::SeqCst) >= 2);
    }

    #[test]
    fn a_roamed_client_is_routed_at_once() {
        let (watch, fake) = watch(Duration::from_secs(3600));
        watch.get(peer("192.168.4.20:47998"));
        fake.bps.store(300_000_000, Ordering::SeqCst);
        let roamed = peer("[2001:db8::7]:47998");
        assert_eq!(watch.get(roamed).bps, 300_000_000);
        assert_eq!(fake.lookups.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_lookup_for_an_address_the_stream_left_is_dropped() {
        let mut state = State::default();
        let old = peer("192.168.4.20:47998");
        let new = peer("192.168.4.21:47998");
        let now = Instant::now();
        state.peer = Some(new);
        assert!(!state.publish(old, Link::default(), now));
        assert_eq!(state.current(old, INTERVAL, now), None);
        let link = Link {
            bps: 1,
            ..Default::default()
        };
        assert!(state.publish(new, link, now));
        assert_eq!(state.current(new, INTERVAL, now), Some(link));
        assert_eq!(state.current(old, INTERVAL, now), None);
    }

    #[test]
    fn without_a_watcher_the_route_ages_out_after_the_interval() {
        let mut state = State::default();
        let client = peer("192.168.4.20:47998");
        let now = Instant::now();
        state.peer = Some(client);
        state.publish(client, Link::default(), now);
        assert!(
            state
                .current(client, INTERVAL, now + INTERVAL / 2)
                .is_some()
        );
        assert!(state.current(client, INTERVAL, now + INTERVAL).is_none());
        state.watching = true;
        assert!(
            state
                .current(client, INTERVAL, now + INTERVAL * 10)
                .is_some()
        );
    }

    #[test]
    fn dropping_the_watch_stops_its_thread() {
        let (watch, fake) = watch(Duration::from_millis(5));
        watch.get(peer("192.168.4.20:47998"));
        assert!(eventually(|| fake.lookups.load(Ordering::SeqCst) >= 2));
        drop(watch);
        let after = fake.lookups.load(Ordering::SeqCst);
        thread::sleep(Duration::from_millis(50));
        assert_eq!(fake.lookups.load(Ordering::SeqCst), after);
    }

    #[test]
    fn the_slowest_lookup_is_reported_once() {
        let watch = LinkWatch::new(
            |_| {
                thread::sleep(Duration::from_millis(3));
                Link::default()
            },
            Duration::from_secs(3600),
        );
        watch.get(peer("192.168.4.20:47998"));
        assert!(watch.take_slowest() >= Duration::from_millis(3));
        assert_eq!(watch.take_slowest(), Duration::ZERO);
    }
}
