//! This PC behind the [`Pc`] and [`Instance`] seams: `butterpollo_windows::nucleus`,
//! and the download of the pinned release.
use super::*;

/// This PC.
#[cfg_attr(test, allow(dead_code, reason = "tests never reach the real PC"))]
pub(super) struct OnPc;
impl Pc for OnPc {
    fn unavailable(&self) -> Option<String> {
        if !butterpollo_windows::process::is_system() {
            return Some("Nucleus Co-op sessions need the host to run as the Rubylight Windows service, which starts Nucleus Co-op as the signed-in user".into());
        }
        butterpollo_core::os_gate::missing(
            butterpollo_windows::display::windows_build(),
            butterpollo_core::os_gate::Feature::WgcWindow,
        )
    }
    fn installed(&self, configured: &str) -> Result<Option<PathBuf>> {
        install::installed(&butterpollo_core::paths::data_root(), configured)
    }
    fn ensure(&self, configured: &str, progress: &(dyn Fn(u64) + Sync)) -> Result<PathBuf> {
        let mut download = Download(
            tokio::runtime::Handle::try_current()
                .context("downloading Nucleus Co-op needs the host's runtime")?,
            progress,
        );
        install::ensure(
            &butterpollo_core::paths::data_root(),
            configured,
            &mut download,
        )
    }
    fn open(&self, folder: &Path) -> Result<Box<dyn Window>> {
        Ok(Box::new(butterpollo_windows::nucleus::session::open(
            folder,
        )?))
    }
    fn running(&self) -> Result<Option<u32>> {
        butterpollo_windows::nucleus::session::running()
    }
    fn catalog(&self, folder: &Path) -> Result<Catalog> {
        // A nucleus_path copy may be a release that reads the AppData list.
        let installed =
            butterpollo_core::nucleus::install::folder(&butterpollo_core::paths::data_root());
        let appdata = if folder == installed {
            None
        } else {
            Some(
                butterpollo_windows::user_folder::roaming_app_data()
                    .map_err(|error| anyhow::anyhow!("{error}"))
                    .context("finding the signed-in user's AppData, whose Nucleus Co-op game list counts for nucleus_path")?,
            )
        };
        files::catalog(folder, true, appdata.as_deref())
    }
    fn free_pads(&self) -> Result<usize> {
        pads::free(&mut pads::System::open()?)
    }
    fn one_player(&self, app: &App) -> Result<Setup, Refusal> {
        // A Steam game's link: the .exe Steam starts for it.
        if !starts_exe(app)
            && let Some(appid) = serde_json::to_value(app)
                .ok()
                .and_then(|app| butterpollo_core::steam::game_of(&app))
        {
            let roots = butterpollo_windows::steam::roots();
            let launch = butterpollo_core::steam::game_executable(appid, &roots)
                .map_err(Refusal::not_found)?;
            return Ok(Setup {
                anti_cheat: launch.vac.then_some("Valve Anti-Cheat"),
                arguments: steam_arguments(&launch.arguments, command(app).unwrap_or_default()),
                ..Setup::new(
                    &launch.exe.to_string_lossy(),
                    &app.name,
                    Some((appid, &launch.install_dir.to_string_lossy())),
                )
            });
        }
        let exe = executable(app)
            .filter(|_| starts_exe(app))
            .filter(|exe| Path::new(exe).is_absolute())
            .ok_or_else(|| {
                Refusal::missing(&Missing::NoExecutable {
                    app: app.name.clone(),
                })
            })?;
        refuse_path(exe).map_err(|missing| Refusal::missing(&missing))?;
        if !Path::new(exe).is_file() {
            return Err(Refusal::not_found(not_on_this_pc(exe)));
        }
        // An .exe in a Steam library is its Steam app's, as a link would be.
        let steam = butterpollo_core::steam::library_app(
            Path::new(exe),
            &arguments(app),
            &butterpollo_windows::steam::roots(),
        )
        .map_err(Refusal::not_found)?;
        Ok(match steam {
            Some(steam) => Setup {
                anti_cheat: steam.vac.then_some("Valve Anti-Cheat"),
                arguments: arguments(app),
                ..Setup::new(
                    exe,
                    &app.name,
                    Some((steam.appid, &steam.install_dir.to_string_lossy())),
                )
            },
            None => Setup {
                arguments: arguments(app),
                ..Setup::new(exe, &app.name, None)
            },
        })
    }
    fn account(&self) -> Result<Account> {
        butterpollo_windows::process::user_account()
    }
    fn game_need(&self, exe: &str, folder: bool) -> Result<Need> {
        // The game's folder is usually the user's to change: its files are
        // read as them, never as the host.
        let user = user_copy::user()?;
        let sid = user_copy::sid(&user)?;
        let exe = Path::new(exe);
        let need = elevation::most([
            butterpollo_windows::nucleus::elevation::game_need(exe, &sid, Some(&user))?,
            match exe.parent().filter(|_| folder) {
                Some(parent) => {
                    butterpollo_windows::nucleus::elevation::folder_need(parent, Some(&user))?
                }
                None => Need::Nothing,
            },
        ]);
        Ok(need)
    }
    fn user_copy(&self, folder: &Path) -> Result<PathBuf> {
        let user = user_copy::user()?;
        let sid = user_copy::sid(&user)?;
        user_copy::ensure(&butterpollo_core::paths::data_root(), folder, &sid, &user)
    }
    fn write_own(&self, folder: &Path, copy: Option<&Path>, setup: &Setup) -> Result<Handler> {
        if let Some(pid) = self.running()? {
            anyhow::bail!(butterpollo_windows::nucleus::session::already_running(pid));
        }
        let own = match copy {
            None => files::write_own(folder, setup)?,
            Some(copy) => user_copy::write_own(folder, copy, setup, Some(&user_copy::user()?))?,
        };
        // Earlier builds added their games to a list Nucleus Co-op never
        // reads; they go from it once. The list is inert, so a failure is
        // only logged.
        match butterpollo_windows::user_folder::roaming_app_data() {
            Ok(appdata) => {
                if let Err(error) = files::forget_appdata_games(&appdata) {
                    tracing::warn!(
                        error = format!("{error:#}"),
                        "Nucleus Co-op: Rubylight's games left in the AppData list it does not read"
                    );
                }
            }
            Err(error) => tracing::warn!(
                %error,
                "Nucleus Co-op: the AppData list it does not read was not checked for Rubylight's games"
            ),
        }
        Ok(own)
    }
    fn remove_own(&self, folder: &Path, copy: Option<&Path>, own: &Handler) -> Result<()> {
        if let Some(pid) = self.running()? {
            anyhow::bail!(butterpollo_windows::nucleus::session::already_running(pid));
        }
        files::remove_own(folder, own)?;
        match copy {
            Some(copy) => user_copy::remove_game(copy, &own.guid, Some(&user_copy::user()?)),
            None => Ok(()),
        }
    }
    fn ready(
        &self,
        folder: &Path,
        run: Run,
        own: Option<&Setup>,
        stale: &[&Handler],
    ) -> Result<()> {
        butterpollo_windows::nucleus::session::ready(folder, run)?;
        if own.is_some() || !stale.is_empty() {
            if let Some(setup) = own {
                files::check_own(folder, setup)?;
            }
            let guids: Vec<&str> = stale.iter().map(|h| h.guid.as_str()).collect();
            files::check_list(folder, own, &guids)?;
        }
        Ok(())
    }
    fn start(
        &self,
        start: &Start,
        policy: Policy,
        warnings: Arc<Warnings>,
    ) -> Result<Box<dyn Instance>> {
        let running = butterpollo_windows::nucleus::session::start_on_pc(start, policy, warnings)?;
        Ok(Box::new(OnPcInstance(running)))
    }
}

