//! The [`Registry`]: the host's Nucleus Co-op install, its handlers and games,
//! the window the console opens, and the sessions launches are admitted to.
use super::*;

struct Entry {
    instance: Arc<dyn Instance>,
    app_id: u32,
    /// The launch streaming each slot; a leave from any other launch is stale.
    holders: Vec<Option<String>>,
    /// Each slot's feedback its stream has not taken yet.
    feedback: Vec<Vec<PadReport>>,
    /// When the registry first saw the session ended.
    ended: Option<Instant>,
}

#[derive(Default)]
pub(super) struct State {
    sessions: BTreeMap<u32, Entry>,
    next: u32,
    /// The bytes downloaded while an install runs, or is claimed to run.
    installing: Option<u64>,
    install_error: Option<String>,
    /// Finished installs: a call waiting for one wakes when it moves.
    installs: u64,
    /// The last install's result, which its waiters share.
    install_result: Option<Result<PathBuf, String>>,
    /// Calls waiting for the install another one runs.
    pub(super) install_waiters: usize,
}

struct Cached {
    at: Instant,
    folder: PathBuf,
    catalog: Result<Arc<Catalog>, String>,
}

/// [`own_status`]'s answer for one .exe, as it was modified then.
pub(super) struct OwnStatus {
    pub(super) at: Instant,
    modified: Option<std::time::SystemTime>,
    status: Result<Option<Run>, Missing>,
}

/// The host's Nucleus Co-op sessions.
pub struct Registry {
    pc: RwLock<Arc<dyn Pc>>,
    /// One co-op launch is admitted at a time, so two first players of one
    /// game start one session.
    admit: Mutex<()>,
    state: Mutex<State>,
    /// Signalled when an install finishes.
    install_done: Condvar,
    catalog: Mutex<Option<Cached>>,
    /// The console's [`own_status`] answers by .exe path; only the console
    /// reads them.
    pub(super) own_statuses: Mutex<BTreeMap<String, OwnStatus>>,
    /// Whether the signed-in account elevates, or why not, as the console
    /// last read it ([`Self::account`]), and when.
    pub(super) account: Mutex<Option<(Instant, Account)>>,
    /// The Nucleus Co-op the console opened: the only one the host closes,
    /// for a session or the user.
    opened: Mutex<Option<Box<dyn Window>>>,
}

