//! Current colour information for one selected capture output. DXGI descriptions
//! belong to a factory snapshot: GetDesc1 on an old output can stay unchanged
//! after Windows changes HDR or display luminance.
//! https://learn.microsoft.com/windows/win32/api/dxgi1_6/nf-dxgi1_6-idxgioutput6-getdesc1

use super::*;
use std::sync::{Arc, Mutex, Weak};
use std::thread;

const CHECK_INTERVAL: Duration = Duration::from_secs(1);
/// Enumerations per poll while the display topology keeps changing, and the
/// wait before trying again when it is still changing.
const REFRESH_ATTEMPTS: usize = 3;
const REFRESH_RETRY: Duration = Duration::from_millis(50);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Identity {
    adapter: (u32, i32),
    name: String,
    monitor: usize,
}

fn bind_identity(expected: &str, identity: Identity, attached: bool) -> Result<Identity> {
    // Display selection and device creation enumerate separately. If their
    // ordinals changed in between, refuse the newly resolved wrong monitor.
    if !attached || identity.monitor == 0 || identity.name != expected {
        bail!("selected capture output changed or left the desktop: {expected}");
    }
    Ok(identity)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Description {
    pub color_space: DXGI_COLOR_SPACE_TYPE,
    pub metadata: butterpollo_core::hdr::Metadata,
}

/// Whether an output's colour space is the HDR10 signal the HDR encode path carries.
pub(super) fn in_hdr(description: Option<Description>) -> Option<bool> {
    description.map(|d| d.color_space == DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020)
}

pub(super) fn mode_changed(
    previous: Option<DXGI_COLOR_SPACE_TYPE>,
    current: Option<Description>,
) -> bool {
    current.is_some_and(|current| Some(current.color_space) != previous)
}

/// Never use an adapter/output ordinal, primary display or a name alone after
/// re-enumeration. A removed monitor's name can be reassigned to another one.
fn selected<T>(
    identity: &Identity,
    candidates: impl IntoIterator<Item = (Identity, T)>,
) -> Result<T> {
    let mut matching = candidates
        .into_iter()
        .filter_map(|(candidate, output)| (candidate == *identity).then_some(output));
    let output = matching
        .next()
        .with_context(|| format!("selected capture output is unavailable: {}", identity.name))?;
    if matching.next().is_some() {
        bail!("selected capture output is ambiguous: {}", identity.name);
    }
    Ok(output)
}

pub(super) trait Source {
    type Output;
    fn current(&self) -> bool;
    fn refresh(&mut self, identity: &Identity) -> Result<Self::Output>;
    fn describe(&self, output: &Self::Output) -> Option<Description>;
    /// The output's refresh rate in Hz, 0 when unknown.
    fn refresh_hz(&self, identity: &Identity) -> u32;
}

struct Cache<S: Source> {
    source: S,
    identity: Identity,
    output: Option<S::Output>,
    next_check: Instant,
    snapshot: std::result::Result<Option<Description>, String>,
    metadata: butterpollo_core::hdr::Metadata,
    /// The refresh rate in Hz at the last check, 0 when unknown.
    refresh: u32,
}

impl<S: Source> Cache<S> {
    fn new(source: S, identity: Identity, output: S::Output, now: Instant) -> Self {
        Self {
            source,
            identity,
            output: Some(output),
            next_check: now,
            snapshot: Ok(None),
            metadata: Default::default(),
            refresh: 0,
        }
    }

    fn poll(&mut self, now: Instant) -> Result<Option<Description>> {
        if now >= self.next_check {
            self.next_check = now + CHECK_INTERVAL;
            // Read with the colour, at its interval only.
            self.refresh = self.source.refresh_hz(&self.identity);
            let mut attempts = 0;
            let result = loop {
                attempts += 1;
                let attempt = (|| {
                    if self.output.is_none() || !self.source.current() {
                        self.output = None;
                        self.output = Some(self.source.refresh(&self.identity)?);
                    }
                    let description = self.source.describe(self.output.as_ref().unwrap());
                    // The topology can change again during enumeration. Do not
                    // publish an already stale factory's description as current.
                    anyhow::Ok(self.source.current().then_some(description))
                })();
                match attempt {
                    Ok(None) if attempts < REFRESH_ATTEMPTS => {}
                    attempt => break attempt,
                }
            };
            self.snapshot = match result {
                Ok(Some(description)) => {
                    if let Some(description) = description {
                        self.metadata = description.metadata;
                    }
                    Ok(description)
                }
                // Displays are still changing (another display arriving, or
                // its mode and HDR being set). That is not a failed output:
                // capture must not restart or fail to open for it. Keep the
                // last published state and look again on the next poll.
                Ok(None) => {
                    self.output = None;
                    self.next_check = now + REFRESH_RETRY;
                    self.snapshot.clone()
                }
                Err(error) => {
                    // Retry even if the failed lookup left a current factory.
                    // Metadata retains the last known value for this output.
                    self.output = None;
                    Err(format!("{error:#}"))
                }
            };
        }
        self.snapshot.clone().map_err(anyhow::Error::msg)
    }

    #[cfg(test)]
    fn metadata(&mut self, now: Instant) -> butterpollo_core::hdr::Metadata {
        let _ = self.poll(now);
        self.metadata
    }
}

pub(super) struct Dxgi {
    factory: IDXGIFactory1,
}

impl Source for Dxgi {
    type Output = IDXGIOutput1;

    fn current(&self) -> bool {
        // SAFETY: The cache owns the live factory throughout this query.
        unsafe { self.factory.IsCurrent().as_bool() }
    }

    fn refresh(&mut self, identity: &Identity) -> Result<Self::Output> {
        // SAFETY: Enumeration returns owned COM references; descriptors are
        // owned values and no output ordinal survives this enumeration.
        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
            let mut candidates = Vec::new();
            for index in 0..32 {
                let Ok(adapter) = factory.EnumAdapters1(index) else {
                    break;
                };
                let desc = adapter.GetDesc1()?;
                if (desc.AdapterLuid.LowPart, desc.AdapterLuid.HighPart) != identity.adapter {
                    continue;
                }
                for index in 0..32 {
                    let Ok(output) = adapter.EnumOutputs(index) else {
                        break;
                    };
                    let desc = output.GetDesc()?;
                    if !desc.AttachedToDesktop.as_bool() {
                        continue;
                    }
                    candidates.push((
                        Identity {
                            adapter: identity.adapter,
                            name: from_wide(&desc.DeviceName),
                            monitor: desc.Monitor.0 as usize,
                        },
                        output.cast::<IDXGIOutput1>()?,
                    ));
                }
            }
            let output = selected(identity, candidates)?;
            self.factory = factory;
            Ok(output)
        }
    }

    fn describe(&self, output: &Self::Output) -> Option<Description> {
        // Preserve the existing fallback when GetDesc1/IDXGIOutput6 is not
        // available: WGC uses the requested HDR state and metadata stays known
        // or at its default. Missing selected outputs fail in refresh instead.
        // SAFETY: `output` is a live owned COM reference; GetDesc1 returns a value.
        unsafe {
            output
                .cast::<IDXGIOutput6>()
                .and_then(|output| output.GetDesc1())
                .ok()
                .map(|desc| Description {
                    color_space: desc.ColorSpace,
                    metadata: butterpollo_core::hdr::Metadata::display(
                        desc.MaxLuminance,
                        desc.MinLuminance,
                        desc.MaxFullFrameLuminance,
                    ),
                })
        }
    }

    fn refresh_hz(&self, identity: &Identity) -> u32 {
        current_refresh_hz(&identity.name)
    }
}

