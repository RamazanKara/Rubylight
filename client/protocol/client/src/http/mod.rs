//! Host HTTP: serverinfo, PIN pairing, the app list and box art, launch, resume and cancel.
//!
//! Ported from moonlight-android's NvHTTP, PairingManager, NvApp and Nightjar's
//! HostCapabilities (GPL-3.0, Cameron Gutman and contributors, extended for Nightjar by
//! RamazanKara). Plain HTTP serves serverinfo and pairing; everything else is HTTPS with the
//! client certificate, trusting only the host certificate pinned at pairing. The API is
//! blocking; call it from a worker thread.

mod client_ops;
mod clipboard_image;
mod coop;
pub(crate) mod crypto;
pub(crate) mod der;
mod device_approval;
mod files;
mod folders;
pub mod foundation;
mod hardware_metrics;
pub(crate) mod identity;
mod internet_play;
mod launch;
pub mod launch_warning;
mod native;
mod network_path;
mod neural;
pub mod pace;
mod pairing;
mod pc_power;
mod power;
mod probe;
mod remote_check;
mod session_guests;
pub mod transfers;
pub(crate) mod transport;
mod upgrade;
mod usb;
pub(crate) mod wire;

#[cfg(test)]
mod fuzz_tests;

pub use clipboard_image::{
    ImageClipboardError, MAX_CLIPBOARD_IMAGE_BYTES, MAX_IMAGE_PIXELS, MAX_IMAGE_SIDE,
    PNG_SIGNATURE, png_size,
};
pub use coop::{
    Coop, CoopEnd, CoopEndReason, CoopGame, CoopJoined, CoopPhase, CoopPlayer, CoopProblem,
    CoopSession,
};
pub use device_approval::{
    FEATURE_PAIRING_APPROVE, PairingApprove, PairingApproved, PairingDeny, PairingRequest,
    PairingRequests, valid_pairing_pin,
};
pub use files::{FileError, FileList, FileRefusal, RemoteFile, TransferCancel, Uploaded};
pub use folders::{
    EntryKind, FolderAccess, FolderEntries, FolderEntry, MAX_FOLDER_DEPTH, PART_SUFFIX,
    SharedFolder, child_path, collision_name, file_name, has_part_suffix, parent_path,
};
pub use hardware_metrics::{
    CPU_METRICS, GPU_METRICS, GpuMetrics, HardwareMetrics, MAX_HARDWARE_METRICS_BYTES, Readings,
};
pub use identity::ClientIdentity;
pub use internet_play::{InternetPlay, MAX_INTERNET_PLAY_BYTES};
pub use launch_warning::LaunchWarning;
pub use launch::{
    CONFIRM_REPLACE, CORE_LAUNCH_PARAMETERS, HostDisplay, LaunchRequest, Launched,
    asks_to_confirm_replace, asks_to_confirm_terminate, is_role_completed, is_terminate_completed,
    parse_launch_reply, random_ri_key,
};
pub use native::{
    AddedGame, FilePermissions, GameExtra, MovedTo, NativeCapabilities, NativeLibrary, PlayingOn,
    StreamMove,
};
pub use network_path::{MAX_NETWORK_PATH_BYTES, NetworkPath};
pub use neural::{
    FRAME_GENERATION, Neural, NeuralAction, NeuralJob, NeuralMode, NeuralPhase, NeuralSwitch,
    NeuralWhy, NeuralWrite,
};
pub use pairing::{PairFailure, PairResult, PairStep};
pub use pc_power::{
    FEATURE_RESTART, FEATURE_SHUTDOWN, FEATURE_UPDATE, HostUpdate, LateRefusal,
    POWER_TOKEN_REFRESH, PcPower, PcPowerOffer, PowerAvailability, host_words, update_arrived,
};
pub use power::{PowerError, PowerReply, Refusal};
pub use probe::{
    JUMBO_PROBE_FPS, JUMBO_PROBE_KBPS, LINK_PROBE_BYTES, PROBE_FLOOR_KBPS, PROBE_START_SHARE,
    jumbo_probe_passes, jumbo_probe_size, probe_start_kbps,
};
pub use remote_check::{ConnectionTestStats, REMOTE_CHECK_PROBES, RemoteCheck, RemoteOutcome};
pub use session_guests::{
    FEATURE_GUEST_INVITES, FEATURE_SESSION_GUESTS, GUEST_CONTROLLER, GUEST_KEYBOARD_MOUSE,
    GUEST_PERMISSIONS, GUEST_VIEW, GUESTS_POLL_MS, GuestAction, GuestRequest, INVITED_JOINS_ONLY,
    InviteCreate, InviteCreated, SessionGuest, SessionGuests, guest_ended, join_refused,
};
pub use der::{
    certificate_sha256_der, certificate_sha256_pem, pem_decode_certificate, pem_encode_certificate,
};
pub use transport::{HostClientCanceller, set_app_version};
pub use usb::{
    TOKEN_HEADER as USB_TOKEN_HEADER, TUNNEL_PATH as USB_TUNNEL_PATH, UPGRADE as USB_UPGRADE,
    UsbTunnel,
};

