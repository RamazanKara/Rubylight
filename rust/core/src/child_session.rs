//! What is left of background streaming in a Windows child session, which
//! Rubylight removed: Windows signs a child session in only with a Windows
//! password, and a Microsoft account that uses only Windows Hello has none.
//! Development builds set it up on some PCs, so the host and the uninstaller
//! still put back the Windows settings that setup changed ([`restore`]),
//! turn Remote Desktop off where setup's traces outlived its record
//! ([`enforce_unrecorded`]), and delete the password those builds saved
//! ([`delete_passwords`]).
//!
//! Restoring the Windows settings that setup changed is ported from Annex
//! (mattxslv/annex, MIT, Copyright (c) 2026 mattxslv; the license text is in
//! `third-party/licenses/annex-LICENSE.txt`): `src/DesktopSetup.cs`,
//! `scripts/setup.ps1` and `scripts/setup-admin.ps1`.
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

/// What setup changed ([`SetupState`]): one record for the PC, whichever
/// Rubylight profile set it up ([`setup_record`]); while it is there, restore
/// (and the uninstaller) have something to put back. A file of this name
/// anywhere else, such as in a profile its user can write, is never read.
pub const SETUP_FILE: &str = "child_session_setup.json";
/// The folder under [`crate::paths::data_root`] [`setup_record`] is in; only
/// SYSTEM and Administrators can write it.
pub const SETUP_FOLDER: &str = "windows-settings";
/// `%ProgramData%\Butterpollo\windows-settings\child_session_setup.json`.
pub fn setup_record() -> PathBuf {
    crate::paths::data_root()
        .join(SETUP_FOLDER)
        .join(SETUP_FILE)
}
/// The flag that restores it: `butterpollo.exe --child-session-restore`.
pub const RESTORE_FLAG: &str = "--child-session-restore";
/// Its exit code when the restore failed after it had put Remote Desktop back
/// as it was before setup ([`SetupState::rdp_restored`]): RDP is the user's
/// own again, so neither it nor the uninstaller turns it off. 1 is any other
/// failure, after which RDP is off. Not 2, which a command line the host
/// cannot parse exits with.
pub const RESTORE_PARTLY_EXIT: i32 = 4;

/// Where a registry value lives.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Hive {
    LocalMachine,
    /// `HKEY_USERS\<sid>`: the streaming user's own settings, written by an
    /// administrator process that is not that user.
    User(String),
}

pub const TERMINAL_SERVER: &str = r"SYSTEM\CurrentControlSet\Control\Terminal Server";
pub const WINSTATIONS: &str = r"SYSTEM\CurrentControlSet\Control\Terminal Server\WinStations";
pub const RDP_TCP: &str = r"SYSTEM\CurrentControlSet\Control\Terminal Server\WinStations\RDP-Tcp";
/// Group policy for Remote Desktop; its `fDenyTSConnections` overrides [`TERMINAL_SERVER`]'s.
pub const TERMINAL_SERVICES_POLICY: &str =
    r"SOFTWARE\Policies\Microsoft\Windows NT\Terminal Services";
pub const RDP_CLIENT: &str = r"Software\Microsoft\Terminal Server Client";
/// The firewall group of Windows' own "Remote Desktop" inbound rules.
pub const REMOTE_DESKTOP_RULES: &str = "@FirewallAPI.dll,-28752";
/// The inbound block rules setup added for the RDP port, `"<name> (TCP)"`
/// and `"<name> (UDP)"`, which kept RDP on this PC while it was on.
pub const BLOCK_RULE: &str = "Rubylight child session: RDP only on this PC";
const DEFAULT_RDP_PORT: u32 = 3389;

/// The registry's DWORD values: all [`turn_rdp_off`] needs, so it works when
/// the rest of [`SystemSettings`] (Windows Firewall) can't be opened.
pub trait Registry {
    fn read_dword(&self, hive: &Hive, path: &str, name: &str) -> Result<Option<u32>>;
    fn write_dword(&mut self, hive: &Hive, path: &str, name: &str, value: u32) -> Result<()>;
}

