//! Windows side of what is left of the removed background child session:
//! the registry, child-session switch and Windows Firewall behind the seams
//! of [`butterpollo_core::child_session`], so restore puts back what setup
//! changed; the lock restore takes across processes ([`lock`]); the
//! profiles the password the removed feature saved can be in, and its
//! deletion.
//!
//! Restoring the Windows settings that setup changed is ported from Annex
//! (mattxslv/annex, MIT, Copyright (c) 2026 mattxslv; the license text is in
//! `third-party/licenses/annex-LICENSE.txt`): `src/DesktopSetup.cs` and
//! `scripts/setup-admin.ps1`.

pub mod lock;

use crate::text::{from_wide, to_wide};
use anyhow::{Context, Result, bail};
use butterpollo_core::child_session::{Hive, Registry, SystemSettings};
use std::{
    ffi::OsString,
    io::{self, Write},
    os::windows::{
        ffi::OsStringExt,
        fs::{MetadataExt, OpenOptionsExt},
        io::{FromRawHandle, OwnedHandle},
    },
    path::{Path, PathBuf},
};
use windows::{
    Win32::{
        Foundation::{
            ERROR_FILE_NOT_FOUND, ERROR_NO_MORE_ITEMS, ERROR_PATH_NOT_FOUND, ERROR_SUCCESS,
            GetLastError, HANDLE, LUID, SetLastError, WIN32_ERROR,
        },
        NetworkManagement::WindowsFirewall::{INetFwPolicy2, NetFwPolicy2},
        Security::{
            AdjustTokenPrivileges, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW,
            SE_PRIVILEGE_ENABLED, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
        },
        Storage::FileSystem::{
            DELETE, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS,
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
            FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        },
        System::{
            Com::{CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx},
            Registry::{
                HKEY, HKEY_LOCAL_MACHINE, HKEY_USERS, KEY_READ, REG_DWORD, RRF_RT_REG_DWORD,
                RRF_RT_REG_SZ, RegCloseKey, RegDeleteKeyValueW, RegEnumKeyExW, RegGetValueW,
                RegLoadKeyW, RegOpenKeyExW, RegSetKeyValueW, RegUnLoadKeyW,
            },
            RemoteDesktop::{
                WTSEnableChildSessions, WTSGetChildSessionId, WTSIsChildSessionsEnabled,
                WTSQueryUserToken,
            },
            Threading::{GetCurrentProcess, OpenProcessToken},
        },
        UI::Shell::FOLDERID_LocalAppData,
    },
    core::{BOOL, BSTR, HRESULT, PCWSTR, PWSTR, w},
};

/// Whether Windows child sessions are on (`WTSIsChildSessionsEnabled`).
pub fn child_sessions_enabled() -> Result<bool> {
    let mut enabled = BOOL(0);
    // SAFETY: `enabled` is a live local the call writes once.
    unsafe { WTSIsChildSessionsEnabled(&mut enabled) }
        .ok()
        .context("Windows could not report whether child sessions are on")?;
    Ok(enabled.as_bool())
}

/// The child session of this process's session, None when there is none.
pub fn child_session_id() -> Option<u32> {
    let mut session = 0;
    // SAFETY: `session` is a live local the call writes once.
    unsafe { WTSGetChildSessionId(&mut session) }
        .as_bool()
        .then_some(session)
        .filter(|session| *session != 0 && *session != u32::MAX)
}

fn root(hive: &Hive, path: &str) -> (HKEY, Vec<u16>) {
    match hive {
        Hive::LocalMachine => (HKEY_LOCAL_MACHINE, to_wide(path)),
        Hive::User(sid) => (HKEY_USERS, to_wide(&format!(r"{sid}\{path}"))),
    }
}
fn absent(error: WIN32_ERROR) -> bool {
    error == ERROR_FILE_NOT_FOUND || error == ERROR_PATH_NOT_FOUND
}
fn hive_name(hive: &Hive) -> String {
    match hive {
        Hive::LocalMachine => "HKLM".into(),
        Hive::User(sid) => format!(r"HKU\{sid}"),
    }
}

