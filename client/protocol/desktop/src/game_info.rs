//! Artwork from SteamGridDB and game facts from IGDB, with the player's own keys: the desktop's
//! carrier of `nightjar_policy::game_info`, which decides every request. This holds the keys,
//! carries the requests over HTTPS ([`crate::update_net::Native::exchange`]), keeps the records
//! and images on disk and runs one lookup at a time, so the services' pace holds.
//!
//! Without keys it asks nothing and every call answers None: Nightjar looks as it did.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use nightjar_policy::game_info::cache::{self, Available, Record};
use nightjar_policy::game_info::job::{Game, Job, Keys, Reply, Request, Service, Session, Step, Wants};
use nightjar_policy::game_info::names;
use nightjar_policy::game_info::redact::redact;

/// The file the keys are kept in, in the config folder beside `settings.json` but apart from it,
/// so settings shared or reset never carry them.
const KEYS_FILE: &str = "game_info_keys.json";
/// The folder of records and images.
const CACHE_DIR: &str = "game-info";

/// What a request got: the status (0 when nothing came back), `Retry-After` and the body.
pub struct Answer {
    pub status: u16,
    pub retry_after: Option<u64>,
    pub body: Vec<u8>,
}

/// Carries one request. The app's is HTTPS; tests answer from fixtures and never reach a network.
pub trait Http: Send + Sync {
    fn send(&self, request: &Request) -> Answer;
}

/// HTTPS through the update check's transport: WinHTTP on Windows, ureq elsewhere.
pub struct Https(pub crate::update_net::Native);

impl Http for Https {
    fn send(&self, request: &Request) -> Answer {
        let Some(url) = crate::update_net::Url::parse(&request.url).filter(|u| u.secure) else {
            return Answer { status: 0, retry_after: None, body: Vec::new() };
        };
        let follow = request.service == Service::Image;
        match self.0.exchange(&request.method, &url, &request.headers, request.body.as_deref().map(str::as_bytes), follow) {
            Ok(got) => Answer { status: got.status, retry_after: got.retry_after, body: got.body },
            // No answer: the job backs off and tries again. The error is not logged: the job
            // notes the failure itself, without an address.
            Err(_) => Answer { status: 0, retry_after: None, body: Vec::new() },
        }
    }
}

/// The keys as kept on disk.
pub fn load_keys(dir: &Path) -> Keys {
    std::fs::read(dir.join(KEYS_FILE)).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
}