impl Registry {
    pub fn new(pc: Arc<dyn Pc>) -> Self {
        Self {
            pc: RwLock::new(pc),
            admit: Mutex::new(()),
            state: Mutex::default(),
            install_done: Condvar::new(),
            catalog: Mutex::new(None),
            own_statuses: Mutex::default(),
            account: Mutex::new(None),
            opened: Mutex::new(None),
        }
    }
    /// This PC; in tests a PC without Nucleus Co-op, so no test reads the
    /// real one.
    pub fn on_pc() -> Self {
        #[cfg(not(test))]
        let pc: Arc<dyn Pc> = Arc::new(OnPc);
        #[cfg(test)]
        let pc: Arc<dyn Pc> = Arc::new(fake::Pc::unavailable());
        Self::new(pc)
    }
    #[cfg(test)]
    pub fn use_pc(&self, pc: Arc<dyn Pc>) {
        *self.pc.write().unwrap() = pc;
        *self.catalog.lock().unwrap() = None;
        self.own_statuses.lock().unwrap().clear();
        *self.account.lock().unwrap() = None;
    }
    fn pc(&self) -> Arc<dyn Pc> {
        self.pc
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
    pub(super) fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Why this host cannot run Nucleus Co-op sessions, or None.
    pub fn unavailable(&self) -> Option<String> {
        self.pc().unavailable()
    }

    /// The co-op entries of `apps`: none while Nucleus Co-op is unavailable
    /// or not installed. Each failure still shows: the console's panel names
    /// a broken install, and [`Self::catalog`] logs an unreadable one.
    pub fn entries(&self, apps: &[App], configured: &str) -> Vec<CoopApp> {
        let pc = self.pc();
        if pc.unavailable().is_some() {
            return Vec::new();
        }
        let folder = match pc.installed(configured) {
            Ok(Some(folder)) => folder,
            Ok(None) => return Vec::new(),
            Err(error) => {
                tracing::warn!(error = %format!("{error:#}"), "Nucleus Co-op: no co-op entries");
                return Vec::new();
            }
        };
        match self.catalog(&folder, false) {
            Ok(catalog) => entries_of(&catalog, apps),
            Err(_) => Vec::new(),
        }
    }

    /// The co-op entry with id `id`.
    pub fn resolve(&self, apps: &[App], configured: &str, id: u32) -> Option<CoopApp> {
        self.entries(apps, configured)
            .into_iter()
            .find(|coop| coop.id() == id)
    }

    /// The handlers and games of `folder`, read again when `fresh` or once
    /// the last read is older than [`CATALOG_TTL`]. A failure is logged when
    /// it is new.
    fn catalog(&self, folder: &Path, fresh: bool) -> Result<Arc<Catalog>, String> {
        let mut cached = self.catalog.lock().unwrap_or_else(PoisonError::into_inner);
        if !fresh
            && let Some(cached) = cached
                .as_ref()
                .filter(|c| c.folder == folder && c.at.elapsed() < CATALOG_TTL)
        {
            return cached.catalog.clone();
        }
        let catalog = self
            .pc()
            .catalog(folder)
            .map(Arc::new)
            .map_err(|error| format!("{error:#}"));
        if let Err(error) = &catalog
            && cached
                .as_ref()
                .is_none_or(|c| c.catalog.as_ref().err() != Some(error))
        {
            tracing::warn!(folder = %folder.display(), %error, "Nucleus Co-op: the handlers and games did not load");
        }
        *cached = Some(Cached {
            at: Instant::now(),
            folder: folder.into(),
            catalog: catalog.clone(),
        });
        catalog
    }

    /// Where the host's copy of Nucleus Co-op stands.
    pub fn install(&self, configured: &str) -> Install {
        let installed = self.pc().installed(configured);
        let state = self.lock();
        match (installed, state.installing) {
            (Ok(Some(folder)), _) if !configured.trim().is_empty() => Install::Configured(folder),
            (Ok(Some(folder)), _) => Install::Installed(folder),
            (_, Some(received)) => Install::Installing(received),
            (Err(error), None) => Install::Failed(format!("{error:#}")),
            (Ok(None), None) => state
                .install_error
                .clone()
                .map_or(Install::Missing, Install::Failed),
        }
    }

    /// The Nucleus folder, installing it first. One install runs at a time:
    /// a launch while the console's Set up co-op (or another launch) installs
    /// waits for it and shares its result, so there is one download, one
    /// progress and one result. A failure is kept for the console.
    fn ensure(&self, pc: &dyn Pc, configured: &str) -> Result<PathBuf, Refusal> {
        if let Ok(Some(folder)) = pc.installed(configured) {
            return Ok(folder);
        }
        let mut state = self.lock();
        if state.installing.is_some() {
            let round = state.installs;
            state.install_waiters += 1;
            while state.installs == round {
                state = self
                    .install_done
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner);
            }
            state.install_waiters -= 1;
            return match state.install_result.clone() {
                Some(Ok(folder)) => Ok(folder),
                Some(Err(message)) => Err(Refusal::unavailable(message)),
                None => Err(Refusal::unavailable("Nucleus Co-op is not ready")),
            };
        }
        state.installing = Some(0);
        drop(state);
        self.run_install(pc, configured)
    }

