//! Retain controller identities across a client's transports. No input crosses client identities.
use crate::state::{Launch, Shared};
use anyhow::{Result, bail};
use butterpollo_core::{
    config::Config,
    input::Input,
    input_policy::{Policy, gamepad_profile},
    session::Warning,
};
use butterpollo_windows::input::GamepadThread;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const GRACE: Duration = Duration::from_secs(120);
struct Entry {
    pads: Arc<GamepadThread>,
    profile: u16,
    policy: Policy,
    idle: Instant,
}
/// The controllers by client and role, and the released ones whose thread
/// was stuck in the driver: kept apart, never handed to a client, until a
/// later release finds them ended.
#[derive(Default)]
pub struct Pool(
    Mutex<BTreeMap<String, Entry>>,
    Mutex<Vec<Arc<GamepadThread>>>,
);
/// The pool's key for `launch`'s controllers: its client and role.
pub fn key(launch: &Launch) -> String {
    format!("{}:{:?}", launch.client.uuid, launch.role)
}
impl Pool {
    pub fn acquire(&self, launch: &Launch, config: &Config) -> Result<Arc<GamepadThread>> {
        let key = key(launch);
        let profile = gamepad_profile(config.get("gamepad", "auto"));
        let policy = Policy::resolve(config)?;
        let mut entries = self.0.lock().unwrap();
        // Parked controllers wait for their own client's reconnect. Another
        // client's stay plugged beside this client's while a game runs, and
        // can hold the game's first player slot.
        entries.retain(|other, entry| {
            if *other == key || Arc::strong_count(&entry.pads) > 1 {
                return true;
            }
            drop(entry.pads.stop());
            false
        });
        if let Some(entry) = entries.get_mut(&key) {
            if Arc::strong_count(&entry.pads) > 1 {
                bail!("previous controller transport is closing");
            }
            if entry.profile == profile && entry.policy == policy {
                entry.idle = Instant::now();
                entry.pads.set_warnings(launch.warnings.clone());
                return Ok(entry.pads.clone());
            }
        }
        if let Some(old) = entries.remove(&key) {
            drop(old.pads.stop());
        }
        // There are only sixteen host controller slots; cap abandoned preparations as well.
        if entries.len() >= 16 {
            bail!("all controller sessions are in use");
        }
        let pads = Arc::new(GamepadThread::new(
            profile,
            policy.clone(),
            launch.warnings.clone(),
        )?);
        entries.insert(
            key,
            Entry {
                pads: pads.clone(),
                profile,
                policy,
                idle: Instant::now(),
            },
        );
        Ok(pads)
    }
    /// Whether a stream still holds any controller session; parked ones
    /// waiting for a reconnect do not count.
    pub fn in_use(&self) -> bool {
        self.0
            .lock()
            .unwrap()
            .values()
            .any(|entry| Arc::strong_count(&entry.pads) > 1)
    }
    /// Whether [`Self::release_parked`] has anything to unplug: a parked
    /// controller session no stream holds and no key in `kept`, or one whose
    /// release got stuck.
    pub fn parked(&self, kept: &BTreeSet<String>) -> bool {
        !self.1.lock().unwrap().is_empty()
            || self
                .0
                .lock()
                .unwrap()
                .iter()
                .any(|(key, entry)| Self::released(key, entry, kept))
    }
    fn released(key: &str, entry: &Entry, kept: &BTreeSet<String>) -> bool {
        Arc::strong_count(&entry.pads) == 1 && !kept.contains(key)
    }
    /// Unplug the parked controllers no stream holds, all at once within
    /// `timeout`, and return how many pads they had plugged in: a new
    /// Nucleus Co-op session does this before it plugs its players' pads,
    /// so a stream that ended no longer holds the first XInput slot (#56),
    /// and waits for XInput to show them gone (`pads::released`). Their
    /// clients get new ones when they reconnect. The ones under a key in
    /// `kept` stay: a pending launch's, prepared before its stream takes
    /// them.
    pub fn release_parked(&self, timeout: Duration, kept: &BTreeSet<String>) -> Result<u32> {
        let parked: Vec<Arc<GamepadThread>> = {
            let mut entries = self.0.lock().unwrap();
            let keys: Vec<String> = entries
                .iter()
                .filter(|(key, entry)| Self::released(key, entry, kept))
                .map(|(key, _)| key.clone())
                .collect();
            keys.iter()
                .filter_map(|key| entries.remove(key))
                .map(|entry| entry.pads)
                .collect()
        };
        let stuck = std::mem::take(&mut *self.1.lock().unwrap());
        let released = parked.len();
        let all: Vec<Arc<GamepadThread>> = stuck.into_iter().chain(parked).collect();
        let plugged = all.iter().map(|pads| pads.plugged_pads()).sum();
        for pads in &all {
            pads.request_stop();
        }
        let deadline = Instant::now() + timeout;
        let mut failed = None;
        let mut still = Vec::new();
        for pads in all {
            let left = deadline.saturating_duration_since(Instant::now());
            if let Err(error) = pads.stop_within(left) {
                failed.get_or_insert(error);
                still.push(pads);
            }
        }
        if !still.is_empty() {
            self.1.lock().unwrap().extend(still);
        }
        if let Some(error) = failed {
            return Err(error.context("a stream's parked controllers are still plugged in"));
        }
        if released > 0 {
            tracing::info!(
                streams = released,
                pads = plugged,
                "parked controllers released for Nucleus Co-op"
            );
        }
        Ok(plugged)
    }
    pub fn expire(&self, app_running: bool) {
        self.0.lock().unwrap().retain(|_, entry| {
            if Arc::strong_count(&entry.pads) > 1 {
                entry.idle = Instant::now();
            } else if !app_running && entry.idle.elapsed() >= GRACE {
                drop(entry.pads.stop());
                return false;
            }
            true
        });
    }
}
impl Drop for Pool {
    fn drop(&mut self) {
        for entry in self.0.get_mut().unwrap().values() {
            drop(entry.pads.stop());
        }
    }
}