/// The current mode's refresh rate of the display named `name` (its GDI
/// device name), in Hz; 0 when unknown.
pub(crate) fn current_refresh_hz(name: &str) -> u32 {
    use windows::Win32::Graphics::Gdi::{DEVMODEW, ENUM_CURRENT_SETTINGS, EnumDisplaySettingsW};
    let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    let mut mode = DEVMODEW {
        dmSize: size_of::<DEVMODEW>() as u16,
        ..Default::default()
    };
    // SAFETY: `name` is NUL-terminated and `mode` has dmSize set; both outlive the call.
    if unsafe { EnumDisplaySettingsW(PCWSTR(name.as_ptr()), ENUM_CURRENT_SETTINGS, &mut mode) }
        .as_bool()
        && mode.dmDisplayFrequency > 1
    {
        mode.dmDisplayFrequency
    } else {
        0
    }
}

/// A check's result: the description or why it failed, the last known
/// luminance, and the refresh rate in Hz (0 when unknown).
pub(super) type Snapshot = (
    std::result::Result<Option<Description>, String>,
    butterpollo_core::hdr::Metadata,
    u32,
);

/// With no read for this long the watcher stops; the next read checks at once.
const IDLE: Duration = Duration::from_secs(5);

/// One output's colour, checked on a thread of its own. The check (IsCurrent,
/// GetDesc1 and, after a topology change, a new factory's enumeration) ran on
/// the encode and capture threads, under the lock both take; they now read the
/// last check.
pub(super) struct Watched<S: Source> {
    cache: Mutex<Cache<S>>,
    published: Mutex<Published>,
}

