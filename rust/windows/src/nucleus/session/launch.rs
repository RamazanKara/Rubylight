//! Starting Nucleus Co-op's process: elevated or from the signed-in user's own
//! copy for a session, elevated for the console's window, and its environment.
use super::*;

/// What Nucleus Co-op starts with for a game found from Steam app
/// `steam_app`. A Steam game started by its .exe restarts through Steam
/// unless Steam started it; Steam's own launch sets these, and Nucleus
/// starts a handler's game with its own environment unless the handler sets
/// `UseNucleusEnvironment`.
pub fn steam_environment(steam_app: Option<u32>) -> BTreeMap<String, String> {
    steam_app
        .into_iter()
        .flat_map(|app| ["SteamAppId", "SteamGameId"].map(|key| (key.to_string(), app.to_string())))
        .collect()
}

/// Nucleus Co-op as the signed-in user, elevated: it makes symbolic links.
pub struct Elevated;
impl Launcher for Elevated {
    fn launch(
        &mut self,
        folder: &Path,
        guid: &str,
        extra: &BTreeMap<String, String>,
    ) -> Result<Box<dyn Nucleus>> {
        require_administrator(crate::process::user_account()?, Refused::Game)?;
        started(
            &launch_of(folder, guid, &environment(extra)?, Run::Elevated),
            None,
        )
    }
}

/// Nucleus Co-op from the signed-in user's own copy
/// ([`super::user_copy`]), as them with their standard rights: Rubylight's
/// own handler for a game that needs no administrator rights. The same
/// environment, job and window as [`Elevated`]'s; its files are written as
/// the user.
pub struct Unelevated {
    user: OwnedHandle,
}
impl Unelevated {
    pub fn signed_in() -> Result<Self> {
        Ok(Self {
            user: super::user_copy::user()?,
        })
    }
}
impl Launcher for Unelevated {
    fn launch(
        &mut self,
        folder: &Path,
        guid: &str,
        extra: &BTreeMap<String, String>,
    ) -> Result<Box<dyn Nucleus>> {
        let launch = launch_of(folder, guid, &environment(extra)?, Run::User);
        // Windows opens the program and its folder as the host: held, the
        // user can't swap either for a link until it runs. It starts only as
        // the account whose copy is held.
        let sid = super::user_copy::sid(&self.user)?;
        let _held = super::user_copy::hold(folder, &sid)?;
        started(&launch, Some(&sid))
    }
    fn write(&self, work: &mut dyn FnMut() -> Result<()>) -> Result<()> {
        super::user_copy::as_user(Some(&self.user), work)?
    }
    fn reader(&self) -> Result<Option<OwnedHandle>> {
        let user = self
            .user
            .try_clone()
            .context("keeping the signed-in user's token")?;
        Ok(Some(user))
    }
}

/// How a session starts the Nucleus Co-op at `folder`: as the signed-in
/// user, `elevated` or not, in its folder.
#[derive(Debug, PartialEq)]
pub(super) struct Launch {
    pub(super) program: PathBuf,
    pub(super) folder: PathBuf,
    pub(super) arguments: Vec<std::ffi::OsString>,
    pub(super) elevated: bool,
    pub(super) environment: BTreeMap<String, String>,
}

/// [`Launch`] of game `guid` for `run`, with `environment` ([`environment`]).
/// Nucleus reads Settings.ini from its working folder, and its window must
/// be visible for the profile click.
pub(super) fn launch_of(
    folder: &Path,
    guid: &str,
    environment: &BTreeMap<String, String>,
    run: Run,
) -> Launch {
    Launch {
        program: folder.join(EXECUTABLE),
        folder: folder.to_path_buf(),
        arguments: vec![guid.into()],
        elevated: run == Run::Elevated,
        environment: environment.clone(),
    }
}

/// Start `launch`; as the account of SID `account` only, when given.
pub(super) fn started(launch: &Launch, account: Option<&str>) -> Result<Box<dyn Nucleus>> {
    let since = crate::process::now();
    let target = match account {
        Some(sid) => crate::process::Target::Account(sid.to_string()),
        None => crate::process::Target::User {
            elevated: launch.elevated,
        },
    };
    let process = crate::process::Process::spawn(
        &launch.program,
        &launch.arguments,
        Some(&launch.folder),
        target,
        &launch.environment,
        false,
    )
    .with_context(|| {
        format!(
            "starting {} {}",
            launch.program.display(),
            if launch.elevated {
                "elevated"
            } else {
                "without administrator rights"
            }
        )
    })?;
    let elevated = process.elevated();
    tracing::info!(
        pid = process.pid,
        arguments = ?launch.arguments,
        asked = if launch.elevated { "elevated" } else { "user" },
        rights = match &elevated {
            Ok(true) => "administrator",
            Ok(false) => "standard",
            Err(_) => "unknown",
        },
        environment = ?launch.environment,
        "Nucleus Co-op started"
    );
    if let Some(note) = rights_note(launch.elevated, &elevated) {
        tracing::warn!(pid = process.pid, "{note}");
    }
    Ok(Box::new(Started { process, since }))
}

/// What the log adds when Nucleus Co-op's rights are not what its launch
/// asked for (`asked`: elevated), or can't be read.
pub(super) fn rights_note(asked: bool, elevated: &Result<bool>) -> Option<String> {
    match (asked, elevated) {
        (false, Ok(true)) => Some(
            "Nucleus Co-op: started as the signed-in user, it runs with administrator rights all the same: Windows gives this account only its full token (User Account Control is off)".into(),
        ),
        (true, Ok(false)) => Some(
            "Nucleus Co-op: started elevated, it runs without administrator rights".into(),
        ),
        (_, Err(error)) => Some(format!(
            "Nucleus Co-op: Rubylight could not read whether it runs with administrator rights: {error:#}"
        )),
        _ => None,
    }
}