/// Re-exported pure HTTP DTOs and parsers from `nightjar_policy::http`.
pub use nightjar_policy::http::{
    App, AppControl, AppRole, Availability, DEFAULT_HTTP_PORT, DEFAULT_HTTPS_PORT,
    DisplayLuminance, Error, Evidence, Family, Feature, HostApiError, HostCapabilities,
    LaunchUnanswered, PERMISSION_CLIPBOARD_READ, PERMISSION_CLIPBOARD_SET, PERMISSION_CONTROLLER,
    PERMISSION_INPUT, PERMISSION_LAUNCH, PERMISSION_LIST, PERMISSION_SERVER_COMMAND,
    PERMISSION_VIEW, PYROWAVE_CODEC_BITS, Reason, SECOND_SCREEN_PARAMETER, ServerInfo, app_control,
    parse_app_list, parse_rubylight_version, xml,
};
pub use rubylight_native_api as native_api;

pub(crate) mod capabilities {
    #[cfg(test)]
    pub use nightjar_policy::http::{Availability, Evidence, Reason, parse_rubylight_version};
    pub use nightjar_policy::http::{
        Family, Feature, HostCapabilities, PYROWAVE_BANDWIDTH_PROBE_BYTES,
    };
}
pub(crate) mod apps {
    pub use nightjar_policy::http::AppRole;
    #[cfg(test)]
    pub use nightjar_policy::http::parse_app_list;
}
pub(crate) mod luminance {
    pub use nightjar_policy::http::DisplayLuminance;
}
pub(crate) mod serverinfo {
    #[cfg(test)]
    pub use nightjar_policy::http::{ServerInfo, parse_version_quad};
    #[cfg(test)]
    pub(crate) mod tests {
        pub(crate) const RUBYLIGHT: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?><root status_code=\"200\"><NetworkPathVersion>1</NetworkPathVersion><HardwareMetricsVersion>0</HardwareMetricsVersion><hostname>Gaming PC</hostname><appversion>7.1.431.-1</appversion><GfeVersion>3.23.0.74</GfeVersion><uniqueid>0123-ABCD</uniqueid><HttpsPort>47984</HttpsPort><ExternalPort>47989</ExternalPort><PairStatus>1</PairStatus><currentgame>5</currentgame><currentgameuuid>uuid-5</currentgameuuid><state>SUNSHINE_SERVER_BUSY</state><LocalIP>192.168.1.20</LocalIP><mac>AA:BB:CC:DD:EE:FF</mac><MaxLumaPixelsHEVC>1869449984</MaxLumaPixelsHEVC><ServerCodecModeSupport>8457985</ServerCodecModeSupport><RustHostVersion>2.2.21</RustHostVersion><RustHostEncoderBitrateKbps>40000</RustHostEncoderBitrateKbps><RustHostInitialEncoderBitrateKbps>50000</RustHostInitialEncoderBitrateKbps><RustHostSessionCount></RustHostSessionCount><PyroWaveHostLinkMbps>2500</PyroWaveHostLinkMbps><PyroWaveBandwidthProbeBytes>33554432</PyroWaveBandwidthProbeBytes><PyroWaveUdpProbeVersion>1</PyroWaveUdpProbeVersion><PyroWaveUdpHandshakeVersion>1</PyroWaveUdpHandshakeVersion><PyroWaveUdpProbeBurstVersion>1</PyroWaveUdpProbeBurstVersion><PyroWaveWireBudgetVersion>1</PyroWaveWireBudgetVersion><PyroWaveCriticalFecPercentage>20</PyroWaveCriticalFecPercentage><PyroWaveMinParityShards>2</PyroWaveMinParityShards><Permission>119480064</Permission><FrameLimiterSupported>1</FrameLimiterSupported><FrameLimiterEnabled>1</FrameLimiterEnabled><VirtualDisplayFrameLimiterEnabled>0</VirtualDisplayFrameLimiterEnabled><FrameLimiterFpsLimitMilliHz>119880</FrameLimiterFpsLimitMilliHz><VirtualDisplayCapable>true</VirtualDisplayCapable><VirtualDisplayDriverReady>false</VirtualDisplayDriverReady><ControllerPreparation>1</ControllerPreparation><VirtualDisplayHDRCapable>true</VirtualDisplayHDRCapable><ServerCommand>Sleep</ServerCommand><ServerCommand></ServerCommand></root>";
    }
}

