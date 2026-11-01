//! The route to the client, looked up on a thread of its own. The lookup
//! (GetBestInterfaceEx and GetIfEntry2Ex, more on a virtual adapter) took
//! the encode thread before an AMF submit and the paced sender between
//! frames; they now read the watcher's latest answer.
use butterpollo_windows::net::Link;
use std::{
    net::SocketAddr,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

/// How often the route is looked up again, as the stream threads did.
const INTERVAL: Duration = Duration::from_secs(2);
/// A lookup slower than this is reported: the route it answers is stale.
const SLOW_LOOKUP: Duration = Duration::from_secs(2);
/// Client addresses followed at once: while a client roams, the paced sender
/// still sends the old address's frames as the encoder meets the new one.
const ROUTES: usize = 2;

type Query = dyn Fn(SocketAddr) -> Link + Send + Sync;

/// One per stream, shared by its encode loop and video senders.
pub(crate) struct LinkWatch {
    inner: Arc<Inner>,
}

struct Inner {
    state: Mutex<State>,
    changed: Condvar,
    query: Box<Query>,
    interval: Duration,
    /// The longest lookup since the timings log last read it, in microseconds.
    slowest: AtomicU64,
}

#[derive(Clone, Copy)]
struct Route {
    peer: SocketAddr,
    link: Link,
    /// When the lookup was made.
    at: Instant,
    /// When a stream thread last asked for it.
    used: Instant,
}

#[derive(Default)]
struct State {
    /// The latest lookups, for the client addresses the stream sent to last.
    routes: Vec<Route>,
    watching: bool,
    /// The watcher thread could not start; told once.
    spawn_failed: bool,
    /// A stale route was reported; reset by a fresh lookup.
    stale_warned: bool,
    stopped: bool,
}

impl State {
    /// A lookup made for `peer`; one for an address the stream has left is
    /// stale and dropped.
    fn publish(&mut self, peer: SocketAddr, link: Link, at: Instant) -> bool {
        let Some(route) = self.routes.iter_mut().find(|r| r.peer.ip() == peer.ip()) else {
            return false;
        };
        route.link = link;
        route.at = at;
        self.stale_warned = false;
        true
    }

    /// A route a stream thread looked up itself, replacing the one used least
    /// recently once [`ROUTES`] are followed.
    fn insert(&mut self, peer: SocketAddr, link: Link, now: Instant) {
        self.routes.retain(|r| r.peer.ip() != peer.ip());
        if self.routes.len() >= ROUTES
            && let Some(oldest) = (0..self.routes.len()).min_by_key(|&i| self.routes[i].used)
        {
            self.routes.remove(oldest);
        }
        self.routes.push(Route {
            peer,
            link,
            at: now,
            used: now,
        });
    }

    /// The published route to `peer`, while it is still current: the
    /// watcher keeps it so, or without one, the interval bounds its age.
    fn current(&mut self, peer: SocketAddr, interval: Duration, now: Instant) -> Option<Link> {
        let watching = self.watching;
        let route = self.routes.iter_mut().find(|r| r.peer.ip() == peer.ip())?;
        let age = now.saturating_duration_since(route.at);
        if !watching && age >= interval {
            return None;
        }
        route.used = now;
        route.peer = peer;
        let link = route.link;
        if watching
            && age >= interval + SLOW_LOOKUP
            && !std::mem::replace(&mut self.stale_warned, true)
        {
            tracing::warn!(
                age_ms = age.as_millis() as u64,
                "the route to the client was not looked up again in time; pacing keeps the last known link"
            );
        }
        Some(link)
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
        }
    }

    /// The route to `peer`. A new client address is looked up here, once:
    /// its first frame needs the route, as before. The watcher follows it
    /// from then on.
    pub(crate) fn get(&self, peer: SocketAddr) -> Link {
        {
            let mut state = self.inner.state.lock().unwrap();
            if let Some(link) = state.current(peer, self.inner.interval, Instant::now()) {
                return link;
            }
        }
        let link = self.inner.lookup(peer);
        let mut state = self.inner.state.lock().unwrap();
        state.insert(peer, link, Instant::now());
        if !state.watching && !state.spawn_failed && !state.stopped {
            let inner = self.inner.clone();
            // Detached: a lookup stuck in the network stack must not hold up
            // the stream's teardown; the thread ends after that lookup.
            match thread::Builder::new()
                .name("link-watch".into())
                .spawn(move || inner.watch())
            {
                Ok(_) => state.watching = true,
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
    fn lookup(&self, peer: SocketAddr) -> Link {
        let started = Instant::now();
        let link = (self.query)(peer);
        let took = started.elapsed();
        self.slowest.fetch_max(
            took.as_micros().min(u128::from(u64::MAX)) as u64,
            Ordering::Relaxed,
        );
        if took >= SLOW_LOOKUP {
            tracing::warn!(
                took_ms = took.as_millis() as u64,
                "looking up the route to the client took long; the network stack was slow to answer"
            );
        }
        link
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
            let peers: Vec<SocketAddr> = state.routes.iter().map(|r| r.peer).collect();
            drop(state);
            let links: Vec<(SocketAddr, Link, Instant)> = peers
                .into_iter()
                .map(|peer| (peer, self.lookup(peer), Instant::now()))
                .collect();
            state = self.state.lock().unwrap();
            for (peer, link, at) in links {
                state.publish(peer, link, at);
            }
        }
    }
}

impl Drop for LinkWatch {
    fn drop(&mut self) {
        self.inner.state.lock().unwrap().stopped = true;
        self.inner.changed.notify_all();
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
        let newer = peer("192.168.4.22:47998");
        let now = Instant::now();
        state.insert(old, Link::default(), now);
        state.insert(new, Link::default(), now + Duration::from_millis(1));
        // A third address replaces the one used least recently.
        state.insert(newer, Link::default(), now + Duration::from_millis(2));
        assert!(!state.publish(old, Link::default(), now));
        assert_eq!(state.current(old, INTERVAL, now), None);
        let link = Link {
            bps: 1,
            ..Default::default()
        };
        assert!(state.publish(new, link, now));
        assert_eq!(state.current(new, INTERVAL, now), Some(link));
    }

    #[test]
    fn a_roaming_client_s_old_and_new_address_both_stay_routed() {
        let (watch, fake) = watch(Duration::from_secs(3600));
        let old = peer("192.168.4.20:47998");
        let new = peer("10.0.0.7:47998");
        watch.get(old);
        watch.get(new);
        assert_eq!(fake.lookups.load(Ordering::SeqCst), 2);
        // The paced sender's old-address frames and the encoder's new address alternate.
        for _ in 0..100 {
            watch.get(old);
            watch.get(new);
        }
        assert_eq!(fake.lookups.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn without_a_watcher_the_route_ages_out_after_the_interval() {
        let mut state = State::default();
        let client = peer("192.168.4.20:47998");
        let now = Instant::now();
        state.insert(client, Link::default(), now);
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
    fn a_stale_route_is_reported_once_until_a_lookup_answers() {
        let mut state = State::default();
        let client = peer("192.168.4.20:47998");
        let now = Instant::now();
        state.insert(client, Link::default(), now);
        state.watching = true;
        assert!(state.current(client, INTERVAL, now + INTERVAL).is_some());
        assert!(!state.stale_warned);
        let stale = now + INTERVAL + SLOW_LOOKUP;
        assert!(state.current(client, INTERVAL, stale).is_some());
        assert!(state.stale_warned);
        assert!(state.publish(client, Link::default(), stale));
        assert!(!state.stale_warned);
    }

    #[test]
    fn dropping_the_watch_stops_its_thread() {
        let (watch, fake) = watch(Duration::from_millis(5));
        watch.get(peer("192.168.4.20:47998"));
        assert!(eventually(|| fake.lookups.load(Ordering::SeqCst) >= 2));
        drop(watch);
        // A lookup already under way may still finish.
        thread::sleep(Duration::from_millis(20));
        let after = fake.lookups.load(Ordering::SeqCst);
        thread::sleep(Duration::from_millis(50));
        assert_eq!(fake.lookups.load(Ordering::SeqCst), after);
    }

    #[test]
    fn a_hung_lookup_does_not_hold_up_teardown() {
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let gate = release.clone();
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = calls.clone();
        let watch = LinkWatch::new(
            move |_| {
                if counted.fetch_add(1, Ordering::SeqCst) > 0 {
                    let (lock, changed) = &*gate;
                    let _held = changed
                        .wait_while(lock.lock().unwrap(), |released| !*released)
                        .unwrap();
                }
                Link::default()
            },
            Duration::from_millis(5),
        );
        watch.get(peer("192.168.4.20:47998"));
        assert!(eventually(|| calls.load(Ordering::SeqCst) >= 2));
        let started = Instant::now();
        drop(watch);
        assert!(started.elapsed() < Duration::from_secs(1));
        let (lock, changed) = &*release;
        *lock.lock().unwrap() = true;
        changed.notify_all();
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
