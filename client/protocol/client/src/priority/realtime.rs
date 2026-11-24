//! Linux D-Bus connections to xdg-desktop-portal and RealtimeKit for thread priority raises.

use std::sync::atomic::AtomicBool;
use std::sync::OnceLock;
use std::time::Duration;

use zbus::blocking::{connection::Builder, Connection};

use super::{Broker, RaisedBy, RealtimeService, ServiceEntry};

/// Per method call, so a portal that doesn't answer still leaves RealtimeKit time within
/// [`super::SERVICE_BUDGET`].
const TIMEOUT: Duration = Duration::from_millis(200);

/// The process's one broker; Err when its helper thread could not start.
pub(super) fn broker() -> Result<&'static Broker, Vec<String>> {
    static BROKER: OnceLock<Result<Broker, String>> = OnceLock::new();
    BROKER
        .get_or_init(|| Broker::new().map_err(|e| format!("D-Bus helper thread: {e}")))
        .as_ref()
        .map_err(|e| vec![e.clone()])
}

/// The portal, then RealtimeKit outside a Flatpak sandbox ([`super::services_asked`]), each
/// with its process-wide "gone" flag; opened lazily.
pub(super) fn services() -> impl Iterator<Item = ServiceEntry<'static>> {
    static PORTAL_GONE: AtomicBool = AtomicBool::new(false);
    static REALTIME_KIT_GONE: AtomicBool = AtomicBool::new(false);
    static SANDBOXED: OnceLock<bool> = OnceLock::new();
    // Flatpak puts this file at the root of every sandbox.
    let sandboxed = *SANDBOXED.get_or_init(|| std::path::Path::new("/.flatpak-info").exists());
    super::services_asked(sandboxed).iter().map(|&by| {
        let gone: &'static AtomicBool = match by {
            RaisedBy::RealtimeKit => &REALTIME_KIT_GONE,
            _ => &PORTAL_GONE,
        };
        (by, gone, service(by))
    })
}

struct Service {
    connection: Connection,
    destination: &'static str,
    path: &'static str,
    interface: &'static str,
    /// The portal's call takes the process id; RealtimeKit's takes it from the bus.
    with_pid: bool,
}

impl RealtimeService for Service {
    fn min_nice_level(&self) -> Result<i32, String> {
        let reply = self
            .connection
            .call_method(
                Some(self.destination),
                self.path,
                Some("org.freedesktop.DBus.Properties"),
                "Get",
                &(self.interface, "MinNiceLevel"),
            )
            .map_err(|e| e.to_string())?;
        let value: zbus::zvariant::OwnedValue =
            reply.body().deserialize().map_err(|e| e.to_string())?;
        i32::try_from(value).map_err(|e| e.to_string())
    }

    fn make_thread_high_priority(&self, tid: u64, nice: i32) -> Result<(), String> {
        // getpid and gettid as seen inside a Flatpak sandbox are what the portal takes: it
        // maps them to the host's ids itself, which SDL relies on too (SDL_threadprio.c).
        let result = if self.with_pid {
            let pid = u64::from(std::process::id());
            self.connection.call_method(
                Some(self.destination),
                self.path,
                Some(self.interface),
                "MakeThreadHighPriorityWithPID",
                &(pid, tid, nice),
            )
        } else {
            self.connection.call_method(
                Some(self.destination),
                self.path,
                Some(self.interface),
                "MakeThreadHighPriority",
                &(tid, nice),
            )
        };
        result.map(drop).map_err(|e| e.to_string())
    }
}

fn service(by: RaisedBy) -> Result<&'static dyn RealtimeService, String> {
    static PORTAL: OnceLock<Result<Service, String>> = OnceLock::new();
    static REALTIME_KIT: OnceLock<Result<Service, String>> = OnceLock::new();
    let service = match by {
        RaisedBy::Portal => PORTAL.get_or_init(|| {
            open(
                true,
                "org.freedesktop.portal.Desktop",
                "/org/freedesktop/portal/desktop",
                "org.freedesktop.portal.Realtime",
                true,
            )
        }),
        RaisedBy::RealtimeKit => REALTIME_KIT.get_or_init(|| {
            open(
                false,
                "org.freedesktop.RealtimeKit1",
                "/org/freedesktop/RealtimeKit1",
                "org.freedesktop.RealtimeKit1",
                false,
            )
        }),
        RaisedBy::Os => return Err("not a D-Bus service".into()),
    };
    match service {
        Ok(service) => Ok(service),
        Err(e) => Err(e.clone()),
    }
}

fn open(
    session: bool,
    destination: &'static str,
    path: &'static str,
    interface: &'static str,
    with_pid: bool,
) -> Result<Service, String> {
    let builder = if session {
        Builder::session()
    } else {
        Builder::system()
    };
    let connection = builder
        .and_then(|builder| builder.method_timeout(TIMEOUT).build())
        .map_err(|e| format!("{} bus: {e}", if session { "session" } else { "system" }))?;
    Ok(Service {
        connection,
        destination,
        path,
        interface,
        with_pid,
    })
}