/// The system settings restore puts back; the registry, child sessions and
/// the firewall behind one injectable seam.
pub trait SystemSettings: Registry {
    /// Removing a value that does not exist succeeds.
    fn delete_value(&mut self, hive: &Hive, path: &str, name: &str) -> Result<()>;
    fn child_sessions_enabled(&self) -> Result<bool>;
    fn set_child_sessions(&mut self, enabled: bool) -> Result<()>;
    /// Whether a firewall rule named `"<name> (TCP)"` or `"<name> (UDP)"`
    /// is on this PC, on or off.
    fn block_rule_present(&self, name: &str) -> Result<bool>;
    /// Turn exactly the rule with this ID on; it fails when the rule is gone
    /// or is not in the group. An ID is the rule's unique name in the
    /// firewall store (`Get-NetFirewallRule`'s `Name`), not its display name,
    /// which several rules can share.
    fn enable_rule(&mut self, group: &str, id: &str) -> Result<()>;
    /// Remove the rules named `"<name> (TCP)"` and `"<name> (UDP)"`;
    /// removing rules that do not exist succeeds.
    fn remove_rule(&mut self, name: &str) -> Result<()>;
    /// Make `HKEY_USERS\<sid>` writable: load the account's own hive when it
    /// is signed out. True when this call loaded it, so it is unloaded after.
    fn load_user_hive(&mut self, sid: &str) -> Result<bool>;
    fn unload_user_hive(&mut self, sid: &str) -> Result<()>;
}

/// Every value setup changed, as it was before setup; saved once, before the
/// first change, and put back by [`restore`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupState {
    pub version: u32,
    pub user_sid: String,
    pub deny_ts_connections: Option<u32>,
    pub dwm_frame_interval: Option<u32>,
    pub suppress_when_minimized: Option<u32>,
    pub child_sessions: bool,
    /// The IDs (see [`SystemSettings::enable_rule`]) of the Remote Desktop
    /// rules that were on; restore turns on exactly these.
    pub remote_desktop_rule_ids: Vec<String>,
    /// Windows' default when a record has none.
    #[serde(default = "default_rdp_port")]
    pub rdp_port: u16,
    /// A restore put `fDenyTSConnections` back and removed the block rule,
    /// then stopped: RDP is the user's own again, and
    /// `remote_desktop_rule_ids` holds only the rules it could not turn back
    /// on. Restoring again skips those steps.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub rdp_restored: bool,
}
fn default_rdp_port() -> u16 {
    DEFAULT_RDP_PORT as u16
}

struct Change {
    machine: bool,
    path: &'static str,
    name: &'static str,
    /// What setup wrote; only the tests, which set up as it did, write it now.
    #[cfg_attr(not(test), allow(dead_code))]
    value: u32,
}
/// What setup wrote: RDP on (kept loopback-only by [`BLOCK_RULE`]), the
/// session frame interval raised to 8 (about 120 fps), and the RDP client
/// kept rendering while minimized.
const CHANGES: [Change; 3] = [
    Change {
        machine: true,
        path: TERMINAL_SERVER,
        name: "fDenyTSConnections",
        value: 0,
    },
    Change {
        machine: true,
        path: WINSTATIONS,
        name: "DWMFRAMEINTERVAL",
        value: 8,
    },
    Change {
        machine: false,
        path: RDP_CLIENT,
        name: "RemoteDesktop_SuppressWhenMinimized",
        value: 2,
    },
];
/// Where `change` lives for the account `sid`.
fn hive(change: &Change, sid: &str) -> Hive {
    if change.machine {
        Hive::LocalMachine
    } else {
        Hive::User(sid.into())
    }
}