#[derive(Default)]
struct Published {
    /// Kept current by the watcher while it runs.
    snapshot: Option<Snapshot>,
    last_read: Option<Instant>,
    watching: bool,
    /// The watcher thread could not start; told once.
    spawn_failed: bool,
}

pub(super) type Color = Watched<Dxgi>;

impl<S: Source + Send + 'static> Watched<S>
where
    S::Output: Send,
{
    fn new(cache: Cache<S>) -> Self {
        Self {
            cache: Mutex::new(cache),
            published: Mutex::default(),
        }
    }

    /// The last check, or a check now when no watcher keeps one.
    pub(super) fn read(self: &Arc<Self>) -> Snapshot {
        self.read_with(Instant::now(), |watched| {
            thread::Builder::new()
                .name("display-watch".into())
                .spawn(move || watch(watched))
                .map(drop)
        })
    }

    fn read_with(
        self: &Arc<Self>,
        now: Instant,
        spawn: impl FnOnce(Weak<Self>) -> std::io::Result<()>,
    ) -> Snapshot {
        {
            let mut published = self.published.lock().unwrap();
            published.last_read = Some(now);
            if let Some(snapshot) = &published.snapshot {
                return snapshot.clone();
            }
        }
        let snapshot = self.check(now);
        let mut published = self.published.lock().unwrap();
        if !published.watching && !published.spawn_failed {
            match spawn(Arc::downgrade(self)) {
                Ok(()) => published.watching = true,
                Err(error) => {
                    // Without the thread, every read checks as before.
                    published.spawn_failed = true;
                    tracing::warn!(%error, "the display colour watcher did not start; capture and encode check the display themselves");
                }
            }
        }
        if published.watching {
            published.snapshot = Some(snapshot.clone());
        }
        snapshot
    }

    fn check(&self, now: Instant) -> Snapshot {
        let mut cache = self.cache.lock().unwrap();
        let _ = cache.poll(now);
        (cache.snapshot.clone(), cache.metadata, cache.refresh)
    }

    fn next_check(&self) -> Instant {
        self.cache.lock().unwrap().next_check
    }

    /// One pass of the watcher; false when it stops, unread for [`IDLE`].
    fn tick(&self, now: Instant) -> bool {
        {
            let mut published = self.published.lock().unwrap();
            if published
                .last_read
                .is_none_or(|at| now.saturating_duration_since(at) >= IDLE)
            {
                published.snapshot = None;
                published.watching = false;
                return false;
            }
        }
        let snapshot = self.check(now);
        let mut published = self.published.lock().unwrap();
        if published.watching {
            published.snapshot = Some(snapshot);
        }
        true
    }
}

