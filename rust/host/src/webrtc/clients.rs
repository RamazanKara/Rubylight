//! The WebRTC devices: which connections the signalling server holds, the
//! pairing requests waiting in the console, and the credentials check.
//!
//! A paired device is an ordinary `Client` with an empty certificate and
//! `extra.webrtc` (see `butterpollo_core::state::Client::webrtc`). The
//! console's PIN page approves a request through `/api/pin`: the request
//! sits in `Host::pins` under its unguessable request ID with an empty
//! certificate, which no GameStream pairing has, and `/api/pin` hands the
//! entered PIN to the connection that is waiting for it.

use crate::state::Host;
use butterpollo_core::{
    state::Client,
    webrtc::{
        auth::{
            Challenge, ConnectionTable, Credentials, Dropped, MAX_PAIRED, PendingPin, PendingPins,
        },
        protocol::ErrorCode,
    },
};
use rand::rngs::OsRng;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::Mutex,
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, oneshot, watch};

/// Why a connection is told to go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Close {
    /// The same device connected again.
    Replaced,
    /// More than eight connections had not authenticated; this was the oldest.
    TooManyUnauthenticated,
}

/// What the console shows of the server.
#[derive(Debug, Clone, Default)]
pub struct Status {
    pub enabled: bool,
    pub signalling: Option<SocketAddr>,
    pub discovery: Option<SocketAddr>,
    pub error: Option<String>,
}

/// A registered connection: its number, and the channel that tells it to go.
pub struct Registered {
    pub id: u64,
    pub closed: mpsc::UnboundedReceiver<Close>,
}

struct Inner {
    table: ConnectionTable,
    pins: PendingPins,
    closers: HashMap<u64, mpsc::UnboundedSender<Close>>,
    next: u64,
}