/// Open Nucleus Co-op's own window on the PC's screen, as the signed-in user
/// and elevated as a session starts it, for the user to add a handler or a
/// game. It runs outside the host's job, so the host ending does not close
/// it; the [`Opened`] it returns closes it before a session starts.
pub fn open(folder: &Path) -> Result<Opened> {
    require_administrator(crate::process::user_account()?, Refused::Open)?;
    let program = folder.join(EXECUTABLE);
    let line = crate::process::quote(program.as_os_str())?;
    let process = crate::process::Process::spawn_command(
        &program,
        &line,
        Some(folder),
        crate::process::Target::User { elevated: true },
        &environment(&BTreeMap::new())?,
        false,
        true,
    )
    .with_context(|| format!("starting {} elevated", program.display()))?;
    tracing::info!(pid = process.pid, folder = %folder.display(), "Nucleus Co-op opened for the user");
    Ok(Opened(process))
}

/// WebView2 starts its browser without administrator rights even for an
/// elevated Nucleus Co-op, so that browser could not write the hub view's
/// data folder (`webview\cache` in Nucleus' Administrators-only folder) and
/// Nucleus stayed on its loading screen with "Microsoft Edge can't read and
/// write to its data directory" (#59). `WEBVIEW2_USER_DATA_FOLDER` replaces
/// the folder Nucleus passes: one in the signed-in user's own AppData\Local.
/// `extra` (a Steam game's app id) is added to it.
pub(super) fn environment(extra: &BTreeMap<String, String>) -> Result<BTreeMap<String, String>> {
    let local = crate::child_session::console_local_app_data()?
        .context("Nucleus Co-op needs a Windows account signed in at this PC's screen")?;
    Ok(webview_environment(&local, extra))
}
pub(super) fn webview_environment(
    local_app_data: &Path,
    extra: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let folder = local_app_data.join("Butterpollo").join("nucleus-webview");
    let mut environment = BTreeMap::from([(
        "WEBVIEW2_USER_DATA_FOLDER".to_string(),
        folder.to_string_lossy().into_owned(),
    )]);
    environment.extend(extra.clone());
    environment
}

/// Without an elevated token for the signed-in user (a standard account, or
/// Windows' Administrator protection, which elevates only into a separate
/// account after a Windows Hello prompt) Nucleus Co-op would run with
/// standard rights: it can't write to
/// its folder, which only administrators can change, and stops on its
/// loading screen ("Microsoft Edge can't read and write to its data
/// directory").
pub(super) fn require_administrator(account: Account, refused: Refused) -> Result<()> {
    ensure!(account.elevates(), "{}", refusal(account, refused));
    Ok(())
}

fn refusal(account: Account, refused: Refused) -> String {
    format!(
        "Nucleus Co-op needs administrator rights, and {}",
        account.no_elevation(refused)
    )
}

/// What [`open`] refuses with when the signed-in `account` has no elevated
/// token, with what to do or what still runs; the console says the same
/// before the button is pressed.
pub fn needs_administrator(account: Account) -> String {
    refusal(account, Refused::Open)
}

/// The Nucleus Co-op [`open`] started: the only one the host closes. It is
/// every NucleusCoop.exe in the job [`open`] made, so one that restarted
/// itself (elevating, updating) is still this one; the job's members keep
/// their ids from naming other processes.
pub struct Opened(crate::process::Process);
impl Opened {
    /// The process [`open`] started.
    pub fn pid(&self) -> u32 {
        self.0.pid
    }
    /// Whether a NucleusCoop.exe of it still runs. What it started (a
    /// browser from a hub link, a game) is the user's, and doesn't count.
    pub fn running(&self) -> Result<bool> {
        self.0.running_image(EXECUTABLE)
    }
    /// Ask its NucleusCoop.exe windows to close and wait up to `timeout`.
    /// Nothing is ended, and what it started is left alone: one still open (a
    /// question it asks the user) is an error.
    pub fn close(&self, timeout: Duration) -> Result<()> {
        ensure!(
            self.0.close_image(EXECUTABLE, timeout)?,
            "Nucleus Co-op (process {}) did not close within {} s: close it on the PC",
            self.0.pid,
            timeout.as_secs()
        );
        tracing::info!(pid = self.0.pid, "Nucleus Co-op closed");
        Ok(())
    }
}

/// A Nucleus Co-op process running on the PC, whoever started it.
pub fn running() -> Result<Option<u32>> {
    Ok(crate::process::processes()?
        .into_iter()
        .find(|p| p.name.eq_ignore_ascii_case(EXECUTABLE))
        .map(|p| p.pid))
}

/// Why a session does not start while Nucleus Co-op process `pid` runs.
pub fn already_running(pid: u32) -> String {
    format!(
        "Nucleus Co-op is already running on the PC (process {pid}): close it, then start the stream again"
    )
}

/// Nucleus started by [`Elevated`]. Its job ends what it started when this
/// is dropped.
struct Started {
    process: crate::process::Process,
    since: u64,
}
impl Nucleus for Started {
    fn since(&self) -> u64 {
        self.since
    }
    fn exit_code(&mut self) -> Result<Option<u32>> {
        self.process.exit_code()
    }
    fn job(&mut self) -> Result<BTreeSet<u32>> {
        Ok(self.process.process_ids()?.into_iter().collect())
    }
    fn stop(&mut self, instances: &BTreeMap<u32, u64>) {
        if let Err(error) = self.process.stop_graceful(STOP_TIMEOUT) {
            tracing::warn!(error = format!("{error:#}"), "Nucleus Co-op did not stop");
        }
        // Instances Nucleus started outside its job.
        crate::process::stop_processes(instances, STOP_TIMEOUT);
    }
}
