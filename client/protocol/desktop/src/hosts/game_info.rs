//! Artwork and facts from SteamGridDB and IGDB for the PCs' games ([`crate::game_info`]): the
//! worker that looks facts up for the library's filters and game pages, and what the pages read.

use nightjar_policy::game_info::cache::Record;
use nightjar_policy::game_info::job::{Game, Keys};

use super::*;

/// A cover the PC left out, for the SteamGridDB worker: it lands as the cover of `app` at
/// `version`, sized for `scale`.
pub struct CoverJob {
    pub id: u64,
    pub app: i32,
    pub version: String,
    pub game: Game,
    pub scale: f32,
}

/// What a cover worker needs to fill a gap the PC left with SteamGridDB's cover.
pub(super) struct CoverFallback {
    info: crate::game_info::Shared,
    worker: Sender<CoverJob>,
    events: Sender<Event>,
    waker: Waker,
    game: Game,
}

impl CoverFallback {
    /// Fills the gap for `app` at `version`: SteamGridDB's cover from disk when it is there, as the
    /// cover itself; else the PC's answer (`pc`, no cover) now and the cover from the SteamGridDB
    /// worker after it, in that order, so the PC's empty answer never lands over it.
    pub(super) fn fill(self, id: u64, app: i32, version: String, scale: f32, pc: Option<Vec<u8>>) -> Option<Event> {
        if let Some(kept) = self.info.kept_cover(&self.game) {
            let (image, ambient, prepared, sized) = super::covers::art_and_ambient(Some(kept), scale);
            if image.is_some() {
                return Some(Event::Art { id, app, version, image, ambient, prepared, sized });
            }
        }
        let (image, ambient, prepared, sized) = super::covers::art_and_ambient(pc, scale);
        if self.events.send(Event::Art { id, app, version: version.clone(), image, ambient, prepared, sized }).is_ok() {
            (self.waker)();
        }
        let _ = self.worker.send(CoverJob { id, app, version, game: self.game, scale });
        None
    }

    /// Asks the SteamGridDB worker for the cover again, sized for `scale`.
    pub(super) fn sharpen(self, id: u64, app: i32, version: String, scale: f32) {
        let _ = self.worker.send(CoverJob { id, app, version, game: self.game, scale });
    }
}

impl Hosts {
    /// Starts looking games up with `info`: a worker for facts and one for SteamGridDB covers,
    /// apart from the cover workers, which never wait on the services.
    pub fn set_game_info(&mut self, info: crate::game_info::Shared) {
        let (covers, jobs) = channel::<CoverJob>();
        let events = self.sender.clone();
        let waker = self.waker.clone();
        let worker = info.clone();
        let started = std::thread::Builder::new().name("nightjar-game-art".into()).spawn(move || {
            for CoverJob { id, app, version, game, scale } in jobs {
                let Some(decoded) = worker.cover(&game) else { continue };
                let (image, ambient, prepared, sized) =
                    super::covers::bake_art_and_ambient(super::covers::color_image(decoded), scale);
                if image.is_none() {
                    continue;
                }
                if events.send(Event::Art { id, app, version, image, ambient, prepared, sized }).is_err() {
                    return;
                }
                waker();
            }
        });
        if started.is_ok() {
            self.cover_worker = Some(covers);
        }
        let (sender, games) = channel::<Game>();
        let events = self.sender.clone();
        let waker = self.waker.clone();
        let worker = info.clone();
        let started = std::thread::Builder::new().name("nightjar-game-info".into()).spawn(move || {
            for game in games {
                if let Some(record) = worker.record(&game, false) {
                    if events.send(Event::GameInfo(Box::new(record))).is_err() {
                        return;
                    }
                    waker();
                }
            }
        });
        if started.is_ok() {
            self.facts_worker = Some(sender);
        }
        self.game_info = Some(info);
    }

    /// The keys in use, empty without any.
    pub fn game_info_keys(&self) -> Keys {
        self.game_info.as_ref().map(|i| i.keys()).unwrap_or_default()
    }

    /// Takes new keys: covers that found none are asked for again, and so are facts.
    pub fn set_game_info_keys(&mut self, keys: Keys) {
        let Some(info) = &self.game_info else { return };
        if info.keys() == keys {
            return;
        }
        info.set_keys(keys);
        Arc::make_mut(&mut self.facts_asked).clear();
        if !info.keys().info() {
            Arc::make_mut(&mut self.facts).clear();
        }
        for host in &mut self.list {
            Arc::make_mut(&mut host.art).retain(|_, art| !matches!(art, Art::Missing));
        }
    }

    /// Whether facts can be looked up: IGDB has its keys.
    pub fn facts_offered(&self) -> bool {
        self.game_info.as_ref().is_some_and(|i| i.keys().info())
    }

    /// Asks for the facts of `games` that were not asked for yet: from disk when kept, else from
    /// IGDB, one at a time on the worker.
    pub fn want_facts(&mut self, games: Vec<Game>) {
        if !self.facts_offered() {
            return;
        }
        let Some(worker) = &self.facts_worker else { return };
        for game in games {
            let key = nightjar_policy::game_info::names::key(&game.name, game.steam);
            if Arc::make_mut(&mut self.facts_asked).insert(key.clone()) && worker.send(game).is_err() {
                Arc::make_mut(&mut self.facts_asked).remove(&key);
                return;
            }
        }
    }

    /// Takes in a record a worker finished.
    pub(super) fn game_info_landed(&mut self, record: Record) {
        let retry = record.info == nightjar_policy::game_info::cache::Outcome::Failed;
        let key = record.key;
        match record.facts.filter(|f| !f.is_empty()) {
            Some(facts) => {
                Arc::make_mut(&mut self.facts).insert(key, facts);
            }
            None => {
                Arc::make_mut(&mut self.facts).remove(&key);
                if retry {
                    Arc::make_mut(&mut self.facts_asked).remove(&key);
                }
            }
        }
    }

    /// What a cover worker needs to fill a gap with SteamGridDB's cover for `app` on `host`:
    /// None without its key, or for an app the PC's list does not name.
    pub(super) fn cover_fallback(&self, host: &Host, app: i32) -> Option<CoverFallback> {
        let info = self.game_info.clone().filter(|i| i.keys().art())?;
        let worker = self.cover_worker.clone()?;
        let game = host.library()?.games.iter().find(|g| g.app_id as i32 == app)?;
        let store = game.store.as_ref().map(|s| (s.source.as_str(), s.id.as_str()));
        Some(CoverFallback { info, worker, events: self.sender.clone(), waker: self.waker.clone(), game: crate::game_info::game_of(&game.title, game.game_id.as_deref(), store) })
    }
}
