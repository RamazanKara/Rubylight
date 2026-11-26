//! `frame_warp.json` in the configuration directory: what Frame Warp learned for each game,
//! kept for the game's next stream.
//!
//! Schema 2 ([`SCHEMA`]) keeps a [`PriorV2`] per game: the calibrator's gains, lags, curve and
//! lens with the gyro's gain, lag and yaw axis, and when it was last saved while a model was
//! locked. A schema 1 file (no `schema` field, priors without the gyro) reads as schema 2 with
//! no gyro and is written as schema 2 by the next save. A file that does not parse is renamed
//! `frame_warp.json.corrupt` with a warning and every game starts cold; a file from a newer
//! host is read but never written over. Each save writes a temporary file and renames it over
//! the old ([`butterpollo_session::state::write_json`]), one stream at a time, and keeps the
//! [`GAMES`] games used last.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use butterpollo_media_policy::frame_warp::PriorV2;
use serde::{Deserialize, Serialize};

/// In the configuration directory.
pub(crate) const FILE: &str = "frame_warp.json";
/// What a file that does not parse is renamed to.
const CORRUPT: &str = "frame_warp.json.corrupt";
/// The file's schema this host writes.
const SCHEMA: u32 = 2;
/// Games whose prior is kept; the least recently used goes first.
const GAMES: usize = 256;

/// The file: what each game's streams learned, by the stream's game key.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Remembered {
    /// Missing in a schema 1 file.
    #[serde(default = "schema_1")]
    schema: u32,
    #[serde(default)]
    games: BTreeMap<String, Game>,
}

impl Default for Remembered {
    fn default() -> Self {
        Self {
            schema: SCHEMA,
            games: BTreeMap::new(),
        }
    }
}

fn schema_1() -> u32 {
    1
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
struct Game {
    prior: PriorV2,
    /// Unix seconds of the last save.
    used: u64,
}

impl Remembered {
    /// Keeps `prior` for `key`, dropping the least recently used games past [`GAMES`].
    fn remember(&mut self, key: &str, prior: PriorV2, now: u64) {
        self.games.insert(key.to_owned(), Game { prior, used: now });
        while self.games.len() > GAMES {
            let Some(oldest) = self
                .games
                .iter()
                .min_by_key(|(_, game)| game.used)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.games.remove(&oldest);
        }
    }
}

/// One file for every stream: saves take turns.
static SAVES: Mutex<()> = Mutex::new(());

/// One game's entry in the file.
pub(crate) struct Store {
    path: PathBuf,
    key: Option<String>,
}

impl Store {
    /// The entry of the game `key` names (none: nothing is loaded or saved) in `directory`.
    pub(crate) fn new(directory: &Path, key: Option<String>) -> Self {
        Self {
            path: directory.join(FILE),
            key,
        }
    }

    /// What the game's earlier streams learned, if anything.
    pub(crate) fn load(&self) -> Option<PriorV2> {
        let key = self.key.as_ref()?;
        let _turn = SAVES.lock().unwrap_or_else(PoisonError::into_inner);
        match read(&self.path) {
            Ok(remembered) => remembered.games.get(key).map(|game| game.prior),
            Err(error) => {
                tracing::warn!(
                    path = %self.path.display(),
                    %error,
                    "frame warp could not read saved priors"
                );
                None
            }
        }
    }

    /// Keeps `prior` for the game; `confident` when a model is locked, which stamps it.
    pub(crate) fn save(&self, mut prior: PriorV2, confident: bool) {
        let Some(key) = &self.key else {
            return;
        };
        let _turn = SAVES.lock().unwrap_or_else(PoisonError::into_inner);
        let mut remembered = match read(&self.path) {
            Ok(remembered) => remembered,
            Err(error) => {
                tracing::warn!(
                    path = %self.path.display(),
                    %error,
                    "frame warp could not read existing priors; skipping save"
                );
                return;
            }
        };
        if remembered.schema > SCHEMA {
            tracing::warn!(
                path = %self.path.display(),
                schema = remembered.schema,
                "frame warp leaves priors a newer host saved alone"
            );
            return;
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_secs());
        let earlier = remembered.games.get(key).and_then(|g| g.prior.confident_at);
        prior.confident_at = if confident { Some(now) } else { earlier };
        remembered.schema = SCHEMA;
        remembered.remember(key, prior, now);
        if let Err(error) = butterpollo_session::state::write_json(&self.path, &remembered) {
            tracing::warn!(
                path = %self.path.display(),
                error = %format!("{error:#}"),
                "frame warp could not save what it learned"
            );
        }
    }
}

/// The file, or nothing learned when it does not exist yet or does not parse (then renamed
/// [`CORRUPT`]).
fn read(path: &Path) -> std::io::Result<Remembered> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Remembered::default());
        }
        Err(error) => return Err(error),
    };
    match serde_json::from_slice(&bytes) {
        Ok(remembered) => Ok(remembered),
        Err(error) => {
            let corrupt = path.with_file_name(CORRUPT);
            let renamed = std::fs::rename(path, &corrupt);
            tracing::warn!(
                path = %path.display(),
                %error,
                kept_as = %corrupt.display(),
                renamed = renamed.is_ok(),
                "frame warp starts cold: its saved priors do not parse"
            );
            Ok(Remembered::default())
        }
    }
}

#[cfg(test)]
mod tests;
