//! `GET /library`.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Library {
    /// A hash of the whole response without `running`. Clients refetch when
    /// it changes.
    pub revision: String,
    pub games: Vec<Game>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Game {
    /// The app's `uuid` in `apps.json`. The client's primary key per host.
    pub install_id: String,
    /// CRC32 of the name unless pinned. Only for `/appasset` and old calls.
    pub app_id: u32,
    pub title: String,
    /// `<source>:<store id>`, `null` for manual apps. Merges one title across
    /// hosts.
    pub game_id: Option<String>,
    pub store: Option<Store>,
    /// From the host's last launcher scan, `null` for manual apps or when no
    /// scan has run.
    pub installed: Option<bool>,
    /// The app is the host's current game, whoever owns it.
    pub running: bool,
    pub artwork: Artwork,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Store {
    /// A string, not a closed enum, so later stores don't break v1 clients.
    pub source: String,
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artwork {
    /// A path on the same HTTPS origin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover: Option<String>,
    /// Changes when the image file changes.
    pub version: String,
}