/// Keeps `keys`, readable by this user only where the system has such a thing; empty keys remove
/// the file.
pub fn save_keys(dir: &Path, keys: &Keys) -> std::io::Result<()> {
    let path = dir.join(KEYS_FILE);
    if keys.secrets().iter().all(|k| k.is_empty()) {
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    std::fs::create_dir_all(dir)?;
    let text = serde_json::to_vec_pretty(keys).map_err(std::io::Error::other)?;
    let part = path.with_extension("json.part");
    let _ = std::fs::remove_file(&part);
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    // Unix (Linux and macOS): create the key file with 0o600 permissions before writing any secret.
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    {
        use std::io::Write;
        if let Err(e) = opts.open(&part).and_then(|mut file| file.write_all(&text)) {
            let _ = std::fs::remove_file(&part);
            return Err(e);
        }
    }
    std::fs::rename(&part, &path)
}

/// The lookups of this app run.
pub struct GameInfo {
    keys: RwLock<Keys>,
    dir: PathBuf,
    http: Box<dyn Http>,
    /// Held for a whole lookup, its waits included: lookups never interleave, so the services'
    /// pace and the token hold across them. Only the facts and SteamGridDB-cover workers wait on
    /// it, never a cover worker or the UI.
    session: Mutex<Option<Session>>,
    /// The records read or made this run, by key.
    records: Mutex<HashMap<String, Record>>,
    started: Instant,
}

fn wall_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl GameInfo {
    /// Lookups that keep their records in `config_dir`'s `game-info` folder.
    pub fn new(config_dir: &Path, keys: Keys, http: Box<dyn Http>) -> Self {
        Self { keys: RwLock::new(keys), dir: config_dir.join(CACHE_DIR), http, session: Mutex::new(None), records: Mutex::new(HashMap::new()), started: Instant::now() }
    }

    pub fn keys(&self) -> Keys {
        self.keys.read().unwrap_or_else(PoisonError::into_inner).clone()
    }

    pub fn set_keys(&self, keys: Keys) {
        *self.keys.write().unwrap_or_else(PoisonError::into_inner) = keys;
    }

    fn now_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    fn record_path(&self, key: &str) -> PathBuf {
        self.dir.join(cache::record_file(key))
    }

    /// The record kept for `key`, fresh or not, without asking anything.
    pub fn kept(&self, key: &str) -> Option<Record> {
        if let Some(record) = self.records.lock().unwrap_or_else(PoisonError::into_inner).get(key) {
            return Some(record.clone());
        }
        let record: Record = serde_json::from_slice(&std::fs::read(self.record_path(key)).ok()?).ok()?;
        (record.key == key).then(|| {
            self.records.lock().unwrap_or_else(PoisonError::into_inner).insert(key.to_owned(), record.clone());
            record
        })
    }

    fn keep(&self, record: &Record) {
        let _ = std::fs::create_dir_all(&self.dir);
        if let Ok(text) = serde_json::to_vec(record) {
            let path = self.record_path(&record.key);
            let part = path.with_extension("part");
            if std::fs::write(&part, text).is_ok() {
                let _ = std::fs::rename(&part, &path);
            }
        }
        self.records.lock().unwrap_or_else(PoisonError::into_inner).insert(record.key.clone(), record.clone());
    }

    /// The record of `game`, from the cache while it holds, else looked up now (blocking: call it
    /// off the UI thread). `cover` asks SteamGridDB for a cover too; facts are asked whenever IGDB
    /// has keys. None without keys.
    pub fn record(&self, game: &Game, cover: bool) -> Option<Record> {
        let keys = self.keys();
        let wants = Wants { cover: cover && keys.art(), hero: false, logo: false, facts: keys.info() };
        if !wants.cover && !wants.facts {
            return self.kept(&names::key(&game.name, game.steam));
        }
        let available = Available { art: wants.cover, info: wants.facts };
        let key = names::key(&game.name, game.steam);
        if let Some(record) = self.kept(&key).filter(|r| r.fresh_with(wall_secs(), available, &keys)) {
            return Some(record);
        }
        let mut session = self.session.lock().unwrap_or_else(PoisonError::into_inner);
        // Another worker may have looked it up while this one waited.
        if let Some(record) = self.kept(&key).filter(|r| r.fresh_with(wall_secs(), available, &keys)) {
            return Some(record);
        }
        let mut job = Job::new(game.clone(), keys.clone(), wants, session.take(), wall_secs());
        let mut reply = None;
        let done = loop {
            match job.next(reply.take(), self.now_ms()) {
                Step::Send(request) => {
                    let answer = self.http.send(&request);
                    reply = Some(Reply { status: answer.status, body: String::from_utf8_lossy(&answer.body).into_owned(), retry_after: answer.retry_after });
                }
                Step::Wait(ms) => std::thread::sleep(Duration::from_millis(ms.min(60_000))),
                Step::Done(done) => break done,
            }
        };
        *session = Some(done.session);
        drop(session);
        for line in &done.log {
            // Already without keys; masked again in case a service echoed one.
            eprintln!("nightjar: game info: {}", redact(line, &keys.secrets()));
        }
        let record = done.record.merged(self.kept(&key).as_ref());
        self.keep(&record);
        Some(record)
    }

    /// The decoded image at `url` (a record's), from its file, else fetched, validated once, and
    /// kept on disk.
    pub fn image(&self, url: &str) -> Option<image::DynamicImage> {
        let path = self.dir.join("img").join(cache::image_file(url));
        if let Ok(bytes) = std::fs::read(&path) {
            if (bytes.len() as u64) < crate::update_net::EXCHANGE_LIMIT
                && let Ok(decoded) = image::load_from_memory(&bytes)
            {
                return Some(decoded);
            }
            let _ = std::fs::remove_file(&path);
        }
        let answer = self.http.send(&Request { service: Service::Image, method: "GET".into(), url: url.to_owned(), headers: Vec::new(), body: None });
        if answer.status != 200 || answer.body.is_empty() {
            eprintln!("nightjar: game info: an image did not load (HTTP {})", answer.status);
            return None;
        }
        if (answer.body.len() as u64) >= crate::update_net::EXCHANGE_LIMIT {
            eprintln!("nightjar: game info: an image could not be decoded");
            return None;
        }
        let Ok(decoded) = image::load_from_memory(&answer.body) else {
            eprintln!("nightjar: game info: an image could not be decoded");
            return None;
        };
        let _ = std::fs::create_dir_all(path.parent()?);
        let part = path.with_extension("part");
        if std::fs::write(&part, &answer.body).is_ok()
            && std::fs::rename(&part, &path).is_err()
        {
            let _ = std::fs::remove_file(&part);
        }
        Some(decoded)
    }

    /// SteamGridDB's decoded cover for `game`, for one the PC has none for. None without its key,
    /// or when there is none.
    pub fn cover(&self, game: &Game) -> Option<image::DynamicImage> {
        if !self.keys().art() {
            return None;
        }
        let record = self.record(game, true)?;
        self.image(record.cover.as_deref()?)
    }

    /// SteamGridDB's cover for `game` when its record and image are already on disk; asks nothing.
    pub fn kept_cover(&self, game: &Game) -> Option<Vec<u8>> {
        if !self.keys().art() {
            return None;
        }
        let record = self.kept(&names::key(&game.name, game.steam))?;
        std::fs::read(self.dir.join("img").join(cache::image_file(record.cover.as_deref()?))).ok()
    }
}

/// The game a host lists, as a lookup takes it: its title and its Steam app id when the host gave
/// one (Rubylight's native library: `game_id` `steam:<id>`, or a Steam store entry).
pub fn game_of(title: &str, game_id: Option<&str>, store: Option<(&str, &str)>) -> Game {
    let steam = names::steam_of(game_id, store);
    Game { name: title.to_owned(), steam }
}

/// Shared between the UI and the workers.
pub type Shared = Arc<GameInfo>;

#[cfg(test)]
mod tests;
