//! A PC with Nucleus Co-op and one handler, and sessions that only
//! keep their state.
use super::*;
use butterpollo_core::nucleus::handler::Game;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

pub(crate) const GUID: &str = "Portal 2";
pub(crate) const EXE: &str = r"C:\Games\Portal 2\portal2.exe";
/// The account signed in at the fake PC.
pub(crate) const USER: &str = "S-1-5-21-1-2-3-1001";

pub(crate) struct Pc {
    pub unavailable: Mutex<Option<String>>,
    pub installed: Mutex<Option<PathBuf>>,
    /// What reading the install fails with (a folder whose ACL is refused).
    pub installed_error: Mutex<Option<String>>,
    /// What an install fails with.
    pub install_error: Mutex<Option<String>>,
    /// Whether an install panics instead.
    pub install_panics: Mutex<bool>,
    /// What an install downloads, checked as the pinned release is;
    /// None installs without a download.
    pub archive: Mutex<Option<Vec<u8>>>,
    /// While true an install waits in the download, until `release`.
    pub hold: Mutex<bool>,
    pub release: std::sync::Condvar,
    /// How often Nucleus Co-op's window was opened.
    pub opened: AtomicUsize,
    /// The windows opened, the last one last.
    pub windows: Mutex<Vec<Arc<Window>>>,
    /// A Nucleus Co-op the host did not start.
    pub foreign: Mutex<Option<u32>>,
    pub catalog: Mutex<Catalog>,
    pub free: Mutex<usize>,
    pub start_error: Mutex<Option<String>>,
    pub started: Mutex<Vec<Arc<Instance>>>,
    /// The Steam app each started session's Nucleus got, in order.
    pub steam_apps: Mutex<Vec<Option<u32>>>,
    pub installs: AtomicUsize,
    pub reads: AtomicUsize,
    /// The game each app's .exe is for Rubylight's own handler, by app
    /// name; an app not here starts its command's .exe, as on the PC.
    pub one_player: Mutex<BTreeMap<String, Setup>>,
    /// Rubylight's own handlers written, the last one last.
    pub own: Mutex<Vec<Setup>>,
    pub write_error: Mutex<Option<String>>,
    /// What [`super::Pc::ready`] fails with.
    pub ready_error: Mutex<Option<String>>,
    /// Rubylight's stale handler files removed, in order.
    pub removed: Mutex<Vec<String>>,
    /// What [`super::Pc::remove_own`] fails with.
    pub remove_error: Mutex<Option<String>>,
    /// The stale handlers' files each [`super::Pc::ready`] got.
    pub ready_stale: Mutex<Vec<Vec<String>>>,
    /// The game's .exe each started session's Nucleus got, in order.
    pub exe_paths: Mutex<Vec<String>>,
    /// Whether Rubylight knew each started session's game list.
    pub known_lists: Mutex<Vec<bool>>,
    /// How often an app's game was looked up for Rubylight's handler.
    pub lookups: AtomicUsize,
    /// Whether the signed-in user has an elevated token, or why not.
    pub account: Mutex<Account>,
    /// Why reading that fails (nobody signed in).
    pub account_error: Mutex<Option<String>>,
    /// How often it was read.
    pub elevations: AtomicUsize,
    /// What each game's .exe asks of Windows, by path; Nothing else.
    pub needs: Mutex<BTreeMap<String, Need>>,
    /// What walking each game's folder adds, by .exe path.
    pub folder_needs: Mutex<BTreeMap<String, Need>>,
    /// How often the game's folder was walked.
    pub walks: AtomicUsize,
    /// What making the user's own copy fails with.
    pub copy_error: Mutex<Option<String>>,
    /// The run each [`super::Pc::ready`] got.
    pub ready_runs: Mutex<Vec<Run>>,
    /// The copy each own handler was written to (None: the host's).
    pub own_in: Mutex<Vec<Option<PathBuf>>>,
    /// The copy each started session ran (None: the host's, elevated).
    pub user_copies: Mutex<Vec<Option<PathBuf>>>,
    /// The Nucleus folder each user's copy was made from, and what of
    /// Nucleus Co-op ran then.
    pub copied_from: Mutex<Vec<PathBuf>>,
    pub copied_while: Mutex<Vec<Option<u32>>>,
    /// The user's copy each removal of Rubylight's handler cleaned too.
    pub removed_in: Mutex<Vec<Option<PathBuf>>>,
    /// Why reading a game's needs fails.
    pub need_error: Mutex<Option<String>>,
}
impl Pc {
    pub(crate) fn unavailable() -> Self {
        let pc = Self::installed();
        *pc.unavailable.lock().unwrap() = Some("not on this test PC".into());
        pc
    }
    /// Installed, with a two-player handler for portal2.exe and the
    /// game added.
    pub(crate) fn installed() -> Self {
        Self {
            unavailable: Mutex::new(None),
            installed: Mutex::new(Some(PathBuf::from(r"C:\Nucleus"))),
            installed_error: Mutex::new(None),
            install_error: Mutex::new(None),
            install_panics: Mutex::new(false),
            archive: Mutex::new(None),
            hold: Mutex::new(false),
            release: std::sync::Condvar::new(),
            opened: AtomicUsize::new(0),
            windows: Mutex::new(Vec::new()),
            foreign: Mutex::new(None),
            catalog: Mutex::new(Catalog {
                handlers: vec![Handler {
                    file: "Portal 2.js".into(),
                    guid: GUID.into(),
                    name: "Portal 2".into(),
                    executable: "portal2.exe".into(),
                    max_players: 2,
                    supports_keyboard: false,
                    hides_taskbar: false,
                    own: false,
                    owner: None,
                    admin: Vec::new(),
                }],
                games: vec![Game {
                    guid: GUID.into(),
                    exe_path: EXE.into(),
                }],
                user: Some(USER.into()),
            }),
            free: Mutex::new(4),
            start_error: Mutex::new(None),
            started: Mutex::new(Vec::new()),
            steam_apps: Mutex::new(Vec::new()),
            installs: AtomicUsize::new(0),
            reads: AtomicUsize::new(0),
            one_player: Mutex::new(BTreeMap::new()),
            own: Mutex::new(Vec::new()),
            write_error: Mutex::new(None),
            ready_error: Mutex::new(None),
            removed: Mutex::new(Vec::new()),
            remove_error: Mutex::new(None),
            ready_stale: Mutex::new(Vec::new()),
            exe_paths: Mutex::new(Vec::new()),
            known_lists: Mutex::new(Vec::new()),
            lookups: AtomicUsize::new(0),
            account: Mutex::new(Account::Elevates),
            account_error: Mutex::new(None),
            elevations: AtomicUsize::new(0),
            needs: Mutex::new(BTreeMap::new()),
            folder_needs: Mutex::new(BTreeMap::new()),
            walks: AtomicUsize::new(0),
            copy_error: Mutex::new(None),
            ready_runs: Mutex::new(Vec::new()),
            own_in: Mutex::new(Vec::new()),
            user_copies: Mutex::new(Vec::new()),
            copied_from: Mutex::new(Vec::new()),
            copied_while: Mutex::new(Vec::new()),
            removed_in: Mutex::new(Vec::new()),
            need_error: Mutex::new(None),
        }
    }
    /// The session started last.
    pub(crate) fn last(&self) -> Arc<Instance> {
        self.started.lock().unwrap().last().unwrap().clone()
    }
}
impl super::Pc for Pc {
    fn unavailable(&self) -> Option<String> {
        self.unavailable.lock().unwrap().clone()
    }
    fn installed(&self, configured: &str) -> Result<Option<PathBuf>> {
        if let Some(error) = self.installed_error.lock().unwrap().clone() {
            anyhow::bail!(error);
        }
        if !configured.is_empty() {
            return Ok(Some(PathBuf::from(configured)));
        }
        Ok(self.installed.lock().unwrap().clone())
    }
    fn ensure(&self, configured: &str, progress: &(dyn Fn(u64) + Sync)) -> Result<PathBuf> {
        if let Some(folder) = self.installed(configured)? {
            return Ok(folder);
        }
        self.installs.fetch_add(1, Ordering::Relaxed);
        progress(1);
        let mut hold = self.hold.lock().unwrap();
        while *hold {
            hold = self.release.wait(hold).unwrap();
        }
        drop(hold);
        if *self.install_panics.lock().unwrap() {
            panic!("the unpacker broke");
        }
        if let Some(error) = self.install_error.lock().unwrap().clone() {
            anyhow::bail!(error);
        }
        if let Some(archive) = self.archive.lock().unwrap().as_deref() {
            progress(archive.len() as u64);
            butterpollo_core::nucleus::install::verify(archive).map_err(anyhow::Error::msg)?;
        }
        let folder = PathBuf::from(r"C:\Nucleus");
        *self.installed.lock().unwrap() = Some(folder.clone());
        Ok(folder)
    }
    fn open(&self, _folder: &Path) -> Result<Box<dyn super::Window>> {
        let count = self.opened.fetch_add(1, Ordering::Relaxed);
        let window = Arc::new(Window {
            pid: 9000 + count as u32,
            open: AtomicBool::new(true),
            stuck: AtomicBool::new(false),
            unreadable: AtomicBool::new(false),
            restarted_as: Mutex::new(None),
            closes: AtomicUsize::new(0),
        });
        self.windows.lock().unwrap().push(window.clone());
        Ok(Box::new(window))
    }
    fn running(&self) -> Result<Option<u32>> {
        if let Some(pid) = *self.foreign.lock().unwrap() {
            return Ok(Some(pid));
        }
        Ok(self
            .windows
            .lock()
            .unwrap()
            .iter()
            .find(|window| window.open.load(Ordering::Relaxed))
            .map(|window| window.restarted_as.lock().unwrap().unwrap_or(window.pid)))
    }
    fn catalog(&self, _folder: &Path) -> Result<Catalog> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        Ok(self.catalog.lock().unwrap().clone())
    }
    fn free_pads(&self) -> Result<usize> {
        Ok(*self.free.lock().unwrap())
    }
    fn one_player(&self, app: &App) -> Result<Setup, Refusal> {
        self.lookups.fetch_add(1, Ordering::Relaxed);
        if let Some(setup) = self.one_player.lock().unwrap().get(&app.name) {
            return Ok(setup.clone());
        }
        match executable(app).filter(|_| starts_exe(app)) {
            Some(exe) if Path::new(exe).is_absolute() => Ok(Setup {
                arguments: arguments(app),
                ..Setup::new(exe, &app.name, None)
            }),
            _ => Err(Refusal::missing(&Missing::NoExecutable {
                app: app.name.clone(),
            })),
        }
    }
    fn account(&self) -> Result<Account> {
        self.elevations.fetch_add(1, Ordering::Relaxed);
        if let Some(error) = self.account_error.lock().unwrap().clone() {
            anyhow::bail!(error);
        }
        Ok(*self.account.lock().unwrap())
    }
    fn game_need(&self, exe: &str, folder: bool) -> Result<Need> {
        if let Some(error) = self.need_error.lock().unwrap().clone() {
            anyhow::bail!(error);
        }
        let need = |needs: &Mutex<BTreeMap<String, Need>>| {
            needs
                .lock()
                .unwrap()
                .get(exe)
                .copied()
                .unwrap_or(Need::Nothing)
        };
        if !folder {
            return Ok(need(&self.needs));
        }
        self.walks.fetch_add(1, Ordering::Relaxed);
        Ok(elevation::most([
            need(&self.needs),
            need(&self.folder_needs),
        ]))
    }
    fn user_copy(&self, folder: &Path) -> Result<PathBuf> {
        if let Some(error) = self.copy_error.lock().unwrap().clone() {
            anyhow::bail!(error);
        }
        self.copied_from.lock().unwrap().push(folder.to_path_buf());
        self.copied_while
            .lock()
            .unwrap()
            .push(super::Pc::running(self)?);
        Ok(PathBuf::from(format!(
            r"C:\ProgramData\Butterpollo\nucleus-user\{USER}\0123456789abcdef"
        )))
    }
    fn write_own(&self, _folder: &Path, copy: Option<&Path>, setup: &Setup) -> Result<Handler> {
        if let Some(error) = self.write_error.lock().unwrap().clone() {
            anyhow::bail!(error);
        }
        // As on the PC: never while Nucleus Co-op runs.
        if let Some(pid) = super::Pc::running(self)? {
            anyhow::bail!(butterpollo_windows::nucleus::session::already_running(pid));
        }
        self.own.lock().unwrap().push(setup.clone());
        self.own_in
            .lock()
            .unwrap()
            .push(copy.map(Path::to_path_buf));
        butterpollo_core::nucleus::handler::parse(&setup.file(), &setup.handler_of(Some(USER)))
            .map_err(anyhow::Error::msg)
    }
    fn remove_own(&self, _folder: &Path, copy: Option<&Path>, own: &Handler) -> Result<()> {
        if let Some(error) = self.remove_error.lock().unwrap().clone() {
            anyhow::bail!(error);
        }
        self.removed.lock().unwrap().push(own.file.clone());
        self.removed_in
            .lock()
            .unwrap()
            .push(copy.map(Path::to_path_buf));
        Ok(())
    }
    fn ready(
        &self,
        _folder: &Path,
        run: Run,
        _own: Option<&Setup>,
        stale: &[&Handler],
    ) -> Result<()> {
        self.ready_runs.lock().unwrap().push(run);
        self.ready_stale
            .lock()
            .unwrap()
            .push(stale.iter().map(|h| h.file.clone()).collect());
        match self.ready_error.lock().unwrap().clone() {
            Some(error) => anyhow::bail!(error),
            None => Ok(()),
        }
    }
    fn start(
        &self,
        start: &Start,
        _policy: Policy,
        _warnings: Arc<Warnings>,
    ) -> Result<Box<dyn super::Instance>> {
        if let Some(error) = self.start_error.lock().unwrap().clone() {
            anyhow::bail!(error);
        }
        // As the PC's session refuses while Nucleus Co-op runs.
        if let Some(pid) = super::Pc::running(self)? {
            anyhow::bail!(butterpollo_windows::nucleus::session::already_running(pid));
        }
        let pads: Vec<u8> = (0..start.players as u8).collect();
        let session = coop::Session::new(
            start.id,
            start.kind,
            &start.handler.guid,
            start.title,
            &pads,
            start.owner,
        )
        .map_err(anyhow::Error::msg)?;
        let instance = Arc::new(Instance {
            session: Mutex::new(session),
            stopped: AtomicBool::new(false),
            sizes: Mutex::new(vec![start.size; start.players]),
            inputs: Mutex::new(Vec::new()),
            reports: Mutex::new(Vec::new()),
        });
        self.started.lock().unwrap().push(instance.clone());
        self.steam_apps.lock().unwrap().push(start.steam_app);
        self.user_copies
            .lock()
            .unwrap()
            .push(start.user_copy.map(Path::to_path_buf));
        self.exe_paths
            .lock()
            .unwrap()
            .push(start.exe_path.to_string());
        self.known_lists.lock().unwrap().push(start.known_list);
        Ok(Box::new(instance))
    }
}

