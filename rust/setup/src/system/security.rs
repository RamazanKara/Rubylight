//! Folder trust verification, ownership checks and ACL helpers for Rubylight setup.
use super::{run, system32};
use anyhow::{Context, Result, ensure};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use windows::{
    Win32::{Foundation::ERROR_FILE_NOT_FOUND, Security::PSID},
    core::{HSTRING, PCWSTR, PWSTR},
};

/// Why a folder fails [`trusted_folder`] when someone other than
/// Administrators, SYSTEM or TrustedInstaller may have made or changed it: a
/// link in it, or another owner. Errors reading the folder are not one.
#[derive(Debug)]
pub struct Untrusted(pub(super) String);
impl std::fmt::Display for Untrusted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Untrusted {}
pub fn trusted_folder(folder: &Path) -> Result<()> {
    if let Err(error) = std::fs::symlink_metadata(folder) {
        return if error.kind() == std::io::ErrorKind::NotFound {
            Ok(())
        } else {
            Err(error.into())
        };
    }
    no_reparse_points(folder)?;
    trusted_owner(folder, false)
}
/// [`trusted_folder`] for an existing `folder` and everything in it, which
/// also no one else may change or add to: the owner of any entry can change
/// its permissions back, and anyone they let write can plant settings. A
/// missing `folder` is an error. Of each folder in `opaque` (which the host
/// lets users write into and checks itself) only the folder is checked.
/// Entries setup may not read count as untrusted too. An entry removed
/// after its folder was listed (the running host replaces its files) is
/// skipped: nothing of it is left, and only the trusted can add to a folder
/// that passed.
pub fn trusted_tree(folder: &Path, opaque: &[PathBuf]) -> Result<()> {
    let mut pending = vec![folder.to_path_buf()];
    while let Some(path) = pending.pop() {
        match trusted_entry(&path, opaque) {
            Ok(entries) => pending.extend(entries),
            Err(error) if path != folder && vanished(&path, &error) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}
/// One entry of [`trusted_tree`] checked, returning the entries in it to
/// check next.
fn trusted_entry(path: &Path, opaque: &[PathBuf]) -> Result<Vec<PathBuf>> {
    use std::os::windows::fs::MetadataExt;
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_attributes() & 0x400 != 0 {
        return Err(Untrusted(format!("{} is a reparse point", path.display())).into());
    }
    let checked = trusted_owner(path, false)
        .and_then(|()| other_writer(path))
        .and_then(|writer| match writer {
            Some(writer) => Err(Untrusted(format!(
                "{} may be changed by {writer}, not only Administrators, SYSTEM or TrustedInstaller",
                path.display()
            ))
            .into()),
            None => Ok(()),
        });
    match checked {
        Ok(()) if metadata.is_dir() && !listed(path, opaque) => std::fs::read_dir(path)
            .and_then(|entries| {
                entries
                    .map(|entry| entry.map(|entry| entry.path()))
                    .collect::<std::io::Result<Vec<_>>>()
            })
            .map_err(|error| denied_is_untrusted(error.into(), path)),
        Ok(()) => Ok(Vec::new()),
        Err(error) => Err(denied_is_untrusted(error, path)),
    }
}
/// Whether `path` is one of `folders`, by name as Windows compares them.
fn listed(path: &Path, folders: &[PathBuf]) -> bool {
    folders
        .iter()
        .any(|folder| folder.as_os_str().eq_ignore_ascii_case(path.as_os_str()))
}
/// Whether `error`, about `path`, came from `path` being gone, which it
/// still is: not-found alone may also come from a path too long for the
/// calls that read permissions.
pub(super) fn vanished(path: &Path, error: &anyhow::Error) -> bool {
    not_found(error)
        && std::fs::symlink_metadata(path)
            .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
}
pub(super) fn not_found(error: &anyhow::Error) -> bool {
    use windows::Win32::Foundation::ERROR_PATH_NOT_FOUND;
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound)
            || cause
                .downcast_ref::<windows::core::Error>()
                .is_some_and(|e| {
                    e.code() == ERROR_FILE_NOT_FOUND.to_hresult()
                        || e.code() == ERROR_PATH_NOT_FOUND.to_hresult()
                })
    })
}
/// An entry setup may not read: whoever owns it denied Administrators.
fn denied_is_untrusted(error: anyhow::Error, path: &Path) -> anyhow::Error {
    let denied = error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|e| e.kind() == std::io::ErrorKind::PermissionDenied)
            || cause
                .downcast_ref::<windows::core::Error>()
                .is_some_and(|e| e.code() == windows::Win32::Foundation::E_ACCESSDENIED)
    });
    if denied {
        Untrusted(format!("{} may not be read: {error:#}", path.display())).into()
    } else {
        error
    }
}
/// The first account other than SYSTEM, Administrators or TrustedInstaller
/// (or the creator or owner, both trusted here) that `path`'s permissions
/// let change it or add to it.
pub(super) fn other_writer(path: &Path) -> Result<Option<String>> {
    use windows::Win32::{
        Foundation::{HLOCAL, LocalFree},
        Security::{Authorization::*, *},
    };
    // Reading, executing and synchronizing, as specific or generic rights.
    const READING: u32 = 0x1200a9 | 0x8000_0000 | 0x2000_0000;
    let mut dacl: *mut ACL = std::ptr::null_mut();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: dacl and its entries point into descriptor until it is freed once below.
    unsafe {
        GetNamedSecurityInfoW(
            &HSTRING::from(path.as_os_str()),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            None,
            None,
            Some(&mut dacl),
            None,
            &mut descriptor,
        )
        .ok()?;
        let result = (|| -> Result<Option<String>> {
            if dacl.is_null() {
                return Ok(Some("everyone (it has no permission list)".into()));
            }
            let mut size = ACL_SIZE_INFORMATION::default();
            GetAclInformation(
                dacl,
                (&raw mut size).cast(),
                std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
                AclSizeInformation,
            )?;
            for index in 0..size.AceCount {
                let mut ace = std::ptr::null_mut();
                GetAce(dacl, index, &mut ace)?;
                match (*ace.cast::<ACE_HEADER>()).AceType {
                    // Allowed.
                    0 => {}
                    // Denied, plainly or for an object or a callback.
                    1 | 6 | 10 | 12 => continue,
                    other => return Ok(Some(format!("an access entry of type {other}"))),
                }
                let allowed = &*ace.cast::<ACCESS_ALLOWED_ACE>();
                if allowed.Mask & !READING == 0 {
                    continue;
                }
                let sid = PSID((&raw const allowed.SidStart).cast_mut().cast());
                let trusted = [
                    WinLocalSystemSid,
                    WinBuiltinAdministratorsSid,
                    WinCreatorOwnerSid,
                    WinCreatorOwnerRightsSid,
                ]
                .into_iter()
                .any(|known| IsWellKnownSid(sid, known).as_bool())
                    || trusted_installer(sid);
                if !trusted {
                    return Ok(Some(account_name(sid)));
                }
            }
            Ok(None)
        })();
        let _ = LocalFree(Some(HLOCAL(descriptor.0)));
        result.with_context(|| format!("reading the permissions of {}", path.display()))
    }
}
/// Whether `sid` is TrustedInstaller's service SID.
fn trusted_installer(sid: PSID) -> bool {
    use windows::Win32::{
        Foundation::{HLOCAL, LocalFree},
        Security::Authorization::ConvertSidToStringSidW,
    };
    let mut text = PWSTR::null();
    // SAFETY: text is freed once below after it is read.
    unsafe {
        let installer = ConvertSidToStringSidW(sid, &mut text).is_ok()
            && text.to_string().is_ok_and(|sid| {
                sid == "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464"
            });
        if !text.is_null() {
            let _ = LocalFree(Some(HLOCAL(text.0.cast())));
        }
        installer
    }
}
pub fn trusted_ancestors(folder: &Path) -> Result<()> {
    trusted_parents(folder)?;
    trusted_folder(folder)
}
pub fn trusted_parents(folder: &Path) -> Result<()> {
    use std::os::windows::fs::MetadataExt;
    for ancestor in folder
        .ancestors()
        .skip(1)
        .filter(|p| !p.as_os_str().is_empty())
    {
        let metadata = match std::fs::symlink_metadata(ancestor) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        ensure!(
            metadata.file_attributes() & 0x400 == 0,
            "{} is a reparse point",
            ancestor.display()
        );
        trusted_owner(ancestor, system_drive_root(ancestor))?;
    }
    Ok(())
}
/// Whether `folder` belongs to Administrators, SYSTEM or TrustedInstaller,
/// or, with `administrator_accounts`, to an account in the local
/// Administrators group: for the system drive's root, which a "take
/// ownership" tweak can give to its administrator and setup must not change.
pub(super) fn trusted_owner(folder: &Path, administrator_accounts: bool) -> Result<()> {
    use windows::Win32::{
        Foundation::{HLOCAL, LocalFree},
        Security::{Authorization::*, *},
    };
    let mut owner = PSID::default();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: the owner SID points into descriptor until it is freed once below.
    unsafe {
        GetNamedSecurityInfoW(
            &HSTRING::from(folder.as_os_str()),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            Some(&mut owner),
            None,
            None,
            None,
            &mut descriptor,
        )
        .ok()?;
        let trusted = IsWellKnownSid(owner, WinBuiltinAdministratorsSid).as_bool()
            || IsWellKnownSid(owner, WinLocalSystemSid).as_bool()
            || trusted_installer(owner);
        let result = if trusted {
            Ok(())
        } else {
            match administrator_accounts.then(|| administrator_account(owner)) {
                Some(Ok(true)) => Ok(()),
                Some(Err(error)) => Err(error.context(format!(
                    "{} is owned by {}; reading the Administrators group",
                    folder.display(),
                    account_name(owner)
                ))),
                _ => Err(Untrusted(format!(
                    "{} must be owned by Administrators, SYSTEM or TrustedInstaller{}, not {}",
                    folder.display(),
                    if administrator_accounts {
                        " or an administrator's account"
                    } else {
                        ""
                    },
                    account_name(owner)
                ))
                .into()),
            }
        };
        let _ = LocalFree(Some(HLOCAL(descriptor.0)));
        result
    }
}
/// Whether `folder` is the root of the drive Windows runs from (by
/// GetSystemDirectoryW, not the environment). Whoever owns that root can
/// already swap Program Files and every service in it, so trusting its owner
/// adds nothing; another drive's owner would gain Rubylight's service alone.
pub(super) fn system_drive_root(folder: &Path) -> bool {
    use std::path::{Component, Prefix};
    fn letter(component: Option<Component>) -> Option<u8> {
        match component {
            Some(Component::Prefix(prefix)) => match prefix.kind() {
                Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => {
                    Some(letter.to_ascii_uppercase())
                }
                _ => None,
            },
            _ => None,
        }
    }
    let mut buffer = [0u16; 260];
    // SAFETY: the buffer is live and writable for its whole length.
    let length = unsafe {
        windows::Win32::System::SystemInformation::GetSystemDirectoryW(Some(&mut buffer))
    } as usize;
    if length == 0 || length >= buffer.len() {
        return false;
    }
    let system = PathBuf::from(String::from_utf16_lossy(&buffer[..length]));
    let mut parts = folder.components();
    let drive = letter(parts.next());
    drive.is_some()
        && drive == letter(system.components().next())
        && parts.next() == Some(Component::RootDir)
        && parts.next().is_none()
}
/// Whether `account` is listed in the local Administrators group.
///
/// netapi32 is loaded from System32 at run time: as an import, Windows would
/// look for it beside setup.exe first (in Downloads), and run a planted copy
/// as administrator.
///
/// # Safety
/// `account` must point to a valid SID.
pub(super) unsafe fn administrator_account(account: PSID) -> Result<bool> {
    use windows::{
        Win32::{
            Foundation::{FreeLibrary, HLOCAL, LocalFree},
            Security::{Authorization::ConvertStringSidToSidW, EqualSid},
            System::LibraryLoader::{GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW},
        },
        core::s,
    };
    type GetMembers = unsafe extern "system" fn(
        PCWSTR,
        PCWSTR,
        u32,
        *mut *mut u8,
        u32,
        *mut u32,
        *mut u32,
        *mut usize,
    ) -> u32;
    type BufferFree = unsafe extern "system" fn(*const std::ffi::c_void) -> u32;
    // LOCALGROUP_MEMBERS_INFO_0
    #[repr(C)]
    struct Member {
        sid: PSID,
    }
    // The group's name is translated ("Administratoren"); look it up by SID.
    let mut group = PSID::default();
    // SAFETY: `group` is freed once below, the member list once with
    // NetApiBufferFree and the library once after both functions are done;
    // the signatures are those of lmaccess.h and lmapibuf.h.
    unsafe {
        ConvertStringSidToSidW(&HSTRING::from("S-1-5-32-544"), &mut group)?;
        let name = lookup_account(group);
        let _ = LocalFree(Some(HLOCAL(group.0)));
        let (_, name) = name.context("looking up the Administrators group")?;
        let name = HSTRING::from(name);
        let library = LoadLibraryExW(
            &HSTRING::from("netapi32.dll"),
            None,
            LOAD_LIBRARY_SEARCH_SYSTEM32,
        )
        .context("loading netapi32.dll")?;
        let result = (|| -> Result<bool> {
            let get_members: GetMembers = std::mem::transmute(
                GetProcAddress(library, s!("NetLocalGroupGetMembers"))
                    .context("netapi32.dll has no NetLocalGroupGetMembers")?,
            );
            let free: BufferFree = std::mem::transmute(
                GetProcAddress(library, s!("NetApiBufferFree"))
                    .context("netapi32.dll has no NetApiBufferFree")?,
            );
            let mut members: *mut u8 = std::ptr::null_mut();
            let (mut read, mut total) = (0u32, 0u32);
            let status = get_members(
                PCWSTR::null(),
                PCWSTR(name.as_ptr()),
                0,
                &mut members,
                u32::MAX,
                &mut read,
                &mut total,
                std::ptr::null_mut(),
            );
            let listed = status == 0
                && !members.is_null()
                && std::slice::from_raw_parts(members.cast::<Member>(), read as usize)
                    .iter()
                    .any(|member| EqualSid(member.sid, account).is_ok());
            if !members.is_null() {
                free(members.cast());
            }
            ensure!(status == 0, "NetLocalGroupGetMembers failed ({status})");
            Ok(listed)
        })();
        let _ = FreeLibrary(library);
        result
    }
}
/// `DOMAIN\name` for an account, or its SID when it has none (an account
/// deleted since, or one from an earlier Windows installation).
///
/// # Safety
/// `account` must point to a valid SID.
unsafe fn account_name(account: PSID) -> String {
    use windows::Win32::{
        Foundation::{HLOCAL, LocalFree},
        Security::Authorization::ConvertSidToStringSidW,
    };
    // SAFETY: the caller's SID is only read; the SID text is freed once.
    unsafe {
        if let Ok((domain, name)) = lookup_account(account) {
            return if domain.is_empty() {
                name
            } else {
                format!("{domain}\\{name}")
            };
        }
        let mut text = PWSTR::null();
        if ConvertSidToStringSidW(account, &mut text).is_err() {
            return "an unknown account".into();
        }
        let sid = text.to_string().unwrap_or_default();
        let _ = LocalFree(Some(HLOCAL(text.0.cast())));
        sid
    }
}
/// The domain and name of an account.
///
/// # Safety
/// `account` must point to a valid SID.
unsafe fn lookup_account(account: PSID) -> Result<(String, String)> {
    use windows::Win32::Security::{LookupAccountSidW, SID_NAME_USE};
    let (mut name, mut domain) = ([0u16; 256], [0u16; 256]);
    let (mut name_len, mut domain_len) = (name.len() as u32, domain.len() as u32);
    let mut kind = SID_NAME_USE::default();
    // SAFETY: both buffers are as long as the lengths passed with them.
    unsafe {
        LookupAccountSidW(
            PCWSTR::null(),
            account,
            Some(PWSTR(name.as_mut_ptr())),
            &mut name_len,
            Some(PWSTR(domain.as_mut_ptr())),
            &mut domain_len,
            &mut kind,
        )?;
    }
    Ok((
        String::from_utf16_lossy(&domain[..domain_len as usize]),
        String::from_utf16_lossy(&name[..name_len as usize]),
    ))
}
pub fn no_reparse_points(folder: &Path) -> Result<()> {
    no_reparse_points_except(folder, &[])
}
/// [`no_reparse_points`], looking at each folder in `opaque` but not into it.
fn no_reparse_points_except(folder: &Path, opaque: &[PathBuf]) -> Result<()> {
    use std::os::windows::fs::MetadataExt;
    let mut pending = vec![folder.to_path_buf()];
    while let Some(path) = pending.pop() {
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(Untrusted(format!("{} is a reparse point", path.display())).into());
        }
        if metadata.is_dir() && !listed(&path, opaque) {
            for entry in std::fs::read_dir(&path)? {
                pending.push(entry?.path());
            }
        }
    }
    Ok(())
}
/// Limit a folder to SYSTEM and Administrators (and, if `users_read`, read
/// access for users), replacing the owner and the entire permission list.
pub fn restrict(folder: &Path, users_read: bool) -> Result<()> {
    restrict_except(folder, users_read, &[])
}
/// [`restrict`] for a folder holding the folders in `opaque`, which the host
/// lets users write into: links in them are no error, only a link in their
/// place. The new permissions reach no further than a folder with its own,
/// as the host gives these, and Windows carries them through no link either
/// (measured on Windows 11 with junctions and folder symbolic links).
pub fn restrict_except(folder: &Path, users_read: bool, opaque: &[PathBuf]) -> Result<()> {
    no_reparse_points_except(folder, opaque)?;
    use windows::Win32::{
        Foundation::{HLOCAL, LocalFree},
        Security::{Authorization::*, *},
    };
    let sddl = restricted_sddl(users_read);
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: descriptor owns the SID and ACL until LocalFree after the last call.
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            &HSTRING::from(sddl),
            1,
            &mut descriptor,
            None,
        )?;
        let result = (|| -> Result<()> {
            let mut owner = PSID::default();
            let mut defaulted = windows::core::BOOL(0);
            let mut present = windows::core::BOOL(0);
            let mut dacl = std::ptr::null_mut();
            GetSecurityDescriptorOwner(descriptor, &mut owner, &mut defaulted)?;
            GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted)?;
            SetNamedSecurityInfoW(
                &HSTRING::from(folder.as_os_str()),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION
                    | DACL_SECURITY_INFORMATION
                    | PROTECTED_DACL_SECURITY_INFORMATION,
                Some(owner),
                None,
                Some(dacl),
                None,
            )
            .ok()?;
            Ok(())
        })();
        let _ = LocalFree(Some(HLOCAL(descriptor.0)));
        result.with_context(|| format!("setting permissions on {}", folder.display()))
    }
}
fn restricted_sddl(users_read: bool) -> &'static str {
    if users_read {
        "O:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;0x1200a9;;;BU)"
    } else {
        "O:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)"
    }
}
/// Create `folder` (not its parents) with the permissions [`restrict`]
/// gives, from the start: a folder created in ProgramData first inherits
/// users' right to add to it. Fails if `folder` exists.
pub fn create_restricted(folder: &Path, users_read: bool) -> Result<()> {
    use windows::Win32::{
        Foundation::{HLOCAL, LocalFree},
        Security::{Authorization::*, *},
        Storage::FileSystem::CreateDirectoryW,
    };
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: descriptor stays alive until LocalFree after CreateDirectoryW.
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            &HSTRING::from(restricted_sddl(users_read)),
            1,
            &mut descriptor,
            None,
        )?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: false.into(),
        };
        let result = CreateDirectoryW(&HSTRING::from(folder.as_os_str()), Some(&attributes));
        let _ = LocalFree(Some(HLOCAL(descriptor.0)));
        result.with_context(|| format!("creating {}", folder.display()))
    }
}
/// Operations `inherit_contents` runs on each target, in order: taking
/// ownership first grants Administrators `WRITE_DAC` so `/reset` succeeds
/// even when a non-admin owner stripped Administrators from the ACL.
pub(crate) const INHERIT_OPERATIONS: [(&[&str], &str); 2] = [
    (&["/setowner", "*S-1-5-32-544"], "setting ownership on"),
    (&["/reset"], "resetting permissions on"),
];
/// Reset a folder's contents to inherit the folder's permissions and belong
/// to Administrators, except the folders in `opaque` directly in it, which
/// keep their own and so does everything in them.
pub fn inherit_contents(folder: &Path, opaque: &[PathBuf]) -> Result<()> {
    no_reparse_points_except(folder, opaque)?;
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(folder)? {
        let entry = entry?;
        if !listed(&entry.path(), opaque) {
            entries.push((entry.path(), entry.file_type()?.is_dir()));
        }
    }
    if entries.is_empty() {
        return Ok(());
    }
    // Everything in one pass, unless something may be kept, even one made
    // only after the listing: then each entry on its own, recursing only
    // into folders, as icacls looks for a file's name in every folder below
    // it too.
    let targets = if opaque.is_empty() {
        vec![(folder.join("*"), true)]
    } else {
        entries
    };
    // Each change on every entry, as icacls goes on past a file it can't
    // change (/C).
    let mut failures = Vec::new();
    for (target, recurse) in targets {
        let target = target.display().to_string();
        for (change, failed) in INHERIT_OPERATIONS {
            let mut args = vec![target.as_str()];
            args.extend(change);
            if recurse {
                args.push("/T");
            }
            args.extend(["/C", "/L", "/Q"]);
            let (code, _) = run(&system32("icacls.exe"), &args, Duration::from_secs(300))?;
            if code != 0 {
                failures.push(format!("{failed} {target} failed ({code})"));
            }
        }
    }
    ensure!(failures.is_empty(), "{}", failures.join("; "));
    Ok(())
}
