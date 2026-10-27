//! A running session's USB forwarding: the [`Forwarder`] that asks on its control stream.

use std::sync::Arc;

use super::Session;
use crate::http::HostClient;
use crate::usb::Forwarder;

impl Session {
    /// A [`Forwarder`] that asks on this session's control stream and opens its tunnels to
    /// `host`, the host this session streams from. Hand it every
    /// [`ControlEvent::Usb`](crate::control::ControlEvent::Usb); it ends when dropped.
    pub fn usb_forwarder(
        &self,
        host: Arc<HostClient>,
        changed: impl Fn() + Send + Sync + 'static,
    ) -> Forwarder {
        let control = self.control().clone();
        Forwarder::for_host(host, move |message| control.send(message), changed)
    }
}
