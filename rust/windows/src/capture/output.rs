//! Current colour information for one selected capture output. DXGI descriptions
//! belong to a factory snapshot: GetDesc1 on an old output can stay unchanged
//! after Windows changes HDR or display luminance.
//! https://learn.microsoft.com/windows/win32/api/dxgi1_6/nf-dxgi1_6-idxgioutput6-getdesc1

use super::*;

const CHECK_INTERVAL: Duration = Duration::from_secs(1);
/// Enumerations per poll while the display topology keeps changing, and the
/// wait before trying again when it is still changing.
const REFRESH_ATTEMPTS: usize = 3;
const REFRESH_RETRY: Duration = Duration::from_millis(50);

#[derive(Clone, Debug, PartialEq, Eq)]
struct Identity {
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

trait Source {
    type Output;
    fn current(&self) -> bool;
    fn refresh(&mut self, identity: &Identity) -> Result<Self::Output>;
    fn describe(&self, output: &Self::Output) -> Option<Description>;
}

struct Cache<S: Source> {
    source: S,
    identity: Identity,
    output: Option<S::Output>,
    next_check: Instant,
    snapshot: std::result::Result<Option<Description>, String>,
    metadata: butterpollo_core::hdr::Metadata,
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
        }
    }

    fn poll(&mut self, now: Instant) -> Result<Option<Description>> {
        if now >= self.next_check {
            self.next_check = now + CHECK_INTERVAL;
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

    fn metadata(&mut self, now: Instant) -> butterpollo_core::hdr::Metadata {
        let _ = self.poll(now);
        self.metadata
    }
}

struct Dxgi {
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
}

pub(super) struct Color(Cache<Dxgi>);

impl Color {
    pub(super) fn new(output: &IDXGIOutput1, name: &str) -> Result<Self> {
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
            Ok(Self(Cache::new(
                Dxgi { factory },
                identity,
                output.clone(),
                Instant::now(),
            )))
        }
    }

    pub(super) fn poll(&mut self, now: Instant) -> Result<Option<Description>> {
        self.0.poll(now)
    }

    pub(super) fn metadata(&mut self, now: Instant) -> butterpollo_core::hdr::Metadata {
        self.0.metadata(now)
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
}
