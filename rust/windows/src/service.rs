//! The Windows service and the host. The service itself is its own crate
//! (rust/service), which stays the same from release to release; it runs the
//! host as SYSTEM in the console session and hands it the duties whose code
//! changes with the host, as `butterpollo.exe` tasks (rust/host/src/service_tasks.rs).
//! Here: the names both sides share, and putting back what Nucleus Co-op left.

use anyhow::Result;

/// The service's name, as in rust/service/src/supervisor.rs.
pub const NAME: &str = "ApolloService";
/// The exit code with which the host asks the service to start it again at
/// once, as in rust/service/src/supervisor.rs.
pub const RESTART_EXIT_CODE: u32 = 75;

/// Put back the compatibility settings a Nucleus Co-op session left for the
/// user at the console when its host stopped mid-session
/// (`nucleus::layers`): the service has the host do it before a host starts,
/// and as the user signs in. A session of theirs that runs keeps its own.
/// Whether nothing failed; each failure is logged.
pub fn restore_nucleus_layers(config: &std::path::Path) -> bool {
    use crate::nucleus::layers::AtConsole;
    // The host's settings file, for nucleus_path.
    let configured = match butterpollo_core::config::Config::load(&config.join("sunshine.conf")) {
        Ok(config) => config
            .nucleus_path()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default(),
        Err(error) => {
            tracing::error!(error = %format!("{error:#}"), "Nucleus Co-op: the settings did not load, so the compatibility settings an earlier session changed were not put back");
            return false;
        }
    };
    let settings = restore_nucleus_settings(&configured);
    match crate::nucleus::layers::restore_at_console(
        &butterpollo_core::paths::data_root(),
        &configured,
    ) {
        Ok(AtConsole::Done) => {}
        Ok(AtConsole::Busy) => tracing::info!(
            "Nucleus Co-op: a session runs for the user at the console and puts its compatibility settings back itself"
        ),
        Ok(AtConsole::NoUser { why, waiting }) => {
            if !waiting.is_empty() {
                let files = waiting
                    .iter()
                    .map(|file| file.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                tracing::warn!(
                    why,
                    files,
                    "Nucleus Co-op: compatibility settings an earlier session changed wait for their user to sign in at the console"
                );
            }
        }
        Err(error) => {
            tracing::error!(error = %format!("{error:#}"), "Nucleus Co-op: the compatibility settings an earlier session changed were not put back");
            return false;
        }
    }
    settings
}

/// Put back the user's Nucleus Co-op settings a session changed when its
/// host stopped mid-session ([`nucleus_settings_left`]). Whether nothing
/// failed.
fn restore_nucleus_settings(configured: &str) -> bool {
    let restored = (|| -> Result<Option<usize>> {
        let Some(folder) =
            crate::nucleus::install::installed(&butterpollo_core::paths::data_root(), configured)?
        else {
            return Ok(Some(0));
        };
        nucleus_settings_left(&folder, crate::nucleus::session::running)
    })();
    match restored {
        Ok(Some(0)) => {}
        Ok(None) => tracing::info!(
            "Nucleus Co-op runs or a session holds its settings; they are put back when it ends or before the next"
        ),
        Ok(Some(changed)) => tracing::info!(
            changed,
            "Nucleus Co-op: the settings an earlier session changed are back"
        ),
        Err(error) => {
            tracing::error!(
                error = %format!("{error:#}"),
                "Nucleus Co-op: the settings an earlier session changed were not put back"
            );
            return false;
        }
    }
    true
}

/// [`crate::nucleus::files::restore_settings_left`] in the Nucleus Co-op at
/// `folder`, unless it runs (`running`): a session's own puts them back as
/// it ends. None when skipped, as also while a session holds the lock.
fn nucleus_settings_left(
    folder: &std::path::Path,
    running: impl FnOnce() -> Result<Option<u32>>,
) -> Result<Option<usize>> {
    if let Some(pid) = running()? {
        tracing::info!(pid, "Nucleus Co-op runs");
        return Ok(None);
    }
    crate::nucleus::files::restore_settings_left(folder)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nucleus_settings_are_put_back_only_with_neither_nucleus_nor_a_session() {
        use butterpollo_core::nucleus::profile::Forced;
        let nucleus = tempfile::tempdir().unwrap();
        let folder = nucleus.path();
        let settings = folder.join("Settings.ini");
        // What a host that stopped mid-session left.
        let left = || {
            std::fs::write(&settings, "[Dev]\r\nShowFavoriteOnly=False\r\n").unwrap();
            let forced = [Forced {
                section: "Dev".into(),
                key: "ShowFavoriteOnly".into(),
                before: b"True".to_vec(),
                forced: b"False".to_vec(),
            }];
            std::fs::write(
                folder.join("Rubylight-settings.json"),
                serde_json::to_vec(&forced).unwrap(),
            )
            .unwrap();
        };
        let favourites = || std::fs::read_to_string(&settings).unwrap();
        left();
        // Nucleus Co-op runs: nothing is read or put back.
        let running = || Ok(Some(42));
        assert_eq!(nucleus_settings_left(folder, running).unwrap(), None);
        assert!(favourites().contains("ShowFavoriteOnly=False"));
        // A session starting holds the lock until it ends.
        let session = crate::nucleus::files::write_settings(folder).unwrap();
        left();
        assert_eq!(nucleus_settings_left(folder, || Ok(None)).unwrap(), None);
        assert!(favourites().contains("ShowFavoriteOnly=False"));
        drop(session);
        left();
        assert_eq!(nucleus_settings_left(folder, || Ok(None)).unwrap(), Some(1));
        assert!(favourites().contains("ShowFavoriteOnly=True"));
        assert!(!folder.join("Rubylight-settings.json").exists());
        // A failed look at the processes is an error, not a restore.
        left();
        let error = nucleus_settings_left(folder, || anyhow::bail!("denied")).unwrap_err();
        assert_eq!(error.to_string(), "denied");
        assert!(favourites().contains("ShowFavoriteOnly=False"));
    }
    #[test]
    fn an_unreadable_settings_file_is_a_failed_restore() {
        let profile = tempfile::tempdir().unwrap();
        // A folder where the settings file belongs cannot be read as one.
        std::fs::create_dir(profile.path().join("sunshine.conf")).unwrap();
        assert!(!restore_nucleus_layers(profile.path()));
    }
}