/// Client supplies only neutral arrivals, over its paired HTTPS launch request.
fn arrivals(value: &str) -> Result<Vec<Input>> {
    if value.len() > 1024 {
        bail!("controller preparation is too large");
    }
    if value.is_empty() {
        return Ok(vec![]);
    }
    let mut seen = 0u16;
    value
        .split(',')
        .map(|entry| {
            let fields = entry
                .split(':')
                .map(str::parse::<u32>)
                .collect::<std::result::Result<Vec<_>, _>>()?;
            if fields.len() != 4
                || fields[0] >= 16
                || !(1..=4).contains(&fields[1])
                || fields[2] > u16::MAX.into()
                || seen & (1 << fields[0]) != 0
            {
                bail!("invalid controller preparation");
            }
            seen |= 1 << fields[0];
            Ok(Input::Arrival {
                id: fields[0] as u8,
                kind: fields[1] as u8,
                capabilities: fields[2] as u16,
                buttons: fields[3],
            })
        })
        .collect()
}
pub fn prepare(h: &Shared, launch: &Launch, value: &str) -> Result<()> {
    let events = arrivals(value)?;
    if events.is_empty() {
        return Ok(());
    }
    if events
        .iter()
        .any(|e| !launch.client.allows(e.required_permission()))
    {
        bail!("controller permission is required");
    }
    let config = crate::stream::effective_config(h, launch)?;
    if !Policy::resolve(&config)?.controller {
        bail!("controllers are disabled on the host");
    }
    let pads = h.controller_sessions.acquire(launch, &config)?;
    pads.activate();
    let mut mask = 0;
    for event in events {
        if let Input::Arrival { id, .. } = &event {
            mask |= 1 << id;
        }
        pads.send(event);
    }
    let ready = pads.wait_ready(mask, Duration::from_secs(12));
    pads.park();
    if !ready {
        bail!("{}", not_ready(&launch.warnings.snapshot()));
    }
    Ok(())
}
/// What a launch says when its controllers did not appear in time. A missing
/// driver or USB/IP client has raised its own warning by then, and that
/// warning says what to install: a reconnect alone would only fail the same
/// way again (#80). The driver that could not be opened comes first, then the
/// DualSense that needs USB/IP, then whatever else refused the controller.
fn not_ready(warnings: &[Warning]) -> String {
    let reason = [
        "input_gamepad",
        "input_dualsense",
        "input_gamepad_injection",
    ]
    .iter()
    .find_map(|code| warnings.iter().find(|warning| warning.code == *code));
    match reason {
        Some(warning) => format!("Controller setup has not finished: {}", warning.message),
        None => "Controller setup has not finished. Reconnect to retry before launching the game."
            .into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use butterpollo_core::session::Role;

    #[test]
    fn another_clients_parked_controllers_are_released_when_a_client_connects() {
        let f = crate::state::test_support::Fixture::new();
        let paired = f.client(0);
        let launch = |uuid: &str, role| {
            let mut client = paired.clone();
            client.uuid = uuid.into();
            f.launch(client, role)
        };
        let (phone, tv, laptop) = (
            launch("phone", Role::Stream),
            launch("tv", Role::Stream),
            launch("laptop", Role::Stream),
        );
        let config = Config::default();
        let pool = Pool::default();
        let keys = |pool: &Pool| pool.0.lock().unwrap().keys().cloned().collect::<Vec<_>>();
        // The phone's stream ends while its game keeps running: its controllers park.
        let parked = pool.acquire(&phone, &config).unwrap();
        drop(parked);
        // The TV's controllers are still in use when the laptop connects.
        let tv_pads = pool.acquire(&tv, &config).unwrap();
        assert_eq!(keys(&pool), ["tv:Stream"]);
        let laptop_pads = pool.acquire(&laptop, &config).unwrap();
        assert_eq!(keys(&pool), ["laptop:Stream", "tv:Stream"]);
        // A client reconnecting still gets its own parked controllers back.
        let retained = Arc::as_ptr(&laptop_pads);
        drop(laptop_pads);
        let again = pool.acquire(&laptop, &config).unwrap();
        assert_eq!(Arc::as_ptr(&again), retained);
        assert_eq!(keys(&pool), ["laptop:Stream", "tv:Stream"]);
        drop((tv_pads, again));
    }

    /// #56: a desktop stream's parked controller held XInput slot 0, and the
    /// background game that started next read it instead of the player's.
    #[test]
    fn parked_controllers_are_released_for_a_nucleus_launch_and_held_ones_stay() {
        let f = crate::state::test_support::Fixture::new();
        let paired = f.client(0);
        let launch = |uuid: &str| {
            let mut client = paired.clone();
            client.uuid = uuid.into();
            f.launch(client, Role::Stream)
        };
        let config = Config::default();
        let pool = Pool::default();
        let keys = |pool: &Pool| pool.0.lock().unwrap().keys().cloned().collect::<Vec<_>>();
        let none = BTreeSet::new();
        // The TV streams on; the phone's desktop stream ended and parked its
        // controller.
        let tv = pool.acquire(&launch("tv"), &config).unwrap();
        drop(pool.acquire(&launch("phone"), &config).unwrap());
        assert_eq!(keys(&pool), ["phone:Stream", "tv:Stream"]);
        assert!(pool.parked(&none));
        // No driver in the test: their threads never plugged a pad.
        assert_eq!(
            pool.release_parked(Duration::from_secs(5), &none).unwrap(),
            0
        );
        assert_eq!(keys(&pool), ["tv:Stream"]);
        assert!(pool.in_use());
        // The TV's held controller is nothing to release.
        assert!(!pool.parked(&none));
        // A pending launch's prepared controllers wait for its stream; they
        // stay. The TV's go once its stream ended.
        let laptop = launch("laptop");
        drop(pool.acquire(&laptop, &config).unwrap());
        let pending = BTreeSet::from([key(&laptop)]);
        assert!(!pool.parked(&pending));
        drop(tv);
        assert!(pool.parked(&pending));
        pool.release_parked(Duration::from_secs(5), &pending)
            .unwrap();
        assert_eq!(keys(&pool), ["laptop:Stream"]);
        assert!(!pool.parked(&pending));
        pool.release_parked(Duration::from_secs(5), &none).unwrap();
        assert!(keys(&pool).is_empty());
    }

    #[test]
    fn a_launch_without_controllers_names_the_missing_driver_instead_of_a_reconnect() {
        use butterpollo_core::session::{EVENT_PERIOD, Warnings};
        let warnings = Warnings::default();
        assert_eq!(
            not_ready(&warnings.snapshot()),
            "Controller setup has not finished. Reconnect to retry before launching the game."
        );
        // A warning about something else is no reason controllers are missing.
        warnings.set("input_steam_deck", "the Deck arrives as a virtual pad");
        assert!(not_ready(&warnings.snapshot()).contains("Reconnect to retry"));
        // The forced DualSense without usbip-win2 raises both of these.
        warnings.event("input_gamepad_injection", "input failed", EVENT_PERIOD);
        warnings.set("input_dualsense", "Install usbip-win2 on the host");
        assert_eq!(
            not_ready(&warnings.snapshot()),
            "Controller setup has not finished: Install usbip-win2 on the host"
        );
        warnings.set("input_gamepad", "run setup again");
        assert_eq!(
            not_ready(&warnings.snapshot()),
            "Controller setup has not finished: run setup again"
        );
        warnings.clear("input_gamepad");
        warnings.clear("input_dualsense");
        assert_eq!(
            not_ready(&warnings.snapshot()),
            "Controller setup has not finished: input failed"
        );
    }
    #[test]
    fn preparation_accepts_only_bounded_unique_neutral_arrivals() {
        assert_eq!(arrivals("0:2:33023:65535,1:4:256:1").unwrap().len(), 2);
        for bad in [
            "0:2:1:1,0:2:1:1",
            "16:2:1:0",
            "0:9:1:0",
            "0:2:65536:0",
            "0:2:0",
            "0:2:0:0:0",
            "-1:2:0:0",
        ] {
            assert!(arrivals(bad).is_err(), "{bad}");
        }
    }

    mod properties {
        use super::*;
        use proptest::prelude::*;
        use std::collections::BTreeMap;

        proptest! {
            /// What a client announces before launch arrives as written, once per controller.
            #[test]
            fn announced_controllers_round_trip(
                pads in proptest::collection::btree_map(0u8..16, (1u8..=4, any::<u16>(), any::<u32>()), 0..16)
            ) {
                let value = pads
                    .iter()
                    .map(|(id, (kind, caps, buttons))| format!("{id}:{kind}:{caps}:{buttons}"))
                    .collect::<Vec<_>>()
                    .join(",");
                let arrived: BTreeMap<_, _> = arrivals(&value)
                    .unwrap()
                    .into_iter()
                    .map(|event| match event {
                        Input::Arrival { id, kind, capabilities, buttons } => (id, (kind, capabilities, buttons)),
                        other => panic!("{other:?}"),
                    })
                    .collect();
                prop_assert_eq!(arrived, pads);
                if !value.is_empty() {
                    prop_assert!(arrivals(&format!("{value},{value}")).is_err(), "a controller twice");
                }
            }

            /// Anything else is refused or a valid list.
            #[test]
            fn any_preparation_is_valid_or_refused(value in r"([0-9:,+-]|\PC){0,40}") {
                if let Ok(events) = arrivals(&value) {
                    let mut seen = 0u32;
                    for event in events {
                        let Input::Arrival { id, kind, .. } = event else { panic!("{event:?}") };
                        prop_assert!(id < 16 && (1..=4).contains(&kind));
                        prop_assert!(seen & (1 << id) == 0);
                        seen |= 1 << id;
                    }
                }
            }
        }
    }
}
