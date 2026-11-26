//! Endpoint change notifications for the audio route upkeep: an
//! `IMMNotificationClient` that only wakes the upkeep thread, so a default
//! device change or a vanished sink is handled at once instead of at the next
//! one-second poll. The poll stays as the safety net.
//!
//! The callbacks run on a Windows audio service thread. They never call back
//! into WASAPI (Microsoft forbids registering, unregistering or waiting on
//! audio work from inside them) and never touch the audio hot path: they only
//! run the wake closure, which in the host unparks the upkeep thread.

use anyhow::{Context, Result};
use std::{
    sync::Arc,
    thread,
    time::{Duration, Instant},
};
use windows::{
    Win32::{Foundation::PROPERTYKEY, Media::Audio::*},
    core::{PCWSTR, implement},
};

/// The upkeep's safety-net poll: it still runs this often with no event.
pub const POLL: Duration = Duration::from_secs(1);
/// The shortest gap between two upkeep rounds, so a burst of property
/// notifications cannot keep the upkeep asking Windows in a tight loop.
pub const MIN_GAP: Duration = Duration::from_millis(100);

/// What the upkeep runs when an endpoint changes.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// Where a listener registers: the device enumerator, or a fake in tests.
pub trait EndpointEvents {
    fn register(&self, client: &IMMNotificationClient) -> Result<()>;
    fn unregister(&self, client: &IMMNotificationClient) -> Result<()>;
}

impl EndpointEvents for IMMDeviceEnumerator {
    fn register(&self, client: &IMMNotificationClient) -> Result<()> {
        // SAFETY: `client` is a live COM interface; the enumerator AddRefs it.
        unsafe { self.RegisterEndpointNotificationCallback(client) }
            .context("RegisterEndpointNotificationCallback")
    }
    fn unregister(&self, client: &IMMNotificationClient) -> Result<()> {
        // SAFETY: `client` is the live interface registered above.
        unsafe { self.UnregisterEndpointNotificationCallback(client) }
            .context("UnregisterEndpointNotificationCallback")
    }
}

#[implement(IMMNotificationClient)]
struct Listener {
    wake: Wake,
}

impl IMMNotificationClient_Impl for Listener_Impl {
    fn OnDeviceStateChanged(&self, _: &PCWSTR, _: DEVICE_STATE) -> windows::core::Result<()> {
        (self.wake)();
        Ok(())
    }
    fn OnDeviceAdded(&self, _: &PCWSTR) -> windows::core::Result<()> {
        (self.wake)();
        Ok(())
    }
    fn OnDeviceRemoved(&self, _: &PCWSTR) -> windows::core::Result<()> {
        (self.wake)();
        Ok(())
    }
    fn OnDefaultDeviceChanged(
        &self,
        _: EDataFlow,
        _: ERole,
        _: &PCWSTR,
    ) -> windows::core::Result<()> {
        (self.wake)();
        Ok(())
    }
    fn OnPropertyValueChanged(&self, _: &PCWSTR, _: &PROPERTYKEY) -> windows::core::Result<()> {
        (self.wake)();
        Ok(())
    }
}

/// A registered listener. Dropping it unregisters the listener and releases
/// every reference this side holds, so its lifetime is the owner's (the
/// stream's audio route upkeep).
pub struct EndpointWatch {
    source: Box<dyn EndpointEvents>,
    client: IMMNotificationClient,
}

impl EndpointWatch {
    /// Register with the Windows device enumerator; the caller's thread has
    /// COM initialized.
    pub fn new(wake: Wake) -> Result<Self> {
        Self::register(Box::new(super::policy::enumerator()?), wake)
    }

    /// Register with `source`; on failure nothing stays registered.
    pub fn register(source: Box<dyn EndpointEvents>, wake: Wake) -> Result<Self> {
        let client: IMMNotificationClient = Listener { wake }.into();
        source.register(&client)?;
        Ok(Self { source, client })
    }
}

impl Drop for EndpointWatch {
    fn drop(&mut self) {
        if let Err(error) = self.source.unregister(&self.client) {
            tracing::warn!("audio endpoint notifications not unregistered: {error:#}");
        }
    }
}

/// Wait for the next upkeep round: until a wake unparks this thread or
/// [`POLL`] passes, then at least [`MIN_GAP`] after `last_round`. A wake
/// during the gap is kept by the park token and runs one more round after it.
pub fn park_until_round(last_round: Instant) {
    thread::park_timeout(POLL);
    let since = last_round.elapsed();
    if since < MIN_GAP {
        thread::sleep(MIN_GAP - since);
    }
}

#[cfg(test)]
mod tests;