use std::sync::{Arc, Mutex};
use std::time::Duration;
use transport::{Cancel, Host, Request, Timeouts};

const SHORT_CONNECT_TIMEOUT: Duration = Duration::from_millis(3000);
const LONG_CONNECT_TIMEOUT: Duration = Duration::from_millis(5000);
const READ_TIMEOUT: Duration = Duration::from_millis(7000);
/// The longest a PC may take to answer a launch or resume: it prepares the session first (its
/// preparation commands, a display, the game), which can take a while, but a PC that never
/// answers must not hold the start for good. Android's `NvHTTP.LAUNCH_READ_TIMEOUT_SECONDS`.
pub const LAUNCH_READ_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_XML_RESPONSE_BYTES: usize = 256 * 1024;
const MAX_APP_LIST_BYTES: usize = 4 * 1024 * 1024;
const MAX_BOX_ART_BYTES: usize = 16 * 1024 * 1024;
const MAX_CLIPBOARD_BYTES: usize = 1024 * 1024;

/// The SHA-256 of a stored host certificate's DER: what a QR pairing link's `fp` names.
pub fn certificate_sha256(pem: &str) -> Result<[u8; 32], Error> {
    let mut hash = [0; 32];
    hash.copy_from_slice(&crypto::sha256(&der::pem_decode(pem, "CERTIFICATE")?));
    Ok(hash)
}

/// The `box.png` hosts send from `/appasset` for an app with no cover of its own, by length and SHA-256:
/// Rubylight's and Sunshine-family hosts' since Sunshine 0.21, and Sunshine 0.20 and older. Android's
/// `PlaceholderArt` knows the same two.
const PLACEHOLDER_BOX_ART: [(usize, &str); 2] = [
    (
        680,
        "d9164ebd069b5f735eb8efc557801778498da37f572ef70e3d35604739e6c613",
    ),
    (
        998,
        "f5992b68a72ef6f74c6e4970d0eb081fd7c51e8b5556c6a3f87dc414fda0473b",
    ),
];

/// True when box art is a host's grey placeholder rather than the app's own cover: hosts answer
/// `/appasset` with it instead of a 404, and the app's title stands in for it.
pub fn is_placeholder_box_art(bytes: &[u8]) -> bool {
    PLACEHOLDER_BOX_ART.iter().any(|(len, sha256)| {
        bytes.len() == *len
            && ring::digest::digest(&ring::digest::SHA256, bytes)
                .as_ref()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
                == *sha256
    })
}

/// How a serverinfo answer was reached ([`HostClient::server_info_checked`]), which says what
/// its `PairStatus` is worth: hosts report 0 over plain HTTP whatever they hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InfoPath {
    /// Over HTTPS from the pinned certificate: `PairStatus` is the host's word.
    Pinned,
    /// The pinned certificate's host refused this client (401) and answered over HTTP: it no
    /// longer knows this client.
    Refused,
    /// Another certificate was presented at this address and the client fell back to HTTP
    /// (`server_info_with_cert_check`); `PairStatus` says nothing about our pairing.
    CertificateMismatch,
    /// Over plain HTTP with nothing pinned: `PairStatus` says nothing of a pairing.
    Plain,
}

/// A launch's failure after `waited`: a timeout that took the whole `limit` is the PC's missing
/// answer; a connect that timed out early stays the usual connection failure.
pub(super) fn launch_failure(error: Error, waited: Duration, limit: Duration) -> Error {
    match error {
        Error::Io(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
            ) && waited >= limit =>
        {
            Error::Io(std::io::Error::other(LaunchUnanswered {
                seconds: limit.as_secs().max(1),
            }))
        }
        other => other,
    }
}

#[derive(Default)]
struct State {
    https_port: u16,
    server_certificate: Option<Vec<u8>>,
    tls: Option<Arc<rustls::ClientConfig>>,
}