/// The real registry, child-session switch and Windows Firewall. Restore
/// runs as an administrator (the SYSTEM host or the uninstaller).
pub struct NativeSettings {
    policy: INetFwPolicy2,
}
impl NativeSettings {
    pub fn new() -> Result<Self> {
        // SAFETY: COM is initialised on this thread before the in-process firewall object is made.
        unsafe {
            // Already initialised in another mode is fine for this in-process object.
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            Ok(Self {
                policy: CoCreateInstance(&NetFwPolicy2, None, CLSCTX_INPROC_SERVER)
                    .context("opening Windows Firewall")?,
            })
        }
    }
}

/// Turns one rule of a group on by its ID; the COM firewall API names rules
/// only by their display name, which several rules can share.
const RULES_SCRIPT: &str = include_str!("child_session_rules.ps1");
const RULES_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
fn run_rules_script(mode: &str, environment: &[(&str, &str)]) -> Result<String> {
    use std::{os::windows::process::CommandExt, process::Command};
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    let mut command = Command::new(
        std::path::Path::new(&root).join("System32\\WindowsPowerShell\\v1.0\\powershell.exe"),
    );
    command
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &format!("& ([scriptblock]::Create([Console]::In.ReadToEnd())) {mode}"),
        ])
        // A PowerShell 7 parent's module path breaks Windows PowerShell's modules.
        .env_remove("PSModulePath")
        .env_remove("BUTTERPOLLO_FIREWALL_RULE")
        .envs(environment.iter().copied())
        .creation_flags(CREATE_NO_WINDOW);
    let (status, stdout, stderr) =
        crate::wake::output_within(&mut command, RULES_SCRIPT.as_bytes(), RULES_TIMEOUT)
            .with_context(|| format!("the firewall rule script ({mode})"))?;
    if !status.success() {
        bail!(
            "the firewall rule script ({mode}) failed with {status}: {}",
            stderr.trim()
        );
    }
    Ok(stdout)
}
const PROTOCOLS: [&str; 2] = ["TCP", "UDP"];
fn read_dword(hive: &Hive, path: &str, name: &str) -> Result<Option<u32>> {
    let (root, key) = root(hive, path);
    let value = to_wide(name);
    let mut data = 0u32;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: `key` and `value` are NUL-terminated and outlive the call, and `data` is the
    // 4-byte buffer `size` names.
    let read = unsafe {
        RegGetValueW(
            root,
            PCWSTR(key.as_ptr()),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut data as *mut u32).cast()),
            Some(&mut size),
        )
    };
    match read {
        error if error.is_ok() => Ok(Some(data)),
        error if absent(error) => Ok(None),
        error => Err(windows::core::Error::from(error.to_hresult()))
            .with_context(|| format!(r"reading {}\{path}\{name}", hive_name(hive))),
    }
}
fn write_dword(hive: &Hive, path: &str, name: &str, value: u32) -> Result<()> {
    let (root, key) = root(hive, path);
    let wide = to_wide(name);
    // SAFETY: `key` and `wide` are NUL-terminated and outlive the call, and `value` is the
    // 4-byte REG_DWORD the size names.
    unsafe {
        RegSetKeyValueW(
            root,
            PCWSTR(key.as_ptr()),
            PCWSTR(wide.as_ptr()),
            REG_DWORD.0,
            Some((&value as *const u32).cast()),
            size_of::<u32>() as u32,
        )
    }
    .ok()
    .with_context(|| format!(r"writing {}\{path}\{name}", hive_name(hive)))
}
/// The registry alone, without Windows Firewall: what turns RDP off when
/// [`NativeSettings::new`] fails.
pub struct NativeRegistry;
impl Registry for NativeRegistry {
    fn read_dword(&self, hive: &Hive, path: &str, name: &str) -> Result<Option<u32>> {
        read_dword(hive, path, name)
    }
    fn write_dword(&mut self, hive: &Hive, path: &str, name: &str, value: u32) -> Result<()> {
        write_dword(hive, path, name, value)
    }
}
impl Registry for NativeSettings {
    fn read_dword(&self, hive: &Hive, path: &str, name: &str) -> Result<Option<u32>> {
        read_dword(hive, path, name)
    }
    fn write_dword(&mut self, hive: &Hive, path: &str, name: &str, value: u32) -> Result<()> {
        write_dword(hive, path, name, value)
    }
}
impl SystemSettings for NativeSettings {
    fn delete_value(&mut self, hive: &Hive, path: &str, name: &str) -> Result<()> {
        let (root, key) = root(hive, path);
        let wide = to_wide(name);
        // SAFETY: `key` and `wide` are NUL-terminated and outlive the call.
        match unsafe { RegDeleteKeyValueW(root, PCWSTR(key.as_ptr()), PCWSTR(wide.as_ptr())) } {
            error if error.is_ok() || absent(error) => Ok(()),
            error => Err(windows::core::Error::from(error.to_hresult()))
                .with_context(|| format!(r"removing {}\{path}\{name}", hive_name(hive))),
        }
    }
    fn child_sessions_enabled(&self) -> Result<bool> {
        child_sessions_enabled()
    }
    fn set_child_sessions(&mut self, enabled: bool) -> Result<()> {
        // SAFETY: the call takes a plain flag.
        unsafe { WTSEnableChildSessions(enabled) }
            .ok()
            .context("WTSEnableChildSessions failed")
    }
    fn block_rule_present(&self, name: &str) -> Result<bool> {
        for base in [name.to_owned(), staging_name(name)] {
            for label in PROTOCOLS {
                let full = format!("{base} ({label})");
                // SAFETY: `policy` is a live firewall policy object and the BSTR outlives the call.
                match unsafe { self.policy.Rules()?.Item(&BSTR::from(full.as_str())) } {
                    Ok(_) => return Ok(true),
                    Err(error) if error.code() == HRESULT::from_win32(ERROR_FILE_NOT_FOUND.0) => {}
                    Err(error) => {
                        return Err(error)
                            .with_context(|| format!("looking for the firewall rule {full}"));
                    }
                }
            }
        }
        Ok(false)
    }
    fn enable_rule(&mut self, group: &str, id: &str) -> Result<()> {
        run_rules_script(
            "enable",
            &[
                ("BUTTERPOLLO_FIREWALL_GROUP", group),
                ("BUTTERPOLLO_FIREWALL_RULE", id),
            ],
        )
        .with_context(|| format!("turning the firewall rule {id} back on"))?;
        Ok(())
    }
    fn remove_rule(&mut self, name: &str) -> Result<()> {
        self.remove_named(&staging_name(name))?;
        self.remove_named(name)
    }
    fn load_user_hive(&mut self, sid: &str) -> Result<bool> {
        load_user_hive(sid)
    }
    fn unload_user_hive(&mut self, sid: &str) -> Result<()> {
        with_privileges(&[w!("SeRestorePrivilege"), w!("SeBackupPrivilege")], || {
            let key = to_wide(sid);
            // SAFETY: `key` is NUL-terminated and outlives the call.
            unsafe { RegUnLoadKeyW(HKEY_USERS, PCWSTR(key.as_ptr())) }
                .ok()
                .with_context(|| format!(r"unloading HKU\{sid}"))
        })
    }
}