/// Put every value back as it was before setup. Every step runs even when an
/// earlier one fails; the failures come back together. The firewall rules
/// come back only once RDP reads back as it was: until then the block rule
/// keeps it on this PC. Once RDP and the block rule are back, `state` says so
/// ([`SetupState::rdp_restored`], keeping only the rules that failed) and is
/// saved through `save` before child sessions: a restore that stops after
/// that resumes from there, never writing RDP's value again. Every value,
/// child sessions too, is read back.
pub fn restore(
    system: &mut dyn SystemSettings,
    state: &mut SetupState,
    save: impl FnOnce(&SetupState) -> Result<()>,
) -> Result<()> {
    let mut problems = Vec::new();
    let originals = [
        state.deny_ts_connections,
        state.dwm_frame_interval,
        state.suppress_when_minimized,
    ];
    let resumed = state.rdp_restored;
    let mut rdp_restored = resumed;
    for (index, (change, original)) in CHANGES.iter().zip(originals).enumerate() {
        // RDP is the user's own since the restore that put it back.
        if index == 0 && resumed {
            continue;
        }
        let hive = hive(change, &state.user_sid);
        // A signed-out account's hive is not under HKEY_USERS until loaded.
        let loaded = match &hive {
            Hive::User(sid) => match system.load_user_hive(sid) {
                Ok(loaded) => loaded.then(|| sid.clone()),
                Err(error) => {
                    problems.push(format!("{}: {error:#}", change.name));
                    continue;
                }
            },
            Hive::LocalMachine => None,
        };
        let done = match original {
            Some(value) => system.write_dword(&hive, change.path, change.name, value),
            None => system.delete_value(&hive, change.path, change.name),
        }
        .and_then(
            |()| match system.read_dword(&hive, change.path, change.name)? {
                now if now == original => Ok(()),
                now => bail!("it reads {now:?} after restoring {original:?}"),
            },
        );
        match done {
            Ok(()) => rdp_restored |= index == 0,
            Err(error) => problems.push(format!("{}: {error:#}", change.name)),
        }
        if let Some(sid) = loaded
            && let Err(error) = system.unload_user_hive(&sid)
        {
            problems.push(format!("unloading the registry hive of {sid}: {error:#}"));
        }
    }
    if rdp_restored {
        let mut failed = Vec::new();
        for id in &state.remote_desktop_rule_ids {
            if let Err(error) = system.enable_rule(REMOTE_DESKTOP_RULES, id) {
                problems.push(format!("firewall rule {id}: {error:#}"));
                failed.push(id.clone());
            }
        }
        let blocked = if resumed {
            Ok(())
        } else {
            system.remove_rule(BLOCK_RULE)
        };
        match blocked {
            Ok(()) if !resumed || failed != state.remote_desktop_rule_ids => {
                state.remote_desktop_rule_ids = failed;
                state.rdp_restored = true;
                if let Err(error) = save(state) {
                    problems.push(format!(
                        "saving that Remote Desktop and its firewall rules are back: {error:#}"
                    ));
                }
            }
            Ok(()) => {}
            Err(error) => problems.push(format!("firewall rule {BLOCK_RULE}: {error:#}")),
        }
    } else {
        problems.push(format!(
            "the firewall rules stay as setup left them (\"{BLOCK_RULE}\" on, Remote Desktop's own rules off) until {} is restored",
            CHANGES[0].name
        ));
    }
    let original = state.child_sessions;
    let child_sessions = system.set_child_sessions(original).and_then(|()| {
        match system.child_sessions_enabled()? {
            now if now == original => Ok(()),
            now => bail!("they read {now} after restoring {original}"),
        }
    });
    if let Err(error) = child_sessions {
        problems.push(format!("child sessions: {error:#}"));
    }
    if problems.is_empty() {
        Ok(())
    } else {
        bail!(
            "some Windows settings were not restored: {}",
            problems.join("; ")
        )
    }
}

/// What [`enforce_unrecorded`] found and did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Unrecorded {
    /// What was found and done, for the log; empty without the block rule.
    pub found: Vec<String>,
    /// The block rule is still there: Remote Desktop could not be turned
    /// off, or the rule could not be removed.
    pub left: bool,
}

/// On a PC with no setup record: when setup's block rule is still there
/// (the record is gone), child sessions were set up here, so Remote Desktop
/// is turned off and, once it reads back off, the rule is removed. Nothing
/// found without the rule: RDP, on or off, is then the user's own. Err when
/// the rule could not be looked for, with nothing changed.
pub fn enforce_unrecorded(system: &mut dyn SystemSettings) -> Result<Unrecorded> {
    if !system
        .block_rule_present(BLOCK_RULE)
        .context("looking for the firewall rule child-session setup added")?
    {
        return Ok(Unrecorded::default());
    }
    let mut left = true;
    let mut done = vec![format!(
        "the firewall rule \"{BLOCK_RULE}\" that child-session setup added is still there"
    )];
    let rdp = &CHANGES[0];
    let on = system
        .read_dword(&Hive::LocalMachine, rdp.path, rdp.name)
        .map(|value| value != Some(1));
    let off = match on {
        Ok(false) => Ok(()),
        Ok(true) => {
            let off = rdp_off(system);
            done.push(turn_rdp_off_report(&off));
            off
        }
        Err(error) => Err(error),
    };
    match off {
        Ok(()) => done.push(match system.remove_rule(BLOCK_RULE) {
            Ok(()) => {
                left = false;
                format!("the firewall rule \"{BLOCK_RULE}\" was removed")
            }
            Err(error) => format!(
                "removing the firewall rule \"{BLOCK_RULE}\" failed ({error:#}); it only blocks Remote Desktop, so remove it in Windows Defender Firewall"
            ),
        }),
        Err(error) => done.push(format!(
            "the firewall rule \"{BLOCK_RULE}\" stays, as Remote Desktop may still be on ({error:#})"
        )),
    }
    Ok(Unrecorded { found: done, left })
}

