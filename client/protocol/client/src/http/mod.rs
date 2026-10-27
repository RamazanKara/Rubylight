//! Host HTTP: serverinfo, PIN pairing, the app list and box art, launch, resume and cancel.
//!
//! Ported from moonlight-android's NvHTTP, PairingManager, NvApp and Nightjar's
//! HostCapabilities (GPL-3.0, Cameron Gutman and contributors, extended for Nightjar by
//! RamazanKara). Plain HTTP serves serverinfo and pairing; everything else is HTTPS with the
//! client certificate, trusting only the host certificate pinned at pairing. The API is
//! blocking; call it from a worker thread.

mod apps;
mod capabilities;
mod clipboard_image;
mod crypto;
mod der;
mod files;
mod folders;
mod hardware_metrics;
mod identity;
mod launch;
mod native;
mod network_path;
mod pairing;
pub mod pace;
mod power;
mod probe;
mod remote_check;
mod serverinfo;
mod transport;
pub mod transfers;
mod usb;
mod xml;

pub use apps::{app_control, parse_app_list, App, AppControl, AppRole};
pub use capabilities::{
    parse_rubylight_version, Availability, Evidence, Family, Feature, HostCapabilities, Reason,
    PERMISSION_CLIPBOARD_READ, PERMISSION_CLIPBOARD_SET, PERMISSION_CONTROLLER, PERMISSION_INPUT,
    PERMISSION_LAUNCH, PERMISSION_LIST, PERMISSION_SERVER_COMMAND, PERMISSION_VIEW,
    PYROWAVE_CODEC_BITS,
};
pub use clipboard_image::{
    png_size, ImageClipboardError, MAX_CLIPBOARD_IMAGE_BYTES, MAX_IMAGE_PIXELS, MAX_IMAGE_SIDE,
    PNG_SIGNATURE,
};
pub use files::{FileError, FileList, FileRefusal, RemoteFile, TransferCancel, Uploaded};
pub use folders::{
    child_path, collision_name, file_name, has_part_suffix, parent_path, EntryKind, FolderAccess,
    FolderEntries, FolderEntry, SharedFolder, MAX_FOLDER_DEPTH, PART_SUFFIX,
};
pub use hardware_metrics::{
    GpuMetrics, HardwareMetrics, Readings, CPU_METRICS, GPU_METRICS, MAX_HARDWARE_METRICS_BYTES,
};
pub use identity::ClientIdentity;
pub use launch::{
    parse_launch_reply, HostDisplay, LaunchRequest, Launched, CORE_LAUNCH_PARAMETERS,
};
pub use native::{FilePermissions, NativeCapabilities};
pub use network_path::{NetworkPath, MAX_NETWORK_PATH_BYTES};
pub use pairing::{PairFailure, PairResult, PairStep};
pub use power::{PowerError, PowerReply, Refusal};
pub use probe::{probe_start_kbps, LINK_PROBE_BYTES, PROBE_FLOOR_KBPS, PROBE_START_SHARE};
pub use remote_check::{ConnectionTestStats, RemoteCheck, RemoteOutcome, REMOTE_CHECK_PROBES};
pub use serverinfo::ServerInfo;
pub use transport::set_app_version;
pub use usb::{
    UsbTunnel, TOKEN_HEADER as USB_TOKEN_HEADER, TUNNEL_PATH as USB_TUNNEL_PATH,
    UPGRADE as USB_UPGRADE,
};

/// The Rubylight native API v1 wire types, for callers of the `HostClient` native methods.
pub use rubylight_native_api as native_api;

use std::sync::{Arc, Mutex};
use std::time::Duration;
use transport::{Cancel, Host, Request, Timeouts};

pub const DEFAULT_HTTP_PORT: u16 = 47989;
pub const DEFAULT_HTTPS_PORT: u16 = 47984;
const SHORT_CONNECT_TIMEOUT: Duration = Duration::from_millis(3000);
const LONG_CONNECT_TIMEOUT: Duration = Duration::from_millis(5000);
const READ_TIMEOUT: Duration = Duration::from_millis(7000);
const MAX_XML_RESPONSE_BYTES: usize = 256 * 1024;
const MAX_APP_LIST_BYTES: usize = 4 * 1024 * 1024;
const MAX_BOX_ART_BYTES: usize = 16 * 1024 * 1024;
const MAX_CLIPBOARD_BYTES: usize = 1024 * 1024;