/// Where setup put new block rules before the old ones went; a setup cut
/// short may have left them.
fn staging_name(name: &str) -> String {
    format!("{name} [new]")
}

/// Run `f` with these privileges of the process enabled, then put them back.
fn with_privileges<T>(names: &[PCWSTR], f: impl FnOnce() -> Result<T>) -> Result<T> {
    let mut token = HANDLE::default();
    // SAFETY: `token` is a live local the call writes once; it is closed below.
    unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
            &mut token,
        )
    }
    .context("opening this process's token")?;
    // SAFETY: the token was just opened here, and only this guard closes it.
    let _close = unsafe { OwnedHandle::from_raw_handle(token.0) };
    let mut previous = Vec::new();
    let mut enabled = || -> Result<()> {
        for name in names {
            let mut luid = LUID::default();
            // SAFETY: `name` is a static NUL-terminated string and `luid` is writable.
            unsafe { LookupPrivilegeValueW(None, *name, &mut luid) }?;
            let privileges = TOKEN_PRIVILEGES {
                PrivilegeCount: 1,
                Privileges: [LUID_AND_ATTRIBUTES {
                    Luid: luid,
                    Attributes: SE_PRIVILEGE_ENABLED,
                }],
            };
            let mut before = TOKEN_PRIVILEGES::default();
            let mut size = 0;
            // SAFETY: the token is open for adjusting; `before` and `size` are writable and
            // `before` is as large as the length given.
            unsafe {
                SetLastError(ERROR_SUCCESS);
                AdjustTokenPrivileges(
                    token,
                    false,
                    Some(&privileges),
                    size_of::<TOKEN_PRIVILEGES>() as u32,
                    Some(&mut before),
                    Some(&mut size),
                )?;
                previous.push(before);
                // Success without the privilege reports ERROR_NOT_ALL_ASSIGNED.
                if GetLastError() != ERROR_SUCCESS {
                    bail!(
                        "this process does not hold {}; run it as an administrator",
                        name.display()
                    );
                }
            }
        }
        Ok(())
    };
    let result = enabled().and_then(|()| f());
    for before in previous.iter().rev() {
        // SAFETY: as above; `before` is what the matching call returned.
        let _ = unsafe { AdjustTokenPrivileges(token, false, Some(before), 0, None, None) };
    }
    result
}

