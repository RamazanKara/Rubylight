//! `GET /capabilities`.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    /// The version of the path that answered.
    pub api_version: u32,
    pub host: HostInfo,
    /// What the host implements and has turned on, the same for every client.
    pub features: Vec<String>,
    /// The calling device's permissions.
    pub permissions: Permissions,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostInfo {
    pub name: String,
    pub version: String,
    pub unique_id: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Permissions {
    pub list: bool,
    pub view: bool,
    pub launch: bool,
    pub host_control: bool,
    /// Upload files (device bit `1 << 18`). Missing from hosts before file
    /// transfer, which then reads as not granted.
    #[serde(default)]
    pub file_upload: bool,
    /// Download files (device bit `1 << 19`).
    #[serde(default)]
    pub file_download: bool,
    /// The device may attach USB devices: its own `allow_usb` flag, never a
    /// permission bit. Missing from hosts before 3.0, which then reads as not granted.
    #[serde(default)]
    pub usb: bool,
    /// The device has at least one shared folder grant (`shared_folders`).
    /// Missing from hosts before 3.0, which then reads as not granted.
    #[serde(default)]
    pub shared_folders: bool,
}