/// Holds the output only while it checks: the last device clone dropped ends it.
fn watch<S: Source + Send + 'static>(watched: Weak<Watched<S>>)
where
    S::Output: Send,
{
    loop {
        let Some(due) = watched.upgrade().map(|watched| watched.next_check()) else {
            return;
        };
        thread::sleep(due.saturating_duration_since(Instant::now()));
        let Some(watched) = watched.upgrade() else {
            return;
        };
        if !watched.tick(Instant::now()) {
            return;
        }
    }
}

/// A watched output's published refresh rate, for readers that know the
/// output only by name (the present-time stamper).
trait PublishedRefresh: Send + Sync {
    fn refresh_hz(self: Arc<Self>) -> u32;
}

impl<S: Source + Send + 'static> PublishedRefresh for Watched<S>
where
    S::Output: Send,
{
    fn refresh_hz(self: Arc<Self>) -> u32 {
        self.read().2
    }
}

/// The watched outputs by GDI device name; a newer device's watch replaces
/// an older one, and dropped ones are skipped.
static REFRESH: Mutex<Vec<(String, Weak<dyn PublishedRefresh>)>> = Mutex::new(Vec::new());

fn register<S: Source + Send + 'static>(name: &str, watched: &Arc<Watched<S>>)
where
    S::Output: Send,
{
    let watched: Arc<dyn PublishedRefresh> = watched.clone();
    let mut outputs = REFRESH.lock().unwrap();
    // Another device on the same output (a GPU probe's) keeps its own entry:
    // any live one answers, whichever stops first.
    outputs.retain(|(_, weak)| weak.strong_count() > 0);
    outputs.push((name.to_owned(), Arc::downgrade(&watched)));
}

/// The refresh rate the display watcher last published for the output named
/// `name`; `None` when no device watches it.
pub(crate) fn published_refresh_hz(name: &str) -> Option<u32> {
    let watched = REFRESH
        .lock()
        .unwrap()
        .iter()
        .rev()
        .filter(|(other, _)| other == name)
        .find_map(|(_, weak)| weak.upgrade())?;
    Some(watched.refresh_hz())
}

impl Color {
    /// Makes the colour readable by output name too, for the refresh rate.
    pub(super) fn publish(self: &Arc<Self>) {
        let name = self.cache.lock().unwrap().identity.name.clone();
        register(&name, self);
    }