/// Load a signed-out account's hive (`NTUSER.DAT` in its profile folder) as
/// `HKU\<sid>`; false when it is there already.
fn load_user_hive(sid: &str) -> Result<bool> {
    let key = to_wide(sid);
    let mut open = HKEY::default();
    // SAFETY: `key` is NUL-terminated and outlives the call; `open` is closed below.
    match unsafe { RegOpenKeyExW(HKEY_USERS, PCWSTR(key.as_ptr()), None, KEY_READ, &mut open) } {
        error if error.is_ok() => {
            // SAFETY: `open` is the key the call opened.
            let _ = unsafe { RegCloseKey(open) };
            return Ok(false);
        }
        error if absent(error) => {}
        error => {
            return Err(windows::core::Error::from(error.to_hresult()))
                .with_context(|| format!(r"opening HKU\{sid}"));
        }
    }
    let folder = profile_folder(sid)?;
    let file = std::path::Path::new(&folder).join("NTUSER.DAT");
    let wide = to_wide(&file.to_string_lossy());
    with_privileges(&[w!("SeRestorePrivilege"), w!("SeBackupPrivilege")], || {
        // SAFETY: both strings are NUL-terminated and outlive the call.
        unsafe { RegLoadKeyW(HKEY_USERS, PCWSTR(key.as_ptr()), PCWSTR(wide.as_ptr())) }
            .ok()
            .with_context(|| format!(r"loading {} as HKU\{sid}", file.display()))
    })?;
    Ok(true)
}

const PROFILE_LIST: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList";

/// The account's profile folder, as `ProfileList` names it.
fn profile_folder(sid: &str) -> Result<String> {
    let key = to_wide(&format!(r"{PROFILE_LIST}\{sid}"));
    let name = to_wide("ProfileImagePath");
    let mut buffer = vec![0u16; 1024];
    let mut size = (buffer.len() * 2) as u32;
    // SAFETY: `key` and `name` are NUL-terminated and outlive the call, and `buffer` is
    // writable for the `size` bytes passed; RegGetValueW expands REG_EXPAND_SZ and
    // NUL-terminates what it returns.
    unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(key.as_ptr()),
            PCWSTR(name.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut size),
        )
    }
    .ok()
    .with_context(|| format!("reading the profile folder of {sid}"))?;
    let folder = from_wide(&buffer);
    if folder.is_empty() {
        bail!("the account {sid} has no profile folder");
    }
    Ok(folder)
}

/// The profile folder of every local and Microsoft account
/// (`S-1-5-21-*` under ProfileList); one that can't be read is an error.
pub fn profile_folders() -> Result<Vec<PathBuf>> {
    let list = to_wide(PROFILE_LIST);
    let mut key = HKEY::default();
    // SAFETY: `list` is NUL-terminated and outlives the call; `key` is closed below.
    unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(list.as_ptr()),
            None,
            KEY_READ,
            &mut key,
        )
    }
    .ok()
    .context("opening the ProfileList")?;
    let mut sids = Vec::new();
    let mut listed = Ok(());
    for index in 0.. {
        let mut name = [0u16; 256];
        let mut length = name.len() as u32;
        // SAFETY: `name` is writable for the `length` characters passed.
        match unsafe {
            RegEnumKeyExW(
                key,
                index,
                Some(PWSTR(name.as_mut_ptr())),
                &mut length,
                None,
                None,
                None,
                None,
            )
        } {
            error if error.is_ok() => sids.push(from_wide(&name[..length as usize])),
            error if error == ERROR_NO_MORE_ITEMS => break,
            error => {
                listed = Err(windows::core::Error::from(error.to_hresult()))
                    .context("listing the ProfileList");
                break;
            }
        }
    }
    // SAFETY: `key` is the key opened above.
    let _ = unsafe { RegCloseKey(key) };
    listed?;
    sids.into_iter()
        .filter(|sid| sid.starts_with("S-1-5-21-"))
        .map(|sid| profile_folder(&sid).map(PathBuf::from))
        .collect()
}