pub struct Registry {
    inner: Mutex<Inner>,
    status: Mutex<Status>,
    changed: watch::Sender<u64>,
    epoch: Instant,
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

impl Registry {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                table: ConnectionTable::default(),
                pins: PendingPins::new(),
                closers: HashMap::new(),
                next: 1,
            }),
            status: Mutex::new(Status::default()),
            changed: watch::channel(0).0,
            epoch: Instant::now(),
        }
    }

    pub fn set_status(&self, status: Status) {
        *self.status.lock().unwrap() = status;
    }

    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }

    /// The `webrtc` object of `/api/config`.
    pub fn status_json(&self, config: &butterpollo_core::config::Config) -> Value {
        let status = self.status();
        json!({
            "enabled": status.enabled,
            "running": status.signalling.is_some(),
            "discovery": status.discovery.is_some(),
            "port": config.webrtc_port(),
            "media_port": config.webrtc_media_port(),
            "error": status.error,
        })
    }

    /// Time on the clock the pairing requests use.
    fn now(&self) -> Duration {
        self.epoch.elapsed()
    }

    /// The paired list changed (a device was disabled, unpaired or renamed):
    /// every connection checks its device again.
    pub fn clients_changed(&self) {
        self.changed.send_modify(|n| *n = n.wrapping_add(1));
    }

    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.changed.subscribe()
    }

    /// A new connection. The oldest unauthenticated ones beyond the limit are told to go.
    pub fn open(&self) -> Registered {
        let mut inner = self.inner.lock().unwrap();
        let id = inner.next;
        inner.next += 1;
        let (tx, closed) = mpsc::unbounded_channel();
        inner.closers.insert(id, tx);
        let dropped = inner.table.open(id);
        inner.tell(dropped, Close::TooManyUnauthenticated);
        Registered { id, closed }
    }

    /// The connection proved itself as `client_id`; its older connections go.
    pub fn authenticate(&self, id: u64, client_id: &str) -> Result<(), ErrorCode> {
        let mut inner = self.inner.lock().unwrap();
        let dropped = inner.table.authenticate(id, client_id)?;
        inner.tell(dropped, Close::Replaced);
        Ok(())
    }

    /// The connection ended.
    pub fn close(&self, id: u64) {
        let mut inner = self.inner.lock().unwrap();
        inner.table.close(id);
        inner.closers.remove(&id);
    }

    /// Connections held.
    #[cfg(test)]
    pub fn connections(&self) -> usize {
        self.inner.lock().unwrap().table.len()
    }

    /// The device's secret, if it is paired (a disabled device too: the proof
    /// is checked before its state is told).
    pub fn secret(&self, h: &Host, client_id: &str) -> Option<String> {
        h.paired
            .read()
            .unwrap()
            .webrtc_client(client_id)
            .and_then(|c| c.webrtc().map(|(_, secret)| secret.to_owned()))
    }

    /// The device if it is paired and enabled; otherwise why not.
    pub fn verify(&self, h: &Host, client_id: &str) -> Result<Client, ErrorCode> {
        let paired = h.paired.read().unwrap();
        let client = paired
            .webrtc_client(client_id)
            .ok_or(ErrorCode::AuthenticationFailed)?;
        if !client.enabled {
            return Err(ErrorCode::ClientDisabled);
        }
        Ok(client.clone())
    }

    /// A TV asked to pair. The request waits in the console's PIN list under
    /// the returned request ID until `/api/pin` sends the PIN the
    /// administrator typed, or it expires.
    pub fn begin_pairing(
        &self,
        h: &Host,
        pin: &str,
        name: &str,
    ) -> (String, oneshot::Receiver<(String, String)>) {
        let now = self.now();
        let request = PendingPin::new(&mut OsRng, pin, name, now);
        let id = request.request_id().to_owned();
        let (sender, receiver) = oneshot::channel();
        let mut inner = self.inner.lock().unwrap();
        inner.pins.prune(now);
        let mut pins = h.pins.lock().unwrap();
        if let Some(evicted) = inner.pins.add(request) {
            pins.remove(evicted.request_id());
        }
        pins.insert(
            id.clone(),
            crate::state::PendingPin {
                name: name.to_owned(),
                certificate: String::new(),
                created: Instant::now(),
                sender,
            },
        );
        (id, receiver)
    }

    /// The request is over without a pairing (the connection closed, or the
    /// TV asked again).
    pub fn cancel_pairing(&self, h: &Host, request_id: &str) {
        self.inner.lock().unwrap().pins.cancel(request_id);
        h.pins.lock().unwrap().remove(request_id);
    }

    /// The administrator entered `entered` for the request. The PIN is
    /// compared with the TV's in constant time, once; on a match the device is
    /// stored, named `name`, and its credentials returned.
    pub fn finish_pairing(
        &self,
        h: &Host,
        request_id: &str,
        entered: &str,
        name: &str,
    ) -> Result<Credentials, ErrorCode> {
        let approved = {
            let mut inner = self.inner.lock().unwrap();
            inner.pins.approve(request_id, entered, self.now())
        };
        let request = approved.map_err(|failure| failure.code())?;
        let credentials = Credentials::generate(&mut OsRng);
        let name = if name.trim().is_empty() {
            request.client_name()
        } else {
            name
        };
        let client = Client::paired_over_webrtc(
            name.to_owned(),
            &credentials.client_id,
            &credentials.secret,
        );
        let surplus = h
            .paired
            .write()
            .unwrap()
            .add_webrtc(&h.paired_path, client, MAX_PAIRED)
            .map_err(|error| {
                tracing::error!(error = %format!("{error:#}"), "a WebRTC device could not be saved");
                ErrorCode::PairingFailed
            })?;
        if !surplus.is_empty() {
            tracing::info!(
                count = surplus.len(),
                "the oldest WebRTC devices were unpaired to stay at 32"
            );
        }
        tracing::info!(client = name, "WebRTC device paired and saved");
        self.clients_changed();
        Ok(credentials)
    }

    /// A fresh challenge for a new connection.
    pub fn challenge(&self) -> Challenge {
        Challenge::new(&mut OsRng)
    }
}

impl Inner {
    fn tell(&mut self, dropped: Vec<Dropped>, reason: Close) {
        for Dropped { id, .. } in dropped {
            if let Some(closer) = self.closers.remove(&id) {
                let _ = closer.send(reason);
            }
        }
    }
}