/// Why a host request failed.
#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Tls(String),
    /// The host presented a certificate other than the pinned one.
    CertificateMismatch,
    /// An HTTP status other than 2xx, or a reply whose `status_code` is not 200. Hosts use
    /// 401 for unpaired clients; 599 is synthesized when a cancel left another client's game.
    Status {
        code: i32,
        message: String,
    },
    /// HTTP 404.
    NotFound(String),
    /// A non-2xx answer from the native API, with the host's error body. `status` is the HTTP
    /// status, which an `ErrorCode::Unknown` from a newer host is handled by.
    Api {
        status: u16,
        error: native_api::error::ApiError,
    },
    Malformed(String),
    /// HTTPS needs the host certificate from pairing.
    NotPaired,
    InvalidIdentity(String),
    InvalidArgument(String),
    /// Ended by [`HostClient::cancel_pending_requests`].
    Cancelled,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::Tls(e) => write!(f, "TLS: {e}"),
            Error::CertificateMismatch => f.write_str("Server certificate mismatch"),
            Error::Status { code, message } => write!(f, "host error {code}: {message}"),
            Error::NotFound(url) => write!(f, "not found: {url}"),
            Error::Api { error, .. } => f.write_str(&error.message),
            Error::Malformed(e) => f.write_str(e),
            Error::NotPaired => f.write_str("the host is not paired"),
            Error::InvalidIdentity(e) => write!(f, "invalid client identity: {e}"),
            Error::InvalidArgument(e) => f.write_str(e),
            Error::Cancelled => f.write_str("request cancelled"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

/// Why an optional host API (hardware metrics, network path) gave no answer.
#[derive(Debug)]
pub enum HostApiError {
    /// The host does not offer the feature to this client, so nothing was sent;
    /// `availability` says what would change that.
    Unavailable {
        feature: Feature,
        availability: Availability,
    },
    /// The host answered with a schema version this client does not read.
    UnsupportedVersion,
    Host(Error),
}

impl HostApiError {
    fn require(capabilities: &HostCapabilities, feature: Feature) -> Result<(), Self> {
        match capabilities.availability(feature) {
            Availability::Supported => Ok(()),
            availability => Err(HostApiError::Unavailable {
                feature,
                availability,
            }),
        }
    }
}

impl std::fmt::Display for HostApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HostApiError::Unavailable { feature, .. } => {
                write!(f, "the host does not offer {feature:?}")
            }
            HostApiError::UnsupportedVersion => f.write_str("unsupported host API version"),
            HostApiError::Host(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for HostApiError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            HostApiError::Host(e) => Some(e),
            _ => None,
        }
    }
}

impl From<Error> for HostApiError {
    fn from(e: Error) -> Self {
        HostApiError::Host(e)
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
    host: Host,
    http_port: u16,
    identity: Arc<ClientIdentity>,
    device_name: String,
    state: Mutex<State>,
    cancel: Cancel,
}

#[derive(Clone, Copy)]
enum Scheme {
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
        Ok(Self {
            host: Host::parse(address)?,
            http_port,
            identity,
            device_name: "Nightjar".into(),
            state: Mutex::new(State::default()),
            cancel: Cancel::default(),
        })
    }

    /// The name the host shows for this client while pairing.
    pub fn with_device_name(mut self, name: impl Into<String>) -> Self {
        self.device_name = name.into();
        self
    }

    /// The HTTPS port from an earlier serverinfo, to skip asking for it.
    pub fn with_https_port(self, port: u16) -> Self {
        self.state.lock().unwrap().https_port = port;
        self
    }

    /// The host certificate stored at pairing (PEM).
    pub fn with_server_certificate_pem(self, pem: &str) -> Result<Self, Error> {
        let der = der::pem_decode(pem, "CERTIFICATE")?;
        self.pin(Some(der));
        Ok(self)
    }

    /// The pinned host certificate as PEM, if any.
    pub fn server_certificate_pem(&self) -> Option<String> {
        let state = self.state.lock().unwrap();
        state
            .server_certificate
            .as_deref()
            .map(|d| der::pem_encode(d, "CERTIFICATE"))
    }

