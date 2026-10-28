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
    /// File transfer: the device lacks Upload files or Download files.
    Forbidden,
    /// File transfer is off on the host.
    FileTransferDisabled,
    /// A file name the host refuses rather than rewrites.
    InvalidName,
    /// The upload ended before its `Content-Length`; nothing was kept.
    IncompleteUpload,
    /// An upload without a `Content-Length`.
    LengthRequired,
    /// An upload over `max_file_bytes`.
    TooLarge,
    /// The transfer folder's volume has no room for the upload.
    InsufficientStorage,
    /// Nobody is signed in to Windows on the host, so there is no folder.
    NoConsoleUser,
    /// The device already has an upload running.
    TransferBusy,
    /// USB forwarding is off on the host, or the device has no `allow_usb`.
    UsbDisabled,
    /// The host already forwards `usb_max_devices` devices, or a tunnel token
    /// was used twice.
    UsbLimit,
    /// Shared folders are off on the host.
    FoldersDisabled,
    /// Windows denies the signed-in user reading a shared folder, or a
    /// folder or file in it.
    FolderUnreadable,
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
            Self::NotPaired
            | Self::PermissionRequired
            | Self::HostControlRequired
            | Self::Forbidden
            | Self::FileTransferDisabled
            | Self::UsbDisabled
            | Self::FoldersDisabled
            | Self::FolderUnreadable => 403,
            Self::NotFound => 404,
            Self::InvalidRequest | Self::InvalidName | Self::IncompleteUpload => 400,
            Self::LengthRequired => 411,
            Self::TooLarge => 413,
            Self::InvalidValue | Self::NotWritable => 422,
            Self::NoSession
            | Self::NotSessionOwner
            | Self::SessionBusy
            | Self::DriverRejected
            | Self::NotStreaming
            | Self::NoConsoleUser
            | Self::TransferBusy
            | Self::UsbLimit => 409,
            Self::InsufficientStorage => 507,
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