/// The real `AppData\Local` (`FOLDERID_LocalAppData`, wherever it was
/// redirected) of the account signed in at this PC's screen when this
/// process is SYSTEM, else of this process's own account. None when nobody is
/// signed in at the screen.
pub fn console_local_app_data() -> Result<Option<PathBuf>> {
    let token = if crate::process::is_system() {
        let mut token = HANDLE::default();
        // SAFETY: `token` is a writable handle slot.
        match unsafe { WTSQueryUserToken(crate::process::console_session(), &mut token) } {
            // SAFETY: WTS returned an open token that only this OwnedHandle closes.
            Ok(()) => Some(unsafe { OwnedHandle::from_raw_handle(token.0) }),
            Err(error) if crate::user_folder::no_signed_in_user(error.code()) => return Ok(None),
            Err(error) => {
                return Err(error).context("reading the account signed in at this PC's screen");
            }
        }
    } else {
        None
    };
    crate::user_folder::known_folder(&FOLDERID_LocalAppData, token.as_ref())
        .map(Some)
        .map_err(|error| anyhow::anyhow!("reading AppData\\Local: {error}"))
}

/// The open file or folder is `path` itself: no link, junction or mount on
/// the way led elsewhere. An administrator writing or deleting in a folder
/// its user can change could otherwise be led to another file.
fn opened_as(file: &std::fs::File, path: &Path) -> io::Result<()> {
    let attributes = file.metadata()?.file_attributes();
    if attributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
        return Err(io::Error::other(
            "it is a link, not what Rubylight saved; nothing was changed",
        ));
    }
    let opened = crate::user_folder::final_path(file)?;
    let expected = OsString::from_wide(&crate::user_folder::verbatim(path)?);
    if !crate::user_folder::same_name(&opened, &expected) {
        return Err(io::Error::other(format!(
            "it opens as {}, through a link on the way; nothing was changed",
            opened.to_string_lossy()
        )));
    }
    Ok(())
}

/// Overwrite the file at `path` with zeros, flush it to disk and delete it,
/// all through one handle opened without following a link; false when there
/// is none. A link, a folder, or a path that leads elsewhere is refused
/// with nothing written.
pub fn wipe_file(path: &Path) -> io::Result<bool> {
    let mut file = match std::fs::OpenOptions::new()
        .access_mode(FILE_GENERIC_READ.0 | FILE_GENERIC_WRITE.0 | DELETE.0)
        .share_mode(0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    opened_as(&file, path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::other("it is not a file; nothing was changed"));
    }
    let zeros = [0u8; 64 * 1024];
    let mut left = metadata.len();
    while left > 0 {
        let chunk = left.min(zeros.len() as u64) as usize;
        file.write_all(&zeros[..chunk])?;
        left -= chunk as u64;
    }
    file.sync_all()?;
    crate::user_folder::delete_on_close(&file)?;
    Ok(true)
}

/// Remove the folder at `path` when it is empty, through one handle opened
/// without following a link; a folder that holds anything, or none, is left.
pub fn remove_empty_folder(path: &Path) -> io::Result<()> {
    let folder = match std::fs::OpenOptions::new()
        .access_mode(DELETE.0 | FILE_READ_ATTRIBUTES.0)
        .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE).0)
        .custom_flags((FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT).0)
        .open(path)
    {
        Ok(folder) => folder,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    opened_as(&folder, path)?;
    if !folder.metadata()?.is_dir() {
        return Err(io::Error::other("it is not a folder; nothing was changed"));
    }
    match crate::user_folder::delete_on_close(&folder) {
        Err(error) if error.kind() == io::ErrorKind::DirectoryNotEmpty => Ok(()),
        removed => removed,
    }
}