    pub fn identity(&self) -> &ClientIdentity {
        &self.identity
    }

    /// Aborts requests in flight, such as a pairing waiting for its PIN, and every later one.
    pub fn cancel_pending_requests(&self) {
        self.cancel.cancel();
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
            transport::encode_query(self.identity.unique_id()),
            transport::random_uuid()
        ));
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
        let tls = transport::tls_config(
            server,
            self.identity.certificate_der(),
            self.identity.private_key_pkcs8(),
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
        let response = transport::get(
            &Request {
                host: &self.host,
                port,
                target: &target,
                tls,
                timeouts: Timeouts { connect, read },
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

    fn request_text(
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

    /// Serverinfo: over pinned HTTPS when paired, falling back to HTTP when the host does not
    /// know this client (401) or shows another certificate, so the client can pair again.
    pub fn server_info(&self, likely_online: bool) -> Result<ServerInfo, Error> {
        let connect = if likely_online {
            LONG_CONNECT_TIMEOUT
        } else {
            SHORT_CONNECT_TIMEOUT
        };
        let paired = self.state.lock().unwrap().server_certificate.is_some();
        if paired {
            let https = self
                .request_text(
                    Scheme::Https,
                    "serverinfo",
                    None,
                    connect,
                    Some(READ_TIMEOUT),
                )
                .and_then(|raw| ServerInfo::parse(&raw, self.http_port));
            match https {
                Ok(info) => return Ok(info),
                Err(Error::CertificateMismatch) | Err(Error::Status { code: 401, .. }) => {}
                Err(e) => return Err(e),
            }
        }
        self.server_info_http(likely_online)
    }

    /// Serverinfo over plain HTTP; hosts report `PairStatus` 0 there.
    pub fn server_info_http(&self, likely_online: bool) -> Result<ServerInfo, Error> {
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
        let info = ServerInfo::parse(&raw, self.http_port)?;
        let mut state = self.state.lock().unwrap();
        if state.https_port == 0 {
            state.https_port = info.https_port;
        }
        Ok(info)
    }

    /// Serverinfo over pinned HTTPS only, without fallback (verifies the paired host).
    pub fn server_info_https(&self) -> Result<ServerInfo, Error> {
        let raw = self.request_text(
            Scheme::Https,
            "serverinfo",
            None,
            SHORT_CONNECT_TIMEOUT,
            Some(READ_TIMEOUT),
        )?;
        ServerInfo::parse(&raw, self.http_port)
    }

    /// Pairs with the PIN the user types on the host. Blocks until the host answers, up to
    /// minutes; [`HostClient::cancel_pending_requests`] ends the wait. On success the host
    /// certificate is pinned; store it with the identity.
    pub fn pair(&self, server_info: &ServerInfo, pin: &str) -> Result<PairResult, Error> {
        self.pair_with(server_info, pin, None)
    }

    /// One-time PIN pairing (Apollo, Vibepollo, Rubylight): the host already knows the PIN
    /// and passphrase, so no one types anything.
    pub fn pair_with_passphrase(
        &self,
        server_info: &ServerInfo,
        pin: &str,
        passphrase: &str,
    ) -> Result<PairResult, Error> {
        self.pair_with(server_info, pin, Some(passphrase))
    }

    /// Pairs with a host's QR pairing link (Experimental): one-time PIN pairing, refused with
    /// [`Error::CertificateMismatch`] before anything is sent back when the host's certificate
    /// does not hash to the link's fingerprint. The client must already point at the link's
    /// host and port.
    pub fn pair_with_link(
        &self,
        server_info: &ServerInfo,
        link: &rubylight_protocol::pairing_uri::PairingUri,
    ) -> Result<PairResult, Error> {
        self.pair_checked(
            server_info,
            &link.otp,
            Some(&link.passphrase),
            Some(link.cert_sha256),
        )
    }

    fn pair_with(
        &self,
        server_info: &ServerInfo,
        pin: &str,
        passphrase: Option<&str>,
    ) -> Result<PairResult, Error> {
        self.pair_checked(server_info, pin, passphrase, None)
    }

    fn pair_checked(
        &self,
        server_info: &ServerInfo,
        pin: &str,
        passphrase: Option<&str>,
        fingerprint: Option<[u8; 32]>,
    ) -> Result<PairResult, Error> {
        let major = server_info.major_version()?;
        let previous = {
            let mut state = self.state.lock().unwrap();
            if state.https_port == 0 {
                state.https_port = server_info.https_port;
            }
            state.server_certificate.clone()
        };
        // A failed attempt keeps the previous pin (the Java client left the new one set).
        let mut transport = PairingClient {
            client: self,
            fingerprint,
        };
        let result = pairing::pair(&mut transport, &self.identity, major, pin, passphrase);
        if !matches!(result, Ok(PairResult::Paired { .. })) {
            self.pin(previous);
        }
        result
    }

    /// Unpairs this client; over HTTPS when paired, else HTTP.
    pub fn unpair(&self) -> Result<(), Error> {
        let scheme = if self.state.lock().unwrap().server_certificate.is_some() {
            Scheme::Https
        } else {
            Scheme::Http
        };
        self.request_text(
            scheme,
            "unpair",
            None,
            LONG_CONNECT_TIMEOUT,
            Some(READ_TIMEOUT),
        )?;
        Ok(())
    }

    /// The app list; `capabilities` (from serverinfo) decides which entries are control tiles.
    pub fn app_list(&self, capabilities: &HostCapabilities) -> Result<Vec<App>, Error> {
        let raw = self.app_list_raw()?;
        parse_app_list(&raw, capabilities)
    }

    /// The app list reply as received, for callers that cache it.
    pub fn app_list_raw(&self) -> Result<String, Error> {
        self.request_text(
            Scheme::Https,
            "applist",
            None,
            LONG_CONNECT_TIMEOUT,
            Some(READ_TIMEOUT),
        )
    }

    fn box_art_query(app_id: i32) -> String {
        format!("appid={app_id}&AssetType=2&AssetIdx=0")
    }

    /// The box art image (PNG or JPEG bytes as the host sends them).
    pub fn box_art(&self, app_id: i32) -> Result<Vec<u8>, Error> {
        self.request(
            Scheme::Https,
            "appasset",
            Some(&Self::box_art_query(app_id)),
            LONG_CONNECT_TIMEOUT,
            Some(READ_TIMEOUT),
            MAX_BOX_ART_BYTES,
        )
    }

    /// The box art URL; fetching it needs this client's certificate and the pinned host's.
    pub fn box_art_url(&self, app_id: i32) -> Result<String, Error> {
        let port = self.https_port(true)?;
        Ok(format!(
            "https://{}:{port}/{}",
            self.host.url_host(),
            self.url_target("appasset", Some(&Self::box_art_query(app_id)))
        ))
    }

    /// Starts an app. Waits without a read timeout while the host prepares the session.
    /// None when the host declined without an error status.
    pub fn launch(
        &self,
        request: &LaunchRequest,
        server_info: &ServerInfo,
    ) -> Result<Option<Launched>, Error> {
        self.start("launch", request, server_info)
    }

    /// Rejoins the running app.
    pub fn resume(
        &self,
        request: &LaunchRequest,
        server_info: &ServerInfo,
    ) -> Result<Option<Launched>, Error> {
        self.start("resume", request, server_info)
    }

    fn start(
        &self,
        verb: &str,
        request: &LaunchRequest,
        server_info: &ServerInfo,
    ) -> Result<Option<Launched>, Error> {
        let query = request.query(&server_info.capabilities, server_info.nvidia_server());
        let raw = self.request_text(
            Scheme::Https,
            verb,
            Some(&query),
            LONG_CONNECT_TIMEOUT,
            None,
        )?;
        parse_launch_reply(&raw, verb == "resume")
    }

    /// Quits the running app. Ok(false) when the host declined; a 599 status when the host
    /// still runs a game afterwards (it belongs to another client).
    pub fn cancel(&self) -> Result<bool, Error> {
        let raw = self.request_text(Scheme::Https, "cancel", None, LONG_CONNECT_TIMEOUT, None)?;
        if xml::parse_reply(&raw)?.required_text("cancel")? == "0" {
            return Ok(false);
        }
        if self.server_info(true)?.current_game != 0 {
            return Err(Error::Status {
                code: 599,
                message: String::new(),
            });
        }
        Ok(true)
    }

    fn clipboard_request(&self, text: Option<&str>) -> Result<transport::Response, Error> {
        if text.is_some_and(|text| text.len() > MAX_CLIPBOARD_BYTES) {
            return Err(Error::InvalidArgument("Clipboard text exceeds 1 MiB".into()));
        }
        let response = transport::send(
            &Request {
                host: &self.host,
                port: self.https_port(true)?,
                target: &self.url_target("actions/clipboard", Some("type=text")),
                tls: Some(self.tls()?),
                timeouts: Timeouts { connect: LONG_CONNECT_TIMEOUT, read: Some(READ_TIMEOUT) },
                body_limit: MAX_CLIPBOARD_BYTES,
            },
            &self.cancel,
            text.map(|text| transport::Body::text(text.as_bytes())),
        )?;
        if !(200..=299).contains(&response.status) {
            return Err(Error::Status { code: i32::from(response.status), message: response.reason });
        }
        Ok(response)
    }

    pub fn clipboard(&self) -> Result<String, Error> {
        let response = self.clipboard_request(None)?;
        if !response.content_type.as_deref().is_some_and(|mime| {
            mime.split(';').next().unwrap_or_default().trim().eq_ignore_ascii_case("text/plain")
        }) {
            return Err(Error::Malformed("The PC does not support text clipboard transfer".into()));
        }
        String::from_utf8(response.body).map_err(|_| Error::Malformed("Clipboard text is not UTF-8".into()))
    }

    pub fn send_clipboard(&self, text: &str) -> Result<(), Error> {
        if !self.clipboard_request(Some(text))?.body.is_empty() {
            return Err(Error::Malformed("The PC did not accept the clipboard text".into()));
        }
        Ok(())
    }

    pub fn set_bitrate(&self, kbps: u32) -> Result<u32, Error> {
        if !(1..=2_000_000).contains(&kbps) {
            return Err(Error::InvalidArgument("Bitrate must be between 1 and 2000000 kbps".into()));
        }
        let raw = self.request_text(
            Scheme::Https, "bitrate", Some(&format!("bitrate={kbps}")),
            LONG_CONNECT_TIMEOUT, Some(READ_TIMEOUT),
        )?;
        xml::parse_reply(&raw)?.required_text("bitrate")?.parse::<u32>().ok()
            .filter(|applied| (1..=2_000_000).contains(applied))
            .ok_or_else(|| Error::Malformed("The PC did not apply the bitrate".into()))
    }
}

struct PairingClient<'a> {
    client: &'a HostClient,
    /// The SHA-256 the host certificate must have (a QR pairing link's `fp`).
    fingerprint: Option<[u8; 32]>,
}

impl pairing::PairingTransport for PairingClient<'_> {
    fn pair_command(&mut self, query: &str, read_timeout: bool) -> Result<String, Error> {
        self.client.request_text(
            Scheme::Http,
            "pair",
            Some(&format!("updateState=1&{query}")),
            LONG_CONNECT_TIMEOUT,
            read_timeout.then_some(READ_TIMEOUT),
        )
    }

    fn pair_challenge(&mut self) -> Result<String, Error> {
        self.client.request_text(
            Scheme::Https,
            "pair",
            Some("updateState=1&phrase=pairchallenge"),
            LONG_CONNECT_TIMEOUT,
            Some(READ_TIMEOUT),
        )
    }

    fn pin_server_certificate(&mut self, der: &[u8]) -> Result<(), Error> {
        if let Some(expected) = self.fingerprint {
            if crypto::sha256(der) != expected {
                return Err(Error::CertificateMismatch);
            }
        }
        self.client.pin(Some(der.to_vec()));
        Ok(())
    }

    fn unpair(&mut self) -> Result<(), Error> {
        self.client.unpair()
    }

    fn cancel_pairing(&mut self) -> Result<(), Error> {
        self.client.request_text(
            Scheme::Http,
            "unpair",
            None,
            LONG_CONNECT_TIMEOUT,
            Some(READ_TIMEOUT),
        )?;
        Ok(())
    }
}