    /// Run the install this caller claimed (`installing` set), keep its
    /// result and wake the calls waiting for it.
    /// The install path returns its errors instead of panicking, so every
    /// waiter gets the reason. The release profile aborts on a panic
    /// (`panic = "abort"`), which ends the host loudly; catching one here
    /// only matters where panics unwind (test and debug builds).
    fn run_install(&self, pc: &dyn Pc, configured: &str) -> Result<PathBuf, Refusal> {
        let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            pc.ensure(configured, &|received| {
                self.lock().installing = Some(received);
            })
        }));
        let result = match run {
            Ok(result) => result.map_err(|error| format!("Nucleus Co-op is not ready: {error:#}")),
            Err(panic) => {
                let why = panic
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| panic.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "it panicked".into());
                tracing::error!(why, "Nucleus Co-op: the install panicked");
                Err(format!(
                    "Nucleus Co-op is not ready: the install stopped: {why}"
                ))
            }
        };
        let mut state = self.lock();
        state.installing = None;
        state.installs += 1;
        state.install_error = result.as_ref().err().cloned();
        state.install_result = Some(result.clone());
        self.install_done.notify_all();
        result.map_err(Refusal::unavailable)
    }

    /// The console's Set up co-op: whether Nucleus Co-op is there, already
    /// downloading, or now marked as installing, which the caller then runs
    /// with [`Self::install_now`] off the async runtime. Refused where
    /// Nucleus Co-op can't run, so nothing downloads that could never start.
    pub fn set_up(&self, configured: &str) -> Result<SetUp, Refusal> {
        let pc = self.pc();
        if let Some(why) = pc.unavailable() {
            return Err(Refusal::unavailable(why));
        }
        // A folder that can't be trusted (its owner or ACL) is refused, not
        // downloaded over.
        let installed = pc.installed(configured).map_err(|error| {
            Refusal::unavailable(format!("Nucleus Co-op is not ready: {error:#}"))
        })?;
        let mut state = self.lock();
        if installed.is_some() {
            return Ok(SetUp::Installed);
        }
        if state.installing.is_some() {
            return Ok(SetUp::Installing);
        }
        state.installing = Some(0);
        state.install_error = None;
        Ok(SetUp::Started)
    }

    /// Run the install [`Self::set_up`] started; a failure shows in
    /// [`Self::install`], as a launch's does.
    pub fn install_now(&self, configured: &str) {
        let pc = self.pc();
        // `set_up` claimed the install; `Pc::ensure` downloads nothing when
        // it is there after all.
        match self.run_install(pc.as_ref(), configured) {
            Ok(folder) => {
                tracing::info!(folder = %folder.display(), "Nucleus Co-op: set up from the console")
            }
            Err(refusal) => {
                tracing::warn!(error = %refusal, "Nucleus Co-op: setting up from the console failed")
            }
        }
    }

    /// Open Nucleus Co-op's window on the PC, where the user adds a handler
    /// or a game. Refused where it can't run, before it is set up, while a
    /// session runs or starts, which holds it, and while Nucleus Co-op
    /// already runs. The next session closes it first.
    pub fn open(&self, configured: &str) -> Result<(), Refusal> {
        let pc = self.pc();
        if let Some(why) = pc.unavailable() {
            return Err(Refusal::unavailable(why));
        }
        let folder = match pc.installed(configured) {
            Ok(Some(folder)) => folder,
            Ok(None) => {
                return Err(Refusal::busy(
                    "Nucleus Co-op is not set up yet: set up co-op first",
                ));
            }
            Err(error) => {
                return Err(Refusal::unavailable(format!(
                    "Nucleus Co-op is not ready: {error:#}"
                )));
            }
        };
        // A launch being admitted may start Nucleus Co-op any moment.
        let _one = match self.admit.try_lock() {
            Ok(one) => one,
            Err(std::sync::TryLockError::Poisoned(one)) => one.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => {
                return Err(Refusal::busy(
                    "A co-op or background stream is starting: open Nucleus Co-op once it has",
                ));
            }
        };
        // Read with no launch being admitted, so a session can't slip in.
        let running = self.lock().sessions.values().find_map(|entry| {
            entry
                .instance
                .session(|s| (!s.ended()).then(|| s.title.clone()))
        });
        if let Some(title) = running {
            return Err(Refusal::busy(format!(
                "Nucleus Co-op is running {title}: end that session first"
            )));
        }
        let mut opened = self.opened.lock().unwrap_or_else(PoisonError::into_inner);
        // Ours by its job, not its first process id: Nucleus Co-op may have
        // restarted itself.
        if let Some(window) = opened.as_ref() {
            let running = window.running().map_err(|error| {
                Refusal::unavailable(format!(
                    "Nucleus Co-op's window could not be read: {error:#}"
                ))
            })?;
            if running {
                return Err(Refusal::busy(
                    "Nucleus Co-op is already open on the PC's screen: use it there, or close it here",
                ));
            }
            *opened = None;
        }
        match pc.running() {
            Ok(Some(pid)) => {
                return Err(Refusal::busy(format!(
                    "Nucleus Co-op is already running on the PC (process {pid}): close it there first"
                )));
            }
            Ok(None) => {}
            Err(error) => {
                return Err(Refusal::unavailable(format!(
                    "Nucleus Co-op's processes could not be listed: {error:#}"
                )));
            }
        }
        let window = pc.open(&folder).map_err(|error| {
            Refusal::unavailable(format!("Nucleus Co-op did not open: {error:#}"))
        })?;
        tracing::info!(pid = window.pid(), folder = %folder.display(), "Nucleus Co-op: opened on the PC from the console");
        *opened = Some(window);
        Ok(())
    }

    /// Whether Windows gives the account signed in on this PC an elevated
    /// token, which hub handlers, co-op and games that need administrator
    /// rights run with, or why not: for the console, read again once
    /// [`OWN_STATUS_TTL`] passed, so its poll asks Windows once a minute.
    /// None when Rubylight could not tell (nobody signed in yet), asked
    /// again at the next poll.
    pub fn account(&self) -> Option<Account> {
        let mut kept = self.account.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((at, account)) = *kept
            && at.elapsed() < OWN_STATUS_TTL
        {
            return Some(account);
        }
        match self.pc().account() {
            Ok(account) => {
                *kept = Some((Instant::now(), account));
                Some(account)
            }
            Err(error) => {
                *kept = None;
                tracing::debug!("Nucleus Co-op: whether the signed-in account elevates: {error:#}");
                None
            }
        }
    }

    /// Whether the Nucleus Co-op the console opened still runs. One that
    /// can't be read counts as open, so the console offers Close, which then
    /// says why.
    pub fn opened(&self) -> bool {
        let opened = match self.opened.try_lock() {
            Ok(opened) => opened,
            Err(std::sync::TryLockError::Poisoned(opened)) => opened.into_inner(),
            // Being opened or closed right now.
            Err(std::sync::TryLockError::WouldBlock) => return true,
        };
        opened
            .as_ref()
            .is_some_and(|window| window.running().unwrap_or(true))
    }

    /// The console's Close Nucleus Co-op: closes the window [`Self::open`]
    /// opened, whether it was still open. A Nucleus Co-op the host did not
    /// start is refused, never ended.
    pub fn close(&self) -> Result<bool, Refusal> {
        let mut opened = self.opened.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(window) = opened.as_ref() {
            let running = window.running().map_err(|error| {
                Refusal::unavailable(format!(
                    "Nucleus Co-op's window could not be read: {error:#}"
                ))
            })?;
            if running {
                window
                    .close(CLOSE_WAIT)
                    .map_err(|error| Refusal::busy(format!("{error:#}")))?;
                *opened = None;
                return Ok(true);
            }
            *opened = None;
        }
        match self.pc().running() {
            Ok(Some(pid)) => Err(Refusal::busy(format!(
                "Nucleus Co-op (process {pid}) was not opened from this console: close it on the PC"
            ))),
            Ok(None) => Ok(false),
            Err(error) => Err(Refusal::unavailable(format!(
                "Nucleus Co-op's processes could not be listed: {error:#}"
            ))),
        }
    }

    /// Before a session starts Nucleus Co-op: close the window the console
    /// opened. One that won't close refuses the session, loudly, as a
    /// Nucleus Co-op the host didn't start does.
    fn close_for_session(&self, title: &str) -> Result<(), Refusal> {
        let mut opened = self.opened.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(window) = opened.as_ref() else {
            return Ok(());
        };
        let pid = window.pid();
        let refuse = |error: anyhow::Error| {
            tracing::warn!(pid, error = %format!("{error:#}"), "Nucleus Co-op: the console's window did not close for a session");
            Refusal::unavailable(format!("Nucleus Co-op did not start {title}: {error:#}"))
        };
        let running = window
            .running()
            .with_context(|| format!("Nucleus Co-op's window (process {pid}) could not be read"));
        if !running.map_err(refuse)? {
            *opened = None;
            return Ok(());
        }
        window.close(CLOSE_WAIT).map_err(refuse)?;
        *opened = None;
        Ok(())
    }

    /// Each app's co-op status, in the order of `apps`; None while Nucleus
    /// Co-op is not installed, and the error while its handlers and games
    /// don't load. The handlers are read as the library reads them, so a
    /// handler added in Nucleus Co-op shows within [`CATALOG_TTL`].
    pub fn games(&self, apps: &[App], config: &Config) -> Option<Result<Vec<GameStatus>, String>> {
        let configured = configured(config);
        let folder = self.pc().installed(&configured).ok().flatten()?;
        let catalog = match self.catalog(&folder, false) {
            Ok(catalog) => catalog,
            Err(error) => return Some(Err(error)),
        };
        // What a background launch without the hub's handler answers.
        let own = |app: &App| -> Result<Option<Run>, Missing> {
            let run = self.own_status(app)?;
            own_list(&configured)?;
            Ok(run)
        };
        let ready = |handler: Handler| GameStatus::Ready {
            players: coop::players(
                handler.max_players,
                config.nucleus_coop_players(),
                coop::MAX_PLAYERS,
            )
            .unwrap_or(1),
            handler: handler.name,
        };
        // As the library lists them: a hidden entry is ready (its switch is
        // off), one whose id is taken is not.
        let statuses = listings(&catalog, apps)
            .into_iter()
            .zip(apps)
            .map(|(listing, app)| match listing {
                Listing::Listed(coop) => ready(coop.handler),
                Listing::Hidden(handler) => ready(handler),
                Listing::Taken => GameStatus::Taken {
                    entry: entry_name(&app.name),
                },
                // Shown as a background launch would answer it: one without
                // the user's added game streams with ours.
                Listing::Missing(missing @ Missing::NoHandler { .. }) => match own(app) {
                    Ok(Some(run)) => GameStatus::Own { missing, run },
                    Ok(None) => GameStatus::Missing(missing),
                    Err(refused) => GameStatus::Missing(refused),
                },
                // Adding the game makes it ready, for co-op and background
                // streams alike, through the hub handler: only what adding
                // can't fix (no .exe, one not on this PC, administrator
                // rights the account lacks) says otherwise.
                Listing::Missing(missing @ Missing::NotAdded { .. }) => {
                    match self.own_status(app) {
                        Err(refused @ (Missing::NoExecutable { .. } | Missing::Refused { .. })) => {
                            GameStatus::Missing(refused)
                        }
                        Ok(Some(run)) if own_list(&configured).is_ok() => {
                            GameStatus::Own { missing, run }
                        }
                        _ => GameStatus::Missing(missing),
                    }
                }
                Listing::Missing(missing) => GameStatus::Missing(missing),
            })
            .collect();
        Some(Ok(statuses))
    }

    /// [`own_status`] for the console, read again once [`OWN_STATUS_TTL`]
    /// passed or the .exe changed; one Rubylight could not tell is read
    /// again at once. The path is checked before anything opens it, and
    /// nothing else waits while the answer is worked out.
    fn own_status(&self, app: &App) -> Result<Option<Run>, Missing> {
        let Some(exe) = executable(app)
            .filter(|_| starts_exe(app))
            .filter(|exe| Path::new(exe).is_absolute())
        else {
            return own_status(self.pc().as_ref(), app).map_err(|no| no.missing);
        };
        refuse_path(exe)?;
        let modified = std::fs::metadata(exe)
            .and_then(|metadata| metadata.modified())
            .ok();
        let fresh = |status: &&OwnStatus| {
            status.modified == modified && status.at.elapsed() < OWN_STATUS_TTL
        };
        if let Some(status) = self
            .own_statuses
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(exe)
            .filter(fresh)
        {
            return status.status.clone();
        }
        let status = own_status(self.pc().as_ref(), app);
        let lasting = status.as_ref().err().is_none_or(|no| no.lasting);
        let status = status.map_err(|no| no.missing);
        let mut statuses = self
            .own_statuses
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        statuses.retain(|_, status| status.at.elapsed() < OWN_STATUS_TTL);
        if lasting {
            statuses.insert(
                exe.to_string(),
                OwnStatus {
                    at: Instant::now(),
                    modified,
                    status: status.clone(),
                },
            );
        } else {
            // Asked again at the next refresh, not a minute later.
            statuses.remove(exe);
        }
        status
    }

    /// Admit a launch: start a session of its game, or seat the device in
    /// the running one. The launch then holds its slot until
    /// [`leave`](Self::leave). A background stream's refusal names the app
    /// and why it can't stream in the background; it never runs another way.
    pub fn admit(&self, request: Admission) -> Result<Seat, Refusal> {
        let game = request.game;
        if game.kind() == Kind::Coop {
            return self.admit_game(request);
        }
        log_background_mapping();
        // A value earlier builds wrote is Nucleus Co-op too, until the next save replaces it.
        let setting = request.config.get(mode::KEY, "");
        let admitted = match mode::parse(setting) {
            Err(reason) if !mode::retired_background(setting) => Err(Refusal::unavailable(reason)),
            _ => self.admit_game(request),
        };
        admitted.map_err(|refusal| Refusal {
            message: format!(
                "{} can't stream in the background: {}",
                game.app().name,
                refusal.message
            ),
            ..refusal
        })
    }

    /// The game of the running background stream of app `app_id` that
    /// `device` plays in, which it returns to.
    fn returning(&self, app_id: u32, device: &str) -> Option<String> {
        self.lock()
            .sessions
            .values()
            .filter(|entry| entry.app_id == app_id)
            .find_map(|entry| {
                entry.instance.session(|s| {
                    (!s.ended() && s.kind == Kind::Background && s.slot_of(device).is_some())
                        .then(|| s.game.clone())
                })
            })
    }

    fn admit_game(&self, request: Admission) -> Result<Seat, Refusal> {
        let pc = self.pc();
        if let Some(why) = pc.unavailable() {
            return Err(Refusal::unavailable(why));
        }
        let kind = request.game.kind();
        let _one = self.admit.lock().unwrap_or_else(PoisonError::into_inner);
        let configured = configured(request.config);
        let folder = self.ensure(pc.as_ref(), &configured)?;
        let known_list = own_list(&configured).is_ok();
        let catalog = self.catalog(&folder, true).map_err(|error| {
            Refusal::unavailable(format!(
                "Nucleus Co-op's handlers and games did not load: {error}"
            ))
        })?;
        let app = request.game.app();
        let client = request.client;
        stop(self.reap());
        let (found, returning) = if kind == Kind::Background {
            // A hub handler only for the game the user added at exactly this
            // path: a file-name match is another game's (Portal's hl2.exe is
            // not Half-Life 2). Any other one-player game streams in the
            // background with ours. Its owner returning to it finds the
            // session's game: no Steam read or folder scan again.
            match executable(app).and_then(|exe| catalog.added(exe)) {
                Some(found) => (Some(Chosen::Hub(found)), None),
                None => match self.returning(request.game.app_id(), &client.uuid) {
                    Some(game) => (None, Some(game)),
                    None => (
                        Some(own_handler(pc.as_ref(), &catalog, app, &configured)?),
                        None,
                    ),
                },
            }
        } else {
            match catalog.find(&app.name, executable(app)) {
                Ok(found) => (Some(Chosen::Hub(found)), None),
                Err(missing) => return Err(Refusal::missing(&missing)),
            }
        };
        let game = returning
            .or_else(|| found.as_ref().map(Chosen::guid))
            .unwrap_or_default();
        let decision = {
            let state = self.lock();
            let current = state
                .sessions
                .values()
                .find_map(|entry| entry.instance.session(|s| (!s.ended()).then(|| s.clone())));
            coop::admit(current.as_ref(), &game, kind, &client.uuid)
        };
        match decision {
            Admit::Busy(text) => Err(Refusal::busy(text)),
            Admit::Join(id) => {
                let mut state = self.lock();
                let entry = state.sessions.get_mut(&id).ok_or_else(|| {
                    Refusal::busy(format!("The {} session ended: launch it again", app.name))
                })?;
                let slot = entry
                    .instance
                    .session(|s| s.join(&client.uuid, &client.name, client.allows_coop_join()))
                    .map_err(|refused| Refusal::busy(refused.to_string()))?
                    .slot();
                let holder = entry.holders.get_mut(slot).ok_or_else(|| {
                    Refusal::unavailable(format!("the session has no slot {slot}"))
                })?;
                *holder = Some(request.launch.into());
                tracing::info!(session = id, ?kind, slot, client = %client.name, title = %app.name, "Nucleus Co-op: a player joined");
                Ok(Seat { session: id, slot })
            }
            Admit::Start => {
                // The session returned to ended meanwhile.
                let Some(found) = found else {
                    return Err(Refusal::busy(format!(
                        "The {} session ended: launch it again",
                        app.name
                    )));
                };
                let free = pc.free_pads().map_err(|error| {
                    Refusal::unavailable(format!(
                        "Nucleus Co-op could not count the free controller slots: {error:#}"
                    ))
                })?;
                let players = match (kind, &found) {
                    (Kind::Coop, Chosen::Hub(found)) => coop::players(
                        found.handler.max_players,
                        request.config.nucleus_coop_players(),
                        free,
                    ),
                    _ => coop::players(1, 1, free),
                }
                .map_err(Refusal::unavailable)?;
                let policy = Policy::resolve(request.config)
                    .map_err(|error| Refusal::unavailable(format!("{error:#}")))?;
                let own = match &found {
                    Chosen::Own(setup) => Some(setup),
                    Chosen::Hub(_) => None,
                };
                let refused = |error: String| {
                    Refusal::unavailable(format!(
                        "Nucleus Co-op did not start {}: {error}",
                        app.name
                    ))
                };
                // A hub handler always runs elevated; ours as the user unless
                // it or the game needs administrator rights.
                let run = match own {
                    Some(setup) => run_of(pc.as_ref(), setup, true).map_err(String::from),
                    None => pc
                        .account()
                        .map_err(|error| format!("{error:#}"))
                        .and_then(|account| elevation::choose(Script::Hub, Need::Nothing, account)),
                }
                .map_err(refused)?;
                // Ours that a hub handler replaces or no game uses, but the
                // one this session starts with.
                let current = own.map(Setup::guid);
                let stale = catalog.stale(current.as_deref());
                pc.ready(&folder, run, own, &stale)
                    .map_err(|error| refused(format!("{error:#}")))?;
                // Last, so a launch refused for another reason leaves the
                // user's window open.
                self.close_for_session(&app.name)?;
                // Copied from the host's once Nucleus Co-op is closed: none of
                // its files changes while they are read.
                let copy = match run {
                    Run::User => Some(pc.user_copy(&folder).map_err(|error| {
                        refused(format!(
                            "the signed-in user's own copy of Nucleus Co-op was not made: {error:#}"
                        ))
                    })?),
                    Run::Elevated => None,
                };
                // They go while Nucleus is closed. A failure leaves a handler
                // that is never chosen and is tried again next time: the
                // stream goes on, and its player sees why.
                let mut kept = Vec::new();
                let mut why = String::new();
                for own in stale {
                    if let Err(error) = pc.remove_own(&folder, copy.as_deref(), own) {
                        tracing::error!(file = own.file, error = %format!("{error:#}"), "Nucleus Co-op: Rubylight's replaced or unused handler was not fully removed");
                        kept.push(own.file.as_str());
                        why = format!("{error:#}");
                    }
                }
                match kept.as_slice() {
                    [] => {}
                    [file] => request.warnings.set(
                        HANDLER_NOT_REMOVED,
                        format!(
                            "Rubylight's Nucleus Co-op handler {file} was not fully removed: {why}"
                        ),
                    ),
                    files => request.warnings.set(
                        HANDLER_NOT_REMOVED,
                        format!(
                            "Rubylight's Nucleus Co-op handlers {} were not fully removed: {why}",
                            files.join(", ")
                        ),
                    ),
                }
                let exe_path = match &found {
                    Chosen::Hub(found) => found.exe_path.clone(),
                    Chosen::Own(setup) => setup.exe_path.clone(),
                };
                let (handler, steam_app) = match found {
                    Chosen::Hub(found) => (found.handler, None),
                    Chosen::Own(setup) => (
                        pc.write_own(&folder, copy.as_deref(), &setup).map_err(|error| {
                            Refusal::unavailable(format!(
                                "Rubylight's Nucleus Co-op handler for {} was not written: {error:#}",
                                app.name
                            ))
                        })?,
                        setup.steam_app,
                    ),
                };
                let id = {
                    let mut state = self.lock();
                    state.next += 1;
                    state.next
                };
                let start = Start {
                    id,
                    kind,
                    folder: &folder,
                    user_copy: copy.as_deref(),
                    known_list,
                    handler: &handler,
                    exe_path: &exe_path,
                    steam_app,
                    title: &app.name,
                    players,
                    owner: (&client.uuid, &client.name),
                    size: request.size,
                };
                let instance: Arc<dyn Instance> = pc
                    .start(&start, policy, request.warnings)
                    .map_err(|error| refused(format!("{error:#}")))?
                    .into();
                let mut holders = vec![None; players];
                holders[0] = Some(request.launch.into());
                self.lock().sessions.insert(
                    id,
                    Entry {
                        instance,
                        app_id: request.game.app_id(),
                        holders,
                        feedback: (0..players).map(|_| Vec::new()).collect(),
                        ended: None,
                    },
                );
                tracing::info!(session = id, ?kind, players, game = %handler.guid, own = handler.own, ?run, client = %client.name, title = %app.name, "Nucleus Co-op: session started");
                Ok(Seat {
                    session: id,
                    slot: 0,
                })
            }
        }
    }

    /// The session's instance while the registry has it.
    pub fn instance(&self, session: u32) -> Option<Arc<dyn Instance>> {
        self.lock()
            .sessions
            .get(&session)
            .map(|entry| entry.instance.clone())
    }

    /// The feedback for `seat`'s player since its last call. One read of
    /// the pads serves every slot; each slot keeps the last
    /// [`FEEDBACK_KEPT`] reports until its stream takes them.
    pub fn feedback(&self, seat: Seat) -> Vec<PadReport> {
        let Some(instance) = self.instance(seat.session) else {
            return Vec::new();
        };
        let reports = instance.feedback();
        let mut state = self.lock();
        let Some(entry) = state.sessions.get_mut(&seat.session) else {
            return Vec::new();
        };
        for (slot, report) in reports {
            if let Some(kept) = entry.feedback.get_mut(slot) {
                if kept.len() == FEEDBACK_KEPT {
                    kept.remove(0);
                }
                kept.push(report);
            }
        }
        entry
            .feedback
            .get_mut(seat.slot)
            .map(std::mem::take)
            .unwrap_or_default()
    }

    /// The stream of `launch` ended or its pending launch expired: the device
    /// keeps its slot for [`coop::GRACE`]. A launch that no longer holds the
    /// slot leaves nothing.
    pub fn leave(&self, seat: Seat, launch: &str) {
        {
            let mut state = self.lock();
            if let Some(entry) = state.sessions.get_mut(&seat.session)
                && let Some(holder) = entry.holders.get_mut(seat.slot)
                && holder.as_deref() == Some(launch)
            {
                *holder = None;
                entry.instance.session(|s| {
                    let device = s
                        .slots
                        .get(seat.slot)
                        .and_then(|slot| slot.seat.as_ref())
                        .map(|seat| seat.device.clone());
                    if let Some(device) = device {
                        s.leave(&device, Instant::now());
                    }
                });
                tracing::info!(
                    session = seat.session,
                    slot = seat.slot,
                    "Nucleus Co-op: a player's stream ended"
                );
            }
        }
        stop(self.reap());
    }

    /// Seat `client` in `session`, or return it to its own slot, from its
    /// session list. The slot waits [`coop::GRACE`] for the device to launch
    /// the entry, unless the device streams it now. Returns the slot and the
    /// entry's app id.
    pub fn join(&self, session: u32, client: &Client) -> Result<(usize, u32), JoinError> {
        stop(self.reap());
        let state = self.lock();
        let entry = state.sessions.get(&session).ok_or(JoinError::NoSession)?;
        // A background stream is private: its id is no session to join, and
        // its refusals would name the game.
        if entry.instance.session(|s| s.kind) != Kind::Coop {
            return Err(JoinError::NoSession);
        }
        let slot = entry
            .instance
            .session(|s| {
                let slot = s
                    .join(&client.uuid, &client.name, client.allows_coop_join())?
                    .slot();
                if entry.holders.get(slot).is_none_or(Option::is_none) {
                    s.leave(&client.uuid, Instant::now());
                }
                Ok(slot)
            })
            .map_err(JoinError::Refused)?;
        tracing::info!(session, slot, client = %client.name, "Nucleus Co-op: a device took a slot from its session list");
        Ok((slot, entry.app_id))
    }

    /// Remove the player of `slot`; they cannot rejoin this session.
    /// Returns the device.
    pub fn kick(&self, session: u32, slot: usize) -> Option<String> {
        let mut state = self.lock();
        let entry = state.sessions.get_mut(&session)?;
        let device = entry.instance.session(|s| s.kick(slot))?;
        if let Some(holder) = entry.holders.get_mut(slot) {
            *holder = None;
        }
        tracing::info!(
            session,
            slot,
            device,
            "Nucleus Co-op: player removed in the web console"
        );
        Some(device)
    }

    /// End `session`: Nucleus and every instance stop. False when there is
    /// no such session.
    pub fn end(&self, session: u32) -> bool {
        let instance = {
            let state = self.lock();
            let Some(entry) = state.sessions.get(&session) else {
                return false;
            };
            entry.instance.session(|s| s.end(End::Ended));
            entry.instance.clone()
        };
        tracing::info!(session, "Nucleus Co-op: session ended");
        instance.stop();
        stop(self.reap());
        true
    }

    /// Every session the registry has, ended ones until they are reaped.
    pub fn sessions(&self) -> Vec<Listed> {
        stop(self.reap());
        self.lock()
            .sessions
            .values()
            .map(|entry| Listed {
                app_id: entry.app_id,
                session: entry.instance.session(|s| s.clone()),
                held: entry.holders.iter().map(Option::is_some).collect(),
            })
            .collect()
    }

    /// The instances of sessions newly seen ended, for the caller to stop
    /// outside the lock; drops sessions ended for [`LINGER`] that no stream
    /// holds.
    fn reap(&self) -> Vec<Arc<dyn Instance>> {
        let now = Instant::now();
        let mut stopping = Vec::new();
        self.lock().sessions.retain(|_, entry| {
            if !entry.instance.session(|s| s.ended()) {
                return true;
            }
            let since = *entry.ended.get_or_insert_with(|| {
                stopping.push(entry.instance.clone());
                now
            });
            entry.holders.iter().any(Option::is_some) || now.duration_since(since) < LINGER
        });
        stopping
    }
}

/// The first background stream says once that `background_stream` and
/// `background-stream` now run in Nucleus Co-op.
fn log_background_mapping() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        tracing::info!(
            "Nucleus Co-op: background_stream (device) and background-stream (app) now run background streams in Nucleus Co-op, which needs a handler for the game"
        );
    });
}

fn stop(instances: Vec<Arc<dyn Instance>>) {
    for instance in instances {
        instance.stop();
    }
}
