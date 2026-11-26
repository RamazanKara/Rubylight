//! Local launcher discovery. Scanning never starts a launcher or changes its files.
use anyhow::{Context, Result};
use butterpollo_session::library::{self, Game, Launch, Source};
use serde::Serialize;
#[cfg(test)]
use std::collections::BTreeMap;
use std::{
    io::Read,
    path::{Path, PathBuf},
};

mod launchers;
mod registry;
mod store;

#[cfg(test)]
use launchers::gog_registered;

#[derive(Serialize)]
pub struct SourceStatus {
    pub source: Source,
    pub name: &'static str,
    pub state: &'static str,
    pub errors: Vec<String>,
    /// Windows named no signed-in user, so the source's games were not read (Microsoft Store
    /// and Xbox at the sign-in screen): [`Discovery::skipped`], not "not installed".
    #[serde(skip)]
    pub signed_out: bool,
}
pub struct Discovery {
    pub sources: Vec<SourceStatus>,
    pub games: Vec<Game>,
}

fn bytes(path: &Path) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .with_context(|| format!("Reading {}", path.display()))?
        .take(16 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() <= 16 * 1024 * 1024,
        "Launcher record is too large: {}",
        path.display()
    );
    Ok(bytes)
}
fn files(folder: &Path, errors: &mut Vec<String>) -> Vec<PathBuf> {
    match std::fs::read_dir(folder) {
        Ok(entries) => entries
            .filter_map(|entry| match entry {
                Ok(entry) => Some(entry.path()),
                Err(error) => {
                    errors.push(format!("Reading {}: {error}", folder.display()));
                    None
                }
            })
            .collect(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => vec![],
        Err(error) => {
            errors.push(format!("Reading {}: {error}", folder.display()));
            vec![]
        }
    }
}
/// Whether `path` sits on a local drive of this PC (never touching network shares as SYSTEM).
pub(super) fn on_this_pc(path: &Path) -> bool {
    butterpollo_session::nucleus::generic::on_this_pc(&path.to_string_lossy()).is_some()
}
fn add(result: Result<Option<Game>>, games: &mut Vec<Game>, errors: &mut Vec<String>) {
    match result {
        Ok(Some(mut game)) => {
            let install_dir = Path::new(&game.install_dir);
            if game.source != Source::Local && !on_this_pc(install_dir) {
                return;
            }
            if !install_dir.is_dir() {
                return;
            }
            if game.cover.is_none() {
                game.cover = ["cover.png", "cover.jpg", "folder.jpg"]
                    .iter()
                    .map(|file| install_dir.join(file))
                    .find(|file| file.is_file());
            }
            games.push(game);
        }
        Ok(None) => {}
        // With its causes: the outermost context alone names only the record or package.
        Err(error) => errors.push(format!("{error:#}")),
    }
}

/// How many files and folders the scan looks at in one game folder.
const GAME_FOLDER_ENTRIES: usize = 2000;
/// The programs in a game folder and up to two folders below it, the top first, leaving
/// out the folders that never hold the game's own program, and not following links.
/// Whether the scan stopped at [`GAME_FOLDER_ENTRIES`] before it read them all.
fn programs(folder: &Path, errors: &mut Vec<String>) -> (Vec<library::Program>, bool) {
    let mut found = vec![];
    let mut pending = std::collections::VecDeque::from([(PathBuf::new(), 0)]);
    let mut seen = 0;
    while let Some((relative, depth)) = pending.pop_front() {
        for path in files(&folder.join(&relative), errors) {
            seen += 1;
            if seen > GAME_FOLDER_ENTRIES {
                return (found, true);
            }
            let (Some(name), Ok(metadata)) = (path.file_name(), std::fs::symlink_metadata(&path))
            else {
                continue;
            };
            let lower = name.to_string_lossy().to_lowercase();
            let relative = relative.join(name);
            if metadata.is_dir() {
                if depth < 2 && !library::LOCAL_SKIPPED_FOLDERS.contains(&lower.as_str()) {
                    pending.push_back((relative, depth + 1));
                }
            } else if metadata.is_file() && lower.ends_with(".exe") {
                found.push(library::Program {
                    path: relative,
                    size: metadata.len(),
                });
            }
        }
    }
    (found, false)
}

/// Why a configured game folder cannot be read, if it cannot. The host runs as a
/// service, which does not see the drive letters a user mapped.
fn unreadable_root(root: &Path) -> Option<String> {
    let shown = root.display();
    if !root.is_absolute() {
        return Some(format!("The game folder {shown} is not a full path."));
    }
    if root.is_dir() {
        return None;
    }
    if root.exists() {
        return Some(format!("The game folder {shown} is a file, not a folder."));
    }
    let drive = root.ancestors().last().unwrap_or(root);
    Some(if drive.exists() {
        format!("The game folder {shown} does not exist.")
    } else {
        format!(
            "The game folder {shown} is on a drive the host cannot see. A mapped network drive belongs to one user: enter its network path (\\\\server\\share) instead."
        )
    })
}
/// Folders a drive or a folder of games holds that are not games: hidden and system
/// folders, such as `$RECYCLE.BIN` and `System Volume Information`.
fn hidden_folder(folder: &Path) -> bool {
    use std::os::windows::fs::MetadataExt;
    const HIDDEN_OR_SYSTEM: u32 = 0x2 | 0x4;
    folder
        .file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with('$'))
        || std::fs::metadata(folder)
            .is_ok_and(|metadata| metadata.file_attributes() & HIDDEN_OR_SYSTEM != 0)
}

