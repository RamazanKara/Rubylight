//! Host HTTP pure DTOs, error types, and XML/JSON reply parsers shared by clients without
//! TLS, sockets, or crypto dependencies.

mod apps;
mod capabilities;
mod capabilities_json;
mod coop;
mod device_approval;
mod files_types;
mod folders_types;
mod launch_types;
mod luminance;
mod native_types;
mod network_path_types;
mod neural;
mod pc_power;
mod remote_check_types;
pub mod serverinfo;
mod session_guests;
pub mod transfers;
pub mod xml;

pub use apps::{app_control, parse_app_list, App, AppControl, AppRole, LEGACY_ORDERING};
pub use capabilities::{
    parse_rubylight_version, Availability, Evidence, Family, Feature, HostCapabilities, Reason,
    PERMISSION_CLIPBOARD_READ, PERMISSION_CLIPBOARD_SET, PERMISSION_CONTROLLER, PERMISSION_INPUT,
    PERMISSION_LAUNCH, PERMISSION_LIST, PERMISSION_SERVER_COMMAND, PERMISSION_VIEW,
    PYROWAVE_BANDWIDTH_PROBE_BYTES, PYROWAVE_CODEC_BITS,
};
pub use coop::{
    Coop, CoopEnd, CoopEndReason, CoopGame, CoopJoined, CoopPhase, CoopPlayer, CoopProblem,
    CoopSession, JoinRequest,
};
pub use device_approval::{
    valid_pairing_pin, PairingApprove, PairingApproved, PairingDeny, PairingRequest,
    PairingRequests, FEATURE_PAIRING_APPROVE,
};
pub use files_types::{
    FileError, FileList, FileRefusal, RemoteFile, TransferCancel, Uploaded,
};
pub use folders_types::{
    check_id, check_path, child_path, collision_name, file_name, has_part_suffix, parent_path,
    EntryKind, FolderAccess, FolderEntries, FolderEntry, SharedFolder, MAX_FOLDER_DEPTH,
    PART_SUFFIX,
};
pub use launch_types::HostDisplay;
pub use luminance::DisplayLuminance;
pub use native_types::{
    parse_added, session_held_by_other_device, session_is_active, session_is_suspended, AddedGame,
    FilePermissions, GameExtra, MovedTo, NativeCapabilities, NativeLibrary, PlayingOn, StreamMove,
    FEATURE_LIBRARY_ADD, FEATURE_LIBRARY_UNADDED, FEATURE_SESSION_CONTINUE_HERE, FEATURE_USB,
};
pub use network_path_types::{NetworkPath, MAX_NETWORK_PATH_BYTES};
pub use neural::{
    Neural, NeuralAction, NeuralJob, NeuralMode, NeuralPhase, NeuralSwitch, NeuralWhy, NeuralWrite,
    FRAME_GENERATION,
};
pub use pc_power::{
    host_words, update_arrived, HostUpdate, LateRefusal, PcPower, PcPowerOffer, PowerAvailability,
    PowerRequest, FEATURE_RESTART, FEATURE_SHUTDOWN, FEATURE_UPDATE, POWER_TOKEN_REFRESH,
};
pub use remote_check_types::{
    verification_problem, ConnectionTestStats, RemoteCheck, RemoteOutcome, REMOTE_CHECK_PROBES,
};
pub use serverinfo::{parse_version_quad, ServerInfo};
pub use session_guests::{
    guest_ended, join_refused, GuestAction, GuestRequest, InviteCreate, InviteCreated,
    SessionGuest, SessionGuests, EXPIRED, FEATURE_GUEST_INVITES, FEATURE_SESSION_GUESTS,
    GAME_OVER, GUESTS_POLL_MS, GUEST_CONTROLLER, GUEST_KEYBOARD_MOUSE, GUEST_PERMISSIONS,
    GUEST_VIEW, INVITED_JOINS_ONLY, KICKED, REVOKED,
};

/// The Rubylight native API v1 wire types.
pub use rubylight_native_api as native_api;

pub const DEFAULT_HTTP_PORT: u16 = 47989;
pub const DEFAULT_HTTPS_PORT: u16 = 47984;
/// The query parameter on every request a device's second screen makes: the host keeps that stream
/// beside the device's first instead of taking it for the first starting again
/// ([`Feature::SecondScreen`]).
pub const SECOND_SCREEN_PARAMETER: &str = "rlSecondScreen";

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
    /// status, which an `ErrorCode::Unknown` from a newer host is handled by. Boxed: with its
    /// optional title, next step and technical detail the body is large, and every
    /// `Result<_, Error>` would carry its size.
    Api {
        status: u16,
        error: Box<native_api::error::ApiError>,
    },
    Malformed(String),
    /// HTTPS needs the host certificate from pairing.
    NotPaired,
    InvalidIdentity(String),
    InvalidArgument(String),
    /// Ended by `HostClient::cancel_pending_requests`.
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

/// The PC took a launch or resume and gave no answer within its limit; carried in an [`Error::Io`]
/// of kind `Other`, so it is not mistaken for a dropped connection that a retry would cure.
#[derive(Debug)]
pub struct LaunchUnanswered {
    pub seconds: u64,
}

impl std::fmt::Display for LaunchUnanswered {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "The PC didn't answer the start within {} seconds. Try again, or check what the PC is doing.",
            self.seconds
        )
    }
}

impl std::error::Error for LaunchUnanswered {}

impl Error {
    /// Whether this is a launch or resume the PC never answered ([`LaunchUnanswered`]).
    pub fn is_launch_unanswered(&self) -> bool {
        matches!(self, Error::Io(e) if e.get_ref().is_some_and(|inner| inner.is::<LaunchUnanswered>()))
    }

    /// The native API's error body, if this is one.
    pub fn api_error(&self) -> Option<&native_api::error::ApiError> {
        match self {
            Error::Api { error, .. } => Some(error.as_ref()),
            _ => None,
        }
    }

    /// The HTTP status of a host answer, whatever kind of error it became.
    pub fn http_status(&self) -> Option<u16> {
        match self {
            Error::Api { status, .. } => Some(*status),
            Error::Status { code, .. } => u16::try_from(*code).ok(),
            Error::NotFound(_) => Some(404),
            _ => None,
        }
    }
}

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
    pub fn require(capabilities: &HostCapabilities, feature: Feature) -> Result<(), Self> {
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