/// One host, reached at one address. Methods block; the client may be shared across threads.
pub struct HostClient {
    pub(super) host: Host,
    pub(super) http_port: u16,
    identity: Option<Arc<ClientIdentity>>,
    unique_id: String,
    device_name: String,
    second_screen: bool,
    state: Mutex<State>,
    pub(super) cancel: Arc<Cancel>,
}

#[derive(Clone, Copy)]
pub(super) enum Scheme {
    Http,
    Https,
}

impl HostClient {
    /// `address` is an IP literal (IPv6 may carry a numeric scope) or a host name.
    pub fn new(
        address: &str,
        http_port: u16,
        identity: Arc<ClientIdentity>,
    ) -> Result<Self, Error> {
        let unique_id = identity.unique_id().to_owned();
        Ok(Self {
            host: Host::parse(address)?,
            http_port,
            identity: Some(identity),
            unique_id,
            device_name: "Nightjar".into(),
            second_screen: false,
            state: Mutex::new(State::default()),
            cancel: Arc::new(Cancel::default()),
        })
    }

    /// An unauthenticated client (no client certificate/key) for HTTP-only calls or tests.
    pub fn new_unauthenticated(
        address: &str,
        http_port: u16,
        unique_id: impl Into<String>,
    ) -> Result<Self, Error> {
        Ok(Self {
            host: Host::parse(address)?,
            http_port,
            identity: None,
            unique_id: unique_id.into(),
            device_name: "Nightjar".into(),
            second_screen: false,
            state: Mutex::new(State::default()),
            cancel: Arc::new(Cancel::default()),
        })
    }

    /// The name the host shows for this client while pairing.
    pub fn with_device_name(mut self, name: impl Into<String>) -> Self {
        self.device_name = name.into();
        self
    }

    /// Every request is the second screen's, with [`SECOND_SCREEN_PARAMETER`]: launch, resume,
    /// bitrate, the PC's bitrate controller, metrics and cancel alike.
    pub fn with_second_screen(mut self) -> Self {
        self.second_screen = true;
        self
    }

    /// Enables or disables the second-screen flag on an existing client instance.
    pub fn set_second_screen(&mut self, second_screen: bool) {
        self.second_screen = second_screen;
    }

    /// The HTTPS port from an earlier serverinfo, to skip asking for it.
    pub fn with_https_port(self, port: u16) -> Self {
        self.set_https_port(port);
        self
    }

    /// Updates the cached HTTPS port in-place (0 forces rediscovery via HTTP `serverinfo`).
    pub fn set_https_port(&self, port: u16) {
        self.state.lock().unwrap().https_port = port;
    }

    /// Currently cached HTTPS port (0 if unknown).
    pub fn https_port_cached(&self) -> u16 {
        self.state.lock().unwrap().https_port
    }

    /// The host certificate stored at pairing (PEM).
    pub fn with_server_certificate_pem(self, pem: &str) -> Result<Self, Error> {
        let der = der::pem_decode(pem, "CERTIFICATE")?;
        self.pin(Some(der));
        Ok(self)
    }

    /// Pins (or clears, with `None`) the host certificate DER bytes in-place.
    pub fn pin_server_certificate_der(&self, der: Option<Vec<u8>>) {
        self.pin(der);
    }

    /// The pinned host certificate DER bytes, if any.
    pub fn server_certificate_der(&self) -> Option<Vec<u8>> {
        self.state.lock().unwrap().server_certificate.clone()
    }

    /// The pinned host certificate as PEM, if any.
    pub fn server_certificate_pem(&self) -> Option<String> {
        let state = self.state.lock().unwrap();
        state
            .server_certificate
            .as_deref()
            .map(|d| der::pem_encode(d, "CERTIFICATE"))
    }

    pub fn identity(&self) -> Option<&ClientIdentity> {
        self.identity.as_deref()
    }

    /// Aborts requests in flight, such as a pairing waiting for its PIN, and every later one.
    pub fn cancel_pending_requests(&self) {
        self.cancel.cancel();
    }

    /// Whether [`HostClient::cancel_pending_requests`] has been called on this client.
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// Returns a cloneable cancellation handle that can abort requests from another thread.
    pub fn canceller(&self) -> HostClientCanceller {
        HostClientCanceller(Arc::clone(&self.cancel))
    }