/// Nucleus Co-op's window as the user would leave it: open until closed,
/// or `stuck` on a question it asks.
pub(crate) struct Window {
    pub pid: u32,
    pub open: AtomicBool,
    pub stuck: AtomicBool,
    /// Whether reading it fails.
    pub unreadable: AtomicBool,
    /// The process id it runs as after restarting itself.
    pub restarted_as: Mutex<Option<u32>>,
    /// How often it was asked to close.
    pub closes: AtomicUsize,
}
impl Pc {
    /// The window opened last.
    pub(crate) fn window(&self) -> Arc<Window> {
        self.windows.lock().unwrap().last().unwrap().clone()
    }
}
impl super::Window for Arc<Window> {
    fn pid(&self) -> u32 {
        self.pid
    }
    fn running(&self) -> Result<bool> {
        anyhow::ensure!(!self.unreadable.load(Ordering::Relaxed), "access denied");
        Ok(self.open.load(Ordering::Relaxed))
    }
    fn close(&self, timeout: Duration) -> Result<()> {
        self.closes.fetch_add(1, Ordering::Relaxed);
        anyhow::ensure!(
            !self.stuck.load(Ordering::Relaxed),
            "Nucleus Co-op (process {}) did not close within {} s: close it on the PC",
            self.pid,
            timeout.as_secs()
        );
        self.open.store(false, Ordering::Relaxed);
        Ok(())
    }
}