    pub(super) fn open(output: &IDXGIOutput1, name: &str) -> Result<Self> {
        // Use the output's owner, which may differ from the encode device's
        // adapter when WGC captures a display on another GPU.
        // SAFETY: The output is live; GetParent returns owned COM references.
        unsafe {
            let adapter: IDXGIAdapter1 = output.GetParent()?;
            let adapter_desc = adapter.GetDesc1()?;
            let desc = output.GetDesc()?;
            let identity = bind_identity(
                name,
                Identity {
                    adapter: (
                        adapter_desc.AdapterLuid.LowPart,
                        adapter_desc.AdapterLuid.HighPart,
                    ),
                    name: from_wide(&desc.DeviceName),
                    monitor: desc.Monitor.0 as usize,
                },
                desc.AttachedToDesktop.as_bool(),
            )?;
            let factory = adapter.GetParent()?;
            Ok(Self::new(Cache::new(
                Dxgi { factory },
                identity,
                output.clone(),
                Instant::now(),
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    struct Fake {
        current: bool,
        fresh_current: bool,
        /// Refreshes whose factory is already stale when they return.
        stale_refreshes: usize,
        candidates: Vec<(Identity, Option<Description>)>,
        enumerations: usize,
        reads: Cell<usize>,
        refresh: u32,
        refresh_reads: Cell<usize>,
    }

    impl Source for Fake {
        type Output = Option<Description>;
        fn current(&self) -> bool {
            self.current
        }
        fn refresh(&mut self, identity: &Identity) -> Result<Self::Output> {
            self.enumerations += 1;
            self.current = self.fresh_current && self.stale_refreshes == 0;
            self.stale_refreshes = self.stale_refreshes.saturating_sub(1);
            selected(identity, self.candidates.clone())
        }
        fn describe(&self, output: &Self::Output) -> Option<Description> {
            self.reads.set(self.reads.get() + 1);
            *output
        }
        fn refresh_hz(&self, _: &Identity) -> u32 {
            self.refresh_reads.set(self.refresh_reads.get() + 1);
            self.refresh
        }
    }

    fn identity() -> Identity {
        Identity {
            adapter: (7, 2),
            name: "selected".into(),
            monitor: 19,
        }
    }

    fn description(hdr: bool, peak: f32) -> Description {
        Description {
            color_space: if hdr {
                DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020
            } else {
                DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709
            },
            metadata: butterpollo_core::hdr::Metadata::display(peak, 0.001, 400.),
        }
    }

    fn cache(now: Instant, initial: Option<Description>) -> Cache<Fake> {
        Cache::new(
            Fake {
                current: true,
                fresh_current: true,
                stale_refreshes: 0,
                candidates: vec![(identity(), initial)],
                enumerations: 0,
                reads: Cell::new(0),
                refresh: 120,
                refresh_reads: Cell::new(0),
            },
            identity(),
            initial,
            now,
        )
    }

    #[test]
    fn initial_binding_refuses_reordered_or_detached_outputs() {
        assert_eq!(
            bind_identity("selected", identity(), true).unwrap(),
            identity()
        );
        assert!(bind_identity("another display", identity(), true).is_err());
        assert!(bind_identity("selected", identity(), false).is_err());
        assert!(
            bind_identity(
                "selected",
                Identity {
                    monitor: 0,
                    ..identity()
                },
                true
            )
            .is_err()
        );
    }

    #[test]
    fn current_output_is_shared_and_throttled_without_reenumeration() {
        let now = Instant::now();
        let expected = description(true, 1000.);
        let shared = std::sync::Arc::new(std::sync::Mutex::new(cache(now, Some(expected))));
        let encoder = shared.clone();
        assert_eq!(shared.lock().unwrap().poll(now).unwrap(), Some(expected));
        assert_eq!(encoder.lock().unwrap().metadata(now), expected.metadata);
        assert_eq!(
            shared
                .lock()
                .unwrap()
                .poll(now + Duration::from_millis(999))
                .unwrap(),
            Some(expected)
        );
        assert_eq!(shared.lock().unwrap().source.reads.get(), 1);
        shared.lock().unwrap().poll(now + CHECK_INTERVAL).unwrap();
        assert_eq!(shared.lock().unwrap().source.reads.get(), 2);
        assert_eq!(shared.lock().unwrap().source.enumerations, 0);
    }

    #[test]
    fn stale_factory_refreshes_both_hdr_transitions_on_the_selected_output() {
        for (before, after) in [(false, true), (true, false)] {
            let now = Instant::now();
            let old = description(before, 1000.);
            let new = description(after, 600.);
            let mut cache = cache(now, Some(old));
            cache.poll(now).unwrap();
            // Reordered outputs include another GPU with the same desktop name
            // and another monitor reusing that name on the capture adapter.
            cache.source.candidates = vec![
                (
                    Identity {
                        adapter: (8, 2),
                        ..identity()
                    },
                    Some(old),
                ),
                (
                    Identity {
                        monitor: 20,
                        ..identity()
                    },
                    Some(old),
                ),
                (identity(), Some(new)),
            ];
            cache.source.current = false;
            let current = cache.poll(now + CHECK_INTERVAL).unwrap();
            assert_eq!(current, Some(new));
            assert!(mode_changed(Some(old.color_space), current));
            assert_eq!(cache.source.enumerations, 1);
        }
    }

    #[test]
    fn luminance_refresh_updates_metadata_without_a_capture_mode_change() {
        let now = Instant::now();
        let old = description(true, 1000.);
        let new = description(true, 650.);
        let mut cache = cache(now, Some(old));
        cache.poll(now).unwrap();
        cache.source.current = false;
        cache.source.candidates = vec![(identity(), Some(new))];
        assert_eq!(cache.metadata(now + CHECK_INTERVAL), new.metadata);
        assert!(!mode_changed(
            Some(old.color_space),
            cache.poll(now + CHECK_INTERVAL).unwrap()
        ));
        assert_eq!(cache.source.enumerations, 1);
    }

    #[test]
    fn removed_output_never_uses_another_monitor_and_refresh_retries() {
        let now = Instant::now();
        let old = description(true, 1000.);
        let mut cache = cache(now, Some(old));
        cache.poll(now).unwrap();
        cache.source.current = false;
        cache.source.candidates = vec![(
            Identity {
                monitor: 20,
                ..identity()
            },
            Some(description(false, 300.)),
        )];
        assert!(cache.poll(now + CHECK_INTERVAL).is_err());
        assert_eq!(cache.metadata(now + CHECK_INTERVAL), old.metadata);
        assert_eq!(cache.source.enumerations, 1);
        // Failed lookup installed a current factory; absence still forces retry.
        assert!(cache.source.current);
        let recovered = description(true, 750.);
        cache.source.candidates = vec![(identity(), Some(recovered))];
        assert_eq!(
            cache.poll(now + CHECK_INTERVAL * 2).unwrap(),
            Some(recovered)
        );
        assert_eq!(cache.source.enumerations, 2);
    }

    #[test]
    fn ambiguous_output_identity_is_refused() {
        assert!(selected(&identity(), [(identity(), 1), (identity(), 2)]).is_err());
    }

    #[test]
    fn already_stale_replacement_is_not_published_and_is_retried() {
        let now = Instant::now();
        let old = description(true, 1000.);
        let new = description(false, 300.);
        let mut cache = cache(now, Some(old));
        cache.poll(now).unwrap();
        cache.source.current = false;
        cache.source.fresh_current = false;
        cache.source.candidates = vec![(identity(), Some(new))];
        assert_eq!(cache.poll(now + CHECK_INTERVAL).unwrap(), Some(old));
        assert_eq!(cache.metadata(now + CHECK_INTERVAL), old.metadata);
        cache.source.fresh_current = true;
        // Retried soon, without waiting another check interval.
        assert_eq!(
            cache.poll(now + CHECK_INTERVAL + REFRESH_RETRY).unwrap(),
            Some(new)
        );
    }

    #[test]
    fn topology_changes_during_a_refresh_do_not_fail_capture() {
        // Another display arriving, or its mode and HDR being set, changes
        // the factory while the selected output is enumerated. Capture opens
        // and its periodic colour check propagate errors, so this must not be
        // one: v2.2.1 tolerated it, and failing restarted or refused capture.
        let now = Instant::now();
        let old = description(true, 1000.);
        let new = description(false, 300.);
        let mut cache = cache(now, Some(old));
        cache.source.current = false;
        cache.source.stale_refreshes = 1;
        cache.source.candidates = vec![(identity(), Some(new))];
        // At open: one more change during enumeration is absorbed in this poll.
        assert_eq!(cache.poll(now).unwrap(), Some(new));
        assert_eq!(cache.source.enumerations, 2);
        // Mid-stream: changes outlasting the poll keep the published state.
        cache.source.current = false;
        cache.source.stale_refreshes = REFRESH_ATTEMPTS;
        cache.source.candidates = vec![(identity(), Some(old))];
        let current = cache.poll(now + CHECK_INTERVAL).unwrap();
        assert_eq!(current, Some(new));
        assert!(!mode_changed(Some(new.color_space), current));
        assert_eq!(cache.metadata, new.metadata);
        assert_eq!(cache.source.enumerations, 2 + REFRESH_ATTEMPTS);
        assert_eq!(
            cache.poll(now + CHECK_INTERVAL + REFRESH_RETRY).unwrap(),
            Some(old)
        );
        assert_eq!(cache.source.enumerations, 3 + REFRESH_ATTEMPTS);
        // A missing output remains an error.
        cache.source.current = false;
        cache.source.candidates.clear();
        assert!(cache.poll(now + CHECK_INTERVAL * 3).is_err());
    }

    #[test]
    fn unavailable_description_preserves_unknown_mode_and_last_known_metadata() {
        let now = Instant::now();
        let mut cache = cache(now, None);
        assert_eq!(cache.poll(now).unwrap(), None);
        assert!(!mode_changed(None, None));
        assert_eq!(cache.metadata(now), Default::default());
        let known = description(true, 850.);
        cache.source.current = false;
        cache.source.candidates = vec![(identity(), Some(known))];
        let current = cache.poll(now + CHECK_INTERVAL).unwrap();
        assert!(mode_changed(None, current));
        assert_eq!(cache.metadata(now + CHECK_INTERVAL), known.metadata);
        cache.source.current = false;
        cache.source.candidates = vec![(identity(), None)];
        assert_eq!(cache.poll(now + CHECK_INTERVAL * 2).unwrap(), None);
        assert_eq!(cache.metadata(now + CHECK_INTERVAL * 2), known.metadata);
    }

    fn watched(now: Instant, initial: Option<Description>) -> Arc<Watched<Fake>> {
        Arc::new(Watched::new(cache(now, initial)))
    }

    fn reads(watched: &Watched<Fake>) -> usize {
        watched.cache.lock().unwrap().source.reads.get()
    }

    #[test]
    fn hot_reads_take_the_watchers_last_check_without_querying() {
        let now = Instant::now();
        let old = description(true, 1000.);
        let new = description(false, 300.);
        let watched = watched(now, Some(old));
        let mut spawned = 0;
        assert_eq!(
            watched
                .read_with(now, |_| {
                    spawned += 1;
                    Ok(())
                })
                .0,
            Ok(Some(old))
        );
        assert_eq!(spawned, 1);
        assert_eq!(reads(&watched), 1);
        {
            let mut cache = watched.cache.lock().unwrap();
            cache.source.current = false;
            cache.source.candidates = vec![(identity(), Some(new))];
        }
        // Past the check interval, a read still only takes the published check.
        let later = now + CHECK_INTERVAL * 3 / 2;
        assert_eq!(
            watched.read_with(later, |_| unreachable!()).0,
            Ok(Some(old))
        );
        assert_eq!(reads(&watched), 1);
        // The watcher's check publishes the change, as a poll did.
        assert!(watched.tick(later));
        let (current, metadata, _) = watched.read_with(later, |_| unreachable!());
        assert_eq!(current, Ok(Some(new)));
        assert!(mode_changed(Some(old.color_space), current.unwrap()));
        assert_eq!(metadata, new.metadata);
        assert_eq!(watched.cache.lock().unwrap().source.enumerations, 1);
    }

    #[test]
    fn a_failed_check_is_published_with_the_last_known_metadata() {
        let now = Instant::now();
        let old = description(true, 1000.);
        let watched = watched(now, Some(old));
        let _ = watched.read_with(now, |_| Ok(()));
        {
            let mut cache = watched.cache.lock().unwrap();
            cache.source.current = false;
            cache.source.candidates.clear();
        }
        assert!(watched.tick(now + CHECK_INTERVAL));
        let (current, metadata, _) = watched.read_with(now + CHECK_INTERVAL, |_| unreachable!());
        assert!(current.is_err());
        assert_eq!(metadata, old.metadata);
    }

    #[test]
    fn an_unread_watcher_stops_and_the_next_read_checks_at_once() {
        let now = Instant::now();
        let old = description(true, 1000.);
        let new = description(true, 600.);
        let watched = watched(now, Some(old));
        let _ = watched.read_with(now, |_| Ok(()));
        assert!(watched.tick(now + CHECK_INTERVAL));
        assert!(!watched.tick(now + IDLE));
        {
            let mut cache = watched.cache.lock().unwrap();
            cache.source.current = false;
            cache.source.candidates = vec![(identity(), Some(new))];
        }
        let mut spawned = false;
        let (current, _, _) = watched.read_with(now + IDLE * 2, |_| {
            spawned = true;
            Ok(())
        });
        assert_eq!(current, Ok(Some(new)));
        assert!(spawned);
    }

    #[test]
    fn without_a_watcher_every_read_checks_as_before() {
        let now = Instant::now();
        let watched = watched(now, Some(description(true, 1000.)));
        let mut spawns = 0;
        for step in 0..3 {
            let _ = watched.read_with(now + CHECK_INTERVAL * step, |_| {
                spawns += 1;
                Err(std::io::Error::other("no threads"))
            });
        }
        // Told once; the cache's own interval throttles the checks.
        assert_eq!(spawns, 1);
        assert_eq!(reads(&watched), 3);
        assert!(watched.published.lock().unwrap().snapshot.is_none());
    }

    #[test]
    fn the_watcher_thread_ends_with_the_last_reader() {
        let now = Instant::now();
        let watched = watched(now, Some(description(true, 1000.)));
        let mut handle = None;
        let _ = watched.read_with(now, |weak| {
            handle = Some(thread::spawn(move || watch(weak)));
            Ok(())
        });
        // Waiting for its next check, the watcher holds no reference.
        thread::sleep(Duration::from_millis(200));
        assert!(!handle.as_ref().unwrap().is_finished());
        assert_eq!(Arc::strong_count(&watched), 1);
        drop(watched);
        let handle = handle.unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !handle.is_finished() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(handle.is_finished());
        handle.join().expect("the watcher ends without a panic");
    }

    #[test]
    fn a_refresh_change_is_published_by_the_watcher_and_found_by_name() {
        let now = Instant::now();
        let watched = watched(now, Some(description(false, 300.)));
        assert_eq!(watched.read_with(now, |_| Ok(())).2, 120);
        // A game's mode switch: hot reads keep the published rate until the watcher checks.
        watched.cache.lock().unwrap().source.refresh = 60;
        assert_eq!(watched.read_with(now, |_| unreachable!()).2, 120);
        assert!(watched.tick(now + CHECK_INTERVAL));
        assert_eq!(
            watched
                .read_with(now + CHECK_INTERVAL, |_| unreachable!())
                .2,
            60
        );
        // The stamper finds it by output name while a device holds it.
        let name = r"\.\REFRESH-TEST";
        register(name, &watched);
        assert!(published_refresh_hz(name).is_some());
        drop(watched);
        assert_eq!(published_refresh_hz(name), None);
    }

    #[test]
    fn another_device_on_the_same_output_does_not_hide_the_stream_s() {
        // A GPU probe opens its own device on the stream's display after the
        // stream did, and stops mid-stream.
        let now = Instant::now();
        let name = r"\.\REFRESH-PROBE";
        let stream = watched(now, None);
        let probe = watched(now, None);
        probe.cache.lock().unwrap().source.refresh = 60;
        register(name, &stream);
        register(name, &probe);
        assert!(published_refresh_hz(name).is_some());
        drop(probe);
        assert_eq!(published_refresh_hz(name), Some(120));
        drop(stream);
        assert_eq!(published_refresh_hz(name), None);
    }

    #[test]
    fn the_refresh_is_read_at_the_check_interval_even_without_a_watcher() {
        let now = Instant::now();
        let watched = watched(now, None);
        for ms in [0, 10, 500, 999] {
            let _ = watched.read_with(now + Duration::from_millis(ms), |_| {
                Err(std::io::Error::other("no threads"))
            });
        }
        assert_eq!(watched.cache.lock().unwrap().source.refresh_reads.get(), 1);
        let _ = watched.read_with(now + CHECK_INTERVAL, |_| unreachable!());
        assert_eq!(watched.cache.lock().unwrap().source.refresh_reads.get(), 2);
    }

    #[test]
    fn the_display_is_in_hdr_by_its_color_space_not_its_frame_format() {
        assert_eq!(in_hdr(Some(description(true, 850.))), Some(true));
        assert_eq!(in_hdr(Some(description(false, 80.))), Some(false));
        // An output that cannot say is not assumed to be in HDR.
        assert_eq!(in_hdr(None), None);
    }
}