/// One game per subfolder of the configured game folders. A folder that is missing is an
/// error of its own; the other folders are still read.
fn local(settings: &library::Settings, games: &mut Vec<Game>, errors: &mut Vec<String>) {
    for root in &settings.local_folders {
        if let Some(error) = unreadable_root(root) {
            errors.push(error);
            continue;
        }
        for folder in files(root, errors) {
            if !folder.is_dir() || hidden_folder(&folder) || settings.excluded(&folder) {
                continue;
            }
            let (found, cut_short) = programs(&folder, errors);
            let game = library::local(&folder, &found);
            if cut_short {
                if game.is_none() {
                    errors.push(format!(
                        "No game program is among the first {GAME_FOLDER_ENTRIES} files in {}.",
                        folder.display()
                    ));
                } else {
                    // The top folders came first, so the program found is the top one.
                    tracing::debug!(folder = %folder.display(), "the game folder scan read only its first {GAME_FOLDER_ENTRIES} files");
                }
            }
            add(Ok(game), games, errors);
        }
    }
}

impl Discovery {
    /// The sources the scan did not read: turned off in the settings, or not readable while no
    /// user is signed in ([`Self::signed_out`]).
    pub fn skipped(&self) -> impl Iterator<Item = Source> + '_ {
        self.sources
            .iter()
            .filter(|status| status.state == "disabled" || status.signed_out)
            .map(|status| status.source)
    }
    /// The sources not read because Windows named no signed-in user: their games keep the state
    /// an earlier scan found (`Scan::carrying`).
    pub fn signed_out(&self) -> impl Iterator<Item = Source> + '_ {
        self.sources
            .iter()
            .filter(|status| status.signed_out)
            .map(|status| status.source)
    }
}

/// Runs `work` in the thread's background mode (lowest CPU, disk and memory priority), so reading
/// a library's files never competes with a running stream. Hold no lock in it: a thread that gets
/// no time would hold up whoever waits for the lock. The mode ends with `work`, also on a panic.
pub fn in_background<T>(work: impl FnOnce() -> T) -> T {
    use windows::Win32::System::Threading::{
        GetCurrentThread, SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN,
        THREAD_MODE_BACKGROUND_END,
    };
    struct Background {
        began: bool,
    }
    impl Drop for Background {
        fn drop(&mut self) {
            if self.began {
                // SAFETY: acts on the current thread's pseudo-handle and takes no pointers.
                unsafe {
                    let _ = SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_END);
                }
            }
        }
    }
    // SAFETY: acts on the current thread's pseudo-handle and takes no pointers. It fails when the
    // thread is in the mode already: the caller's mode then stays until the caller ends it.
    let began =
        unsafe { SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN) }.is_ok();
    let _background = Background { began };
    work()
}

