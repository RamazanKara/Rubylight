//! Retain controller identities across a client's transports. No input crosses client identities.
use crate::state::{Launch, Shared};
use anyhow::{Result, bail};
use butterpollo_core::{
    config::Config,
    input::Input,
    input_policy::{Policy, gamepad_profile},
};
use butterpollo_windows::input::GamepadThread;
use std::{
    collections::BTreeMap,
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
#[derive(Default)]
pub struct Pool(Mutex<BTreeMap<String, Entry>>);
impl Pool {
    pub fn acquire(&self, launch: &Launch, config: &Config) -> Result<Arc<GamepadThread>> {
        let key = format!("{}:{:?}", launch.client.uuid, launch.role);
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
        bail!("Controller setup has not finished. Reconnect to retry before launching the game.");
    }
    Ok(())
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