/// A session on this PC.
#[cfg_attr(test, allow(dead_code, reason = "tests never reach the real PC"))]
struct OnPcInstance(Running);
impl Instance for OnPcInstance {
    fn with_session(&self, f: &mut dyn FnMut(&mut coop::Session)) {
        self.0.with_session(f);
    }
    fn end(&self) -> Option<End> {
        self.0.with_session(|s| ended(s).cloned())
    }
    fn capture_target(&self, slot: usize, display: &str) -> Result<String> {
        self.0.capture_target(slot, display)
    }
    fn resize(&self, slot: usize, size: (u32, u32)) -> Result<()> {
        self.0.resize(slot, size)
    }
    fn input(&self, device: &str, event: Input) -> Result<(), Dropped> {
        self.0.input(device, event)
    }
    fn feedback(&self) -> Vec<(usize, PadReport)> {
        self.0.feedback()
    }
    fn audio(&self, slot: usize) -> Result<AudioProcess> {
        self.0.audio(slot)
    }
    fn stop(&self) {
        self.0.stop();
    }
}

/// Downloads with the updater's HTTPS client, which follows redirects to
/// GitHub's asset hosts only. Blocks, so never on the async runtime.
#[cfg_attr(test, allow(dead_code, reason = "tests never reach the real PC"))]
struct Download<'a>(tokio::runtime::Handle, &'a (dyn Fn(u64) + Sync));
impl install::Fetch for Download<'_> {
    fn fetch(&mut self, url: &str, limit: u64) -> Result<Vec<u8>> {
        self.0.block_on(async {
            let mut response = crate::updater::client(Duration::from_secs(600))?
                .get(url)
                .send()
                .await?
                .error_for_status()?;
            let mut body = Vec::new();
            while let Some(part) = response.chunk().await? {
                chunk(&mut body, &part, limit)?;
                (self.1)(body.len() as u64);
            }
            Ok(body)
        })
    }
}

/// Add `part` to `body`, refusing a body longer than `limit` bytes.
pub(super) fn chunk(body: &mut Vec<u8>, part: &[u8], limit: u64) -> Result<()> {
    ensure!(
        (body.len() as u64).saturating_add(part.len() as u64) <= limit,
        "the download is larger than its {limit} bytes"
    );
    body.extend_from_slice(part);
    Ok(())
}