pub fn discover(settings: &library::Settings) -> Discovery {
    let mut registry_errors = vec![];
    let installs = registry::installed(&mut registry_errors);
    let program_folders = launchers::program_folders();
    let program_data =
        PathBuf::from(std::env::var_os("PROGRAMDATA").unwrap_or_else(|| r"C:\ProgramData".into()));
    let mut sources = vec![];
    let mut games = vec![];
    for source in Source::ALL {
        if !settings.enabled(source) {
            sources.push(SourceStatus {
                source,
                name: source.name(),
                state: "disabled",
                errors: vec![],
                signed_out: false,
            });
            continue;
        }
        let mut errors = if matches!(
            source,
            Source::Steam | Source::Epic | Source::Microsoft | Source::Local
        ) {
            vec![]
        } else {
            registry_errors.clone()
        };
        let mut signed_out = false;
        let available = match source {
            Source::Steam => launchers::scan_steam(&mut games, &mut errors),
            Source::Epic => launchers::scan_epic(&program_data, &mut games, &mut errors),
            Source::Battlenet => launchers::scan_battlenet(
                &installs,
                &program_folders,
                &program_data,
                &mut games,
                &mut errors,
            ),
            Source::Ea => launchers::scan_ea(&installs, &program_folders, &mut games, &mut errors),
            Source::Ubisoft => launchers::scan_ubisoft(&installs, &mut games, &mut errors),
            Source::Gog => {
                launchers::scan_gog(&installs, &program_folders, &mut games, &mut errors)
            }
            Source::Local => {
                local(settings, &mut games, &mut errors);
                true
            }
            Source::Microsoft => {
                // Packages, not the registry: the scan reports its own errors.
                let scan = store::discover();
                signed_out = scan.signed_out;
                errors.extend(scan.errors);
                games.extend(scan.games);
                scan.available
            }
        };
        if !errors.is_empty() {
            // One launcher's trouble never hides the others: its games found so far stay.
            tracing::warn!(
                source = source.key(),
                ?errors,
                "the launcher scan is incomplete"
            );
        }
        sources.push(SourceStatus {
            source,
            name: source.name(),
            state: if !errors.is_empty() {
                "error"
            } else if available {
                "available"
            } else {
                "unavailable"
            },
            errors,
            signed_out,
        });
    }
    Discovery {
        sources,
        games: library::finish(games),
    }
}

pub fn launch(
    action: &Launch,
    directory: Option<&Path>,
    elevated: bool,
    environment: &std::collections::BTreeMap<String, String>,
) -> Result<()> {
    use crate::process::{Process, Target, quote};
    let (program, arguments) = match action {
        Launch::Uri { uri } => {
            // A Store or Xbox game starts through the shell by its application id. That is
            // no protocol handler, and the id is checked again since `apps.json` is editable.
            if uri.starts_with("shell:") {
                anyhow::ensure!(
                    library::store_launch_target(uri).is_some(),
                    "{uri:?} is not the launch address of a Microsoft Store or Xbox game."
                );
            } else {
                let scheme = uri.split_once(':').context("Invalid launcher URI")?.0;
                anyhow::ensure!(
                    registry::protocol(scheme),
                    "The {scheme} launcher is unavailable. Install or repair the launcher."
                );
            }
            let program =
                PathBuf::from(std::env::var_os("WINDIR").context("Windows directory unavailable")?)
                    .join("explorer.exe");
            (program, quote(std::ffi::OsStr::new(uri))?)
        }
        Launch::Program { program, arguments } => (PathBuf::from(program), arguments.clone()),
    };
    let line = format!("{} {arguments}", quote(program.as_os_str())?);
    // The launcher may already serve other games. Own only the new game
    // processes discovered by the tracker, never the shared launcher job.
    Process::spawn_command(
        &program,
        &line,
        directory,
        Target::User { elevated },
        environment,
        true,
        true,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests;