/// Turn Remote Desktop off (`fDenyTSConnections = 1`), read it back, and
/// make sure no group policy keeps it on ([`TERMINAL_SERVICES_POLICY`] wins
/// over the value written here). What was done, for the report.
pub fn turn_rdp_off(system: &mut dyn Registry) -> String {
    turn_rdp_off_report(&rdp_off(system))
}
fn turn_rdp_off_report(off: &Result<()>) -> String {
    let change = &CHANGES[0];
    match off {
        Ok(()) => format!(
            "Remote Desktop was turned off ({} = 1), so it is not open to the network",
            change.name
        ),
        Err(error) => format!(
            "turning Remote Desktop off failed ({}: {error:#}): turn it off in Windows' settings, or choose Restore again under Maintenance",
            change.name
        ),
    }
}
/// [`turn_rdp_off`], Err when RDP may still be on.
fn rdp_off(system: &mut dyn Registry) -> Result<()> {
    let change = &CHANGES[0];
    system
        .write_dword(&Hive::LocalMachine, change.path, change.name, 1)
        .and_then(
            |()| match system.read_dword(&Hive::LocalMachine, change.path, change.name)? {
                Some(1) => Ok(()),
                now => bail!("it reads {now:?} after writing 1"),
            },
        )
        .and_then(|()| {
            match system.read_dword(&Hive::LocalMachine, TERMINAL_SERVICES_POLICY, change.name)? {
                Some(0) => bail!(
                    "group policy keeps Remote Desktop on ({} = 0 under HKLM\\{TERMINAL_SERVICES_POLICY}), which wins over this setting",
                    change.name
                ),
                _ => Ok(()),
            }
        })
}

/// The folder under an account's `AppData\Local` where the removed feature
/// saved that account's Windows password, encrypted with DPAPI for it.
pub const PASSWORD_FOLDER: &str = "Rubylight";
/// The saved password, and the file a save cut short left
/// (`path.with_extension("tmp")`).
pub const PASSWORD_FILES: [&str; 2] = ["child-session-password.bin", "child-session-password.tmp"];

/// Every [`PASSWORD_FOLDER`] a saved password can be in: under each profile
/// folder (`ProfileImagePath` in ProfileList), and under each `AppData\Local`
/// given (an account's own `FOLDERID_LocalAppData`, which can be
/// redirected). Built only from those and the fixed tail: a path that is not
/// absolute or holds `..` is skipped. Each folder once.
pub fn password_folders(profiles: &[PathBuf], local_app_data: &[PathBuf]) -> Vec<PathBuf> {
    let usable = |path: &Path| {
        path.is_absolute() && !path.components().any(|part| part == Component::ParentDir)
    };
    let mut folders: Vec<PathBuf> = Vec::new();
    let candidates = profiles
        .iter()
        .filter(|profile| usable(profile))
        .map(|profile| profile.join("AppData").join("Local"))
        .chain(local_app_data.iter().filter(|local| usable(local)).cloned());
    for local in candidates {
        let folder = local.join(PASSWORD_FOLDER);
        let key = folder.to_string_lossy().to_lowercase();
        if !folders
            .iter()
            .any(|known| known.to_string_lossy().to_lowercase() == key)
        {
            folders.push(folder);
        }
    }
    folders
}

/// What [`delete_passwords`] did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct PasswordCleanup {
    /// The files deleted.
    pub deleted: Vec<PathBuf>,
    /// Each file (or folder) that could not be deleted, with its path and why.
    pub failed: Vec<String>,
}

/// Delete each of [`PASSWORD_FILES`] in `folders` through `wipe`, which
/// overwrites the file with zeros, flushes it and deletes it, and is
/// `Ok(false)` when there is none; then each folder a file went from through
/// `remove_empty`, which leaves a folder that still holds anything. A file
/// that fails is named with its path and the others still go; one that is
/// not there is no failure.
pub fn delete_passwords(
    folders: &[PathBuf],
    mut wipe: impl FnMut(&Path) -> std::io::Result<bool>,
    mut remove_empty: impl FnMut(&Path) -> std::io::Result<()>,
) -> PasswordCleanup {
    let mut cleanup = PasswordCleanup::default();
    for folder in folders {
        let mut emptied = false;
        for name in PASSWORD_FILES {
            let path = folder.join(name);
            match wipe(&path) {
                Ok(true) => {
                    cleanup.deleted.push(path);
                    emptied = true;
                }
                Ok(false) => {}
                Err(error) => cleanup.failed.push(format!(
                    "the child-session password Rubylight had saved at {} could not be deleted: {error}; delete it by hand",
                    path.display()
                )),
            }
        }
        if emptied && let Err(error) = remove_empty(folder) {
            cleanup.failed.push(format!(
                "the empty folder {} could not be removed: {error}",
                folder.display()
            ));
        }
    }
    cleanup
}

#[cfg(test)]
mod tests;