impl NativeSettings {
    fn remove_named(&mut self, name: &str) -> Result<()> {
        for label in PROTOCOLS {
            let full = format!("{name} ({label})");
            // Several rules can share a name; remove until none is left.
            for _ in 0..16 {
                // SAFETY: `policy` is a live firewall policy object and the BSTR outlives the call.
                unsafe {
                    let rules = self.policy.Rules()?;
                    let name = BSTR::from(full.as_str());
                    if rules.Item(&name).is_err() {
                        break;
                    }
                    rules
                        .Remove(&name)
                        .with_context(|| format!("removing the firewall rule {full}"))?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use butterpollo_core::child_session::{Hive, Registry, SystemSettings, TERMINAL_SERVER};

    #[test]
    fn reading_settings_changes_nothing() {
        // Read-only: this PC's registry, child-session and firewall settings stay as they are.
        let mut settings = super::NativeSettings::new().unwrap();
        let value = settings
            .read_dword(&Hive::LocalMachine, TERMINAL_SERVER, "fDenyTSConnections")
            .unwrap();
        assert!(matches!(value, None | Some(0) | Some(1)));
        assert_eq!(
            settings
                .read_dword(
                    &Hive::LocalMachine,
                    r"SOFTWARE\Rubylight\No\Such\Key",
                    "Missing",
                )
                .unwrap(),
            None
        );
        settings.child_sessions_enabled().unwrap();
        assert!(
            !settings
                .block_rule_present("Rubylight test rule that does not exist")
                .unwrap()
        );
        let session = crate::process::current_session().unwrap();
        if let Ok(sid) = crate::process::session_user_sid(session) {
            assert!(!settings.load_user_hive(&sid).unwrap(), "{sid} is loaded");
        }
        assert!(super::profile_folder("S-1-5-21-0-0-0-1").is_err());
    }

    #[test]
    fn a_rule_outside_its_group_is_never_turned_on() {
        let mut settings = super::NativeSettings::new().unwrap();
        let error = format!(
            "{:#}",
            settings
                .enable_rule(
                    "Rubylight test group that does not exist",
                    "RemoteDesktop-UserMode-In-TCP"
                )
                .unwrap_err()
        );
        assert!(error.contains("is not in the group"), "{error}");
    }

    #[test]
    fn every_profile_folder_and_this_users_app_data_are_readable() {
        let profiles = super::profile_folders().unwrap();
        assert!(!profiles.is_empty());
        assert!(profiles.iter().all(|p| p.is_absolute()), "{profiles:?}");
        // Not SYSTEM here: this account's own.
        let local = super::console_local_app_data().unwrap().unwrap();
        assert!(local.is_absolute() && local.is_dir(), "{}", local.display());
    }

    #[test]
    fn a_wiped_file_is_zeroed_flushed_and_gone_and_a_missing_one_is_no_error() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("child-session-password.bin");
        std::fs::write(&path, vec![7u8; 100_000]).unwrap();
        assert!(super::wipe_file(&path).unwrap());
        assert!(!path.exists());
        assert!(!super::wipe_file(&path).unwrap());
        assert!(!super::wipe_file(&folder.path().join("none").join("x.bin")).unwrap());
        // A folder of that name is refused, and stays.
        std::fs::create_dir(&path).unwrap();
        assert!(super::wipe_file(&path).is_err());
        assert!(path.is_dir());
        // A folder goes only when it is empty.
        std::fs::write(path.join("kept"), b"x").unwrap();
        super::remove_empty_folder(&path).unwrap();
        assert!(path.join("kept").is_file());
        std::fs::remove_file(path.join("kept")).unwrap();
        super::remove_empty_folder(&path).unwrap();
        assert!(!path.exists());
        super::remove_empty_folder(&path).unwrap();
    }

    #[test]
    fn a_file_behind_a_junction_is_never_written() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("elsewhere");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("child-session-password.bin"), b"not yours").unwrap();
        let link = root.path().join("Rubylight");
        let made = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&target)
            .output()
            .unwrap();
        assert!(made.status.success(), "{made:?}");
        let error = super::wipe_file(&link.join("child-session-password.bin")).unwrap_err();
        assert!(error.to_string().contains("through a link"), "{error}");
        assert_eq!(
            std::fs::read(target.join("child-session-password.bin")).unwrap(),
            b"not yours"
        );
        // The junction itself is a link, never removed as a folder.
        assert!(super::remove_empty_folder(&link).is_err());
        assert!(target.is_dir());
    }
}