    /// The same host and pairing, untouched by this client's [`HostClient::cancel_pending_requests`]:
    /// for a request that must still go after the stream ends, such as turning a PC's own bitrate
    /// controller off again.
    pub fn detached(&self) -> Self {
        let state = self.state.lock().unwrap();
        Self {
            host: self.host.clone(),
            http_port: self.http_port,
            identity: self.identity.clone(),
            unique_id: self.unique_id.clone(),
            device_name: self.device_name.clone(),
            second_screen: self.second_screen,
            state: Mutex::new(State {
                https_port: state.https_port,
                server_certificate: state.server_certificate.clone(),
                tls: state.tls.clone(),
            }),
            cancel: Arc::new(Cancel::default()),
        }
    }

    fn pin(&self, der: Option<Vec<u8>>) {
        let mut state = self.state.lock().unwrap();
        state.server_certificate = der;
        state.tls = None;
    }

    fn url_target(&self, path: &str, query: Option<&str>) -> String {
        let mut target = format!("{path}?");
        if let Some(query) = query {
            target.push_str(query);
            target.push('&');
        }
        target.push_str(&format!(
            "uniqueid={}&uuid={}",
            transport::encode_query(&self.unique_id),
            transport::random_uuid()
        ));
        if self.second_screen {
            target.push_str(&format!("&{SECOND_SCREEN_PARAMETER}=1"));
        }
        if path == "pair" {
            target.push_str(&format!(
                "&devicename={}",
                transport::encode_query(&self.device_name)
            ));
        }
        target
    }

    fn tls(&self) -> Result<Arc<rustls::ClientConfig>, Error> {
        let mut state = self.state.lock().unwrap();
        if let Some(tls) = &state.tls {
            return Ok(tls.clone());
        }
        let server = state
            .server_certificate
            .as_deref()
            .ok_or(Error::NotPaired)?;
        let identity = self
            .identity
            .as_deref()
            .ok_or_else(|| Error::InvalidIdentity("missing client certificate".into()))?;
        let tls = transport::tls_config(
            server,
            identity.certificate_der(),
            identity.private_key_pkcs8(),
        )?;
        state.tls = Some(tls.clone());
        Ok(tls)
    }

    /// The HTTPS port, asking serverinfo over HTTP the first time.
    fn https_port(&self, likely_online: bool) -> Result<u16, Error> {
        let known = self.state.lock().unwrap().https_port;
        if known != 0 {
            return Ok(known);
        }
        let connect = if likely_online {
            LONG_CONNECT_TIMEOUT
        } else {
            SHORT_CONNECT_TIMEOUT
        };
        let raw = self.request_text(
            Scheme::Http,
            "serverinfo",
            None,
            connect,
            Some(READ_TIMEOUT),
        )?;
        let port = ServerInfo::parse(&raw, self.http_port)?.https_port;
        self.state.lock().unwrap().https_port = port;
        Ok(port)
    }

    fn request(
        &self,
        scheme: Scheme,
        path: &str,
        query: Option<&str>,
        connect: Duration,
        read: Option<Duration>,
        body_limit: usize,
    ) -> Result<Vec<u8>, Error> {
        let (port, tls) = match scheme {
            Scheme::Http => (self.http_port, None),
            Scheme::Https => (self.https_port(true)?, Some(self.tls()?)),
        };
        let target = self.url_target(path, query);
        let timeouts = Timeouts { connect, read };
        #[cfg(test)]
        tests::SENT.with_borrow_mut(|sent| sent.push((path.to_owned(), timeouts)));
        let response = transport::get(
            &Request {
                host: &self.host,
                port,
                target: &target,
                tls,
                timeouts,
                body_limit,
            },
            &self.cancel,
        )?;
        match response.status {
            200..=299 => Ok(response.body),
            404 => Err(Error::NotFound(format!("/{path}"))),
            code => Err(Error::Status {
                code: code as i32,
                message: response.reason,
            }),
        }
    }

    pub(super) fn request_text(
        &self,
        scheme: Scheme,
        path: &str,
        query: Option<&str>,
        connect: Duration,
        read: Option<Duration>,
    ) -> Result<String, Error> {
        let limit = if path == "applist" {
            MAX_APP_LIST_BYTES
        } else {
            MAX_XML_RESPONSE_BYTES
        };
        let body = self.request(scheme, path, query, connect, read, limit)?;
        String::from_utf8(body).map_err(|_| Error::Malformed("host reply is not UTF-8".into()))
    }
}

#[cfg(test)]
mod tests;
