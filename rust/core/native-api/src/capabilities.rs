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
}