/// A session that starts as Nucleus does, without windows until
/// [`Instance::show`]; its pads route as the real ones do.
pub(crate) struct Instance {
    pub session: Mutex<coop::Session>,
    pub stopped: AtomicBool,
    pub sizes: Mutex<Vec<(u32, u32)>>,
    /// What reached each slot's pad.
    pub inputs: Mutex<Vec<(usize, Input)>>,
    /// The pads' feedback the next read returns.
    pub reports: Mutex<Vec<(usize, PadReport)>>,
}
impl Instance {
    pub(crate) fn stopped(&self) -> bool {
        self.stopped.load(Ordering::Relaxed)
    }
    /// Every slot's window is there: slot k's is 100 + k.
    pub(crate) fn show(&self) {
        let mut session = self.session.lock().unwrap();
        let windows: Vec<_> = (0..session.slots.len())
            .map(|k| Some(100 + k as u64))
            .collect();
        session.running(&windows);
    }
}
impl super::Instance for Arc<Instance> {
    fn with_session(&self, f: &mut dyn FnMut(&mut coop::Session)) {
        f(&mut self.session.lock().unwrap());
    }
    fn end(&self) -> Option<End> {
        ended(&self.session.lock().unwrap()).cloned()
    }
    fn capture_target(&self, slot: usize, display: &str) -> Result<String> {
        let window = self
            .session
            .lock()
            .unwrap()
            .slots
            .get(slot)
            .context("no such slot")?
            .window
            .with_context(|| format!("Player {}'s game window is not open", slot + 1))?;
        Ok(format!("window:{window}@{display}"))
    }
    fn resize(&self, slot: usize, size: (u32, u32)) -> Result<()> {
        *self
            .sizes
            .lock()
            .unwrap()
            .get_mut(slot)
            .context("no such slot")? = size;
        Ok(())
    }
    fn input(&self, device: &str, event: Input) -> Result<(), Dropped> {
        let session = self.session.lock().unwrap();
        let slot = session
            .slot_of(device)
            .filter(|_| session.route(device).is_some())
            .ok_or(Dropped::Ignored)?;
        let event = pads::route(event, slot, session.slots.len())?;
        self.inputs.lock().unwrap().push((slot, event));
        Ok(())
    }
    fn feedback(&self) -> Vec<(usize, PadReport)> {
        std::mem::take(&mut self.reports.lock().unwrap())
    }
    fn audio(&self, slot: usize) -> Result<AudioProcess> {
        anyhow::bail!("the fake session has no audio for slot {slot}")
    }
    fn stop(&self) {
        self.stopped.store(true, Ordering::Relaxed);
        self.session.lock().unwrap().end(End::Ended);
    }
}
