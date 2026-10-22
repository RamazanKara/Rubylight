//! The body of every non-2xx answer.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    NotPaired,
    PermissionRequired,
    HostControlRequired,
    NotFound,
    InvalidRequest,
    InvalidValue,
    NotWritable,
    NoSession,
    NotSessionOwner,
    SessionBusy,
    DriverRejected,
    TemporarilyUnavailable,
    Internal,
    /// Appears only as an action `reason`: a disconnect without a stream is
    /// idempotent and answers 200.
    NotStreaming,
    /// A code from a newer host. Clients treat it by its HTTP status.
    #[serde(other)]
    Unknown,
}

impl ErrorCode {
    /// The HTTP status the host answers with. `NotStreaming` is never sent as
    /// an error, and 409 is what it would mean if it were. `Unknown` is not
    /// something a host emits.
    pub fn status(self) -> u16 {
        match self {
            Self::NotPaired | Self::PermissionRequired | Self::HostControlRequired => 403,
            Self::NotFound => 404,
            Self::InvalidRequest => 400,
            Self::InvalidValue | Self::NotWritable => 422,
            Self::NoSession
            | Self::NotSessionOwner
            | Self::SessionBusy
            | Self::DriverRejected
            | Self::NotStreaming => 409,
            Self::TemporarilyUnavailable => 503,
            Self::Internal | Self::Unknown => 500,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub error: ApiError,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiError {
    pub code: ErrorCode,
    /// Plain English the client can show as it is.
    pub message: String,
    /// The setting a write failed on.
    pub setting: Option<String>,
    pub retry_after_ms: Option<u64>,
}
