//! Owners, SIDs and access lists of files, folders and handles.

use super::*;

/// Whether an object owned by `owner` (a SID) is one this process can trust
/// with machine-wide settings ([`owner_trusted`]).
pub fn trusted_owner(owner: &str) -> Result<bool> {
    let token = token()?;
    Ok(owner_trusted(
        owner,
        &token_user_sid(&token)?,
        &token_owner_sid(&token)?,
    ))
}
/// [`trusted_owner`] for a process that runs as `own` and gives what it
/// makes to `makes` (its default owner): SYSTEM's and the Administrators
/// group's (what SYSTEM and an elevated administrator make), and `own`'s
/// only when it makes those itself (an unelevated process: a program of that
/// account can do what it does anyway). An elevated administrator makes the
/// Administrators group's; its account's own are what that account's
/// unelevated programs make, any of which could make one first. Never
/// another user's.
pub(crate) fn owner_trusted(owner: &str, own: &str, makes: &str) -> bool {
    matches!(owner, "S-1-5-18" | "S-1-5-32-544") || (owner == own && makes == own)
}
/// The owner what this process makes gets ([`token_owner_sid`]).
#[cfg(test)]
pub(crate) fn default_owner() -> Result<String> {
    token_owner_sid(&token()?)
}
/// The owner (a SID) of the kernel object `handle` opens with `READ_CONTROL`.
pub fn handle_owner(handle: HANDLE) -> Result<String> {
    owner_of(
        handle,
        windows::Win32::Security::Authorization::SE_KERNEL_OBJECT,
    )
}
/// The owner (a SID) of the file or folder `file` has open: of the object
/// then read through it, which the path could have been swapped for since.
pub fn open_file_owner(file: &std::fs::File) -> Result<String> {
    owner_of(
        HANDLE(file.as_raw_handle()),
        windows::Win32::Security::Authorization::SE_FILE_OBJECT,
    )
}
/// `path` opened to check and read through one handle: a link is opened
/// itself, never followed, and the file is not written while it is open. A
/// `folder` cannot be renamed or deleted while it is open, so a file opened
/// in it after it was checked is in the folder checked.
pub fn open_unfollowed(path: &Path, folder: bool) -> std::io::Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_LIST_DIRECTORY,
        FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, READ_CONTROL,
    };
    let mut options = std::fs::OpenOptions::new();
    if folder {
        // Listing is data access: a handle with only attribute or security
        // access holds no share mode, and the folder could still be renamed.
        options
            .access_mode(FILE_LIST_DIRECTORY.0 | READ_CONTROL.0 | FILE_READ_ATTRIBUTES.0)
            .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE).0)
            .custom_flags((FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT).0);
    } else {
        // Delete is shared so a newer record can still take its name.
        options
            .read(true)
            .share_mode((FILE_SHARE_READ | FILE_SHARE_DELETE).0)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0);
    }
    options.open(path)
}
fn owner_of(
    handle: HANDLE,
    kind: windows::Win32::Security::Authorization::SE_OBJECT_TYPE,
) -> Result<String> {
    use windows::Win32::Security::Authorization::GetSecurityInfo;
    let mut owner = PSID::default();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: `owner` points into `descriptor`, read before it is freed once with LocalFree.
    unsafe {
        GetSecurityInfo(
            handle,
            kind,
            OWNER_SECURITY_INFORMATION,
            Some(&mut owner),
            None,
            None,
            None,
            Some(&mut descriptor),
        )
        .ok()
        .context("reading the owner")?;
        let sid = sid_text(owner);
        let _ = LocalFree(Some(HLOCAL(descriptor.0)));
        sid
    }
}
/// The owner (a SID) of a file or folder.
pub fn file_owner(path: &Path) -> Result<String> {
    use windows::Win32::Security::Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT};
    let name = wide(path.as_os_str())?;
    let mut owner = PSID::default();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: `name` is NUL-terminated and outlives the call; `owner` points into `descriptor`,
    // read before it is freed once with LocalFree.
    unsafe {
        GetNamedSecurityInfoW(
            PCWSTR(name.as_ptr()),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            Some(&mut owner),
            None,
            None,
            None,
            &mut descriptor,
        )
        .ok()
        .with_context(|| format!("reading the owner of {}", path.display()))?;
        let sid = sid_text(owner);
        let _ = LocalFree(Some(HLOCAL(descriptor.0)));
        sid
    }
}
/// # Safety
/// `sid` points at a valid SID.
unsafe fn sid_text(sid: PSID) -> Result<String> {
    use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
    let mut text = PWSTR::null();
    // SAFETY: the caller's SID is valid; the string is read before it is freed once.
    unsafe {
        ConvertSidToStringSidW(sid, &mut text)?;
        let sid = text.to_string();
        let _ = LocalFree(Some(HLOCAL(text.0.cast())));
        sid.context("Windows returned an invalid SID")
    }
}
/// The account `sid` (`S-1-...`) names, as `DOMAIN\name` in this PC's
/// language; None for one Windows doesn't know.
pub fn account_name(sid: &str) -> Option<String> {
    use windows::Win32::Security::{
        Authorization::ConvertStringSidToSidW, LookupAccountSidW, SID_NAME_USE,
    };
    let text = wide(std::ffi::OsStr::new(sid)).ok()?;
    let mut binary = PSID::default();
    // SAFETY: `text` is NUL-terminated and outlives the call; the SID it
    // returns is read by the lookups and freed once after them.
    unsafe {
        ConvertStringSidToSidW(PCWSTR(text.as_ptr()), &mut binary).ok()?;
        let mut name = [0u16; 256];
        let mut domain = [0u16; 256];
        let (mut name_len, mut domain_len) = (name.len() as u32, domain.len() as u32);
        let mut kind = SID_NAME_USE::default();
        let found = LookupAccountSidW(
            PCWSTR::null(),
            binary,
            Some(PWSTR(name.as_mut_ptr())),
            &mut name_len,
            Some(PWSTR(domain.as_mut_ptr())),
            &mut domain_len,
            &mut kind,
        );
        let _ = LocalFree(Some(HLOCAL(binary.0)));
        found.ok()?;
        let name = String::from_utf16_lossy(&name[..name_len as usize]);
        let domain = String::from_utf16_lossy(&domain[..domain_len as usize]);
        Some(if domain.is_empty() {
            name
        } else {
            format!("{domain}\\{name}")
        })
    }
}
pub(crate) fn token_user_sid(token: &OwnedHandle) -> Result<String> {
    use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
    // SAFETY: `buffer` holds a TOKEN_USER once the call succeeds, and the string from
    // ConvertSidToStringSidW is read before it is freed once with LocalFree.
    unsafe {
        let mut buffer = [0usize; 64];
        let mut needed = 0;
        GetTokenInformation(
            raw(token),
            TokenUser,
            Some(buffer.as_mut_ptr().cast()),
            size_of::<[usize; 64]>() as u32,
            &mut needed,
        )?;
        anyhow::ensure!(
            needed as usize >= size_of::<TOKEN_USER>(),
            "Windows returned an invalid token owner"
        );
        let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
        let mut text = PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut text)?;
        let sid = text.to_string();
        let _ = LocalFree(Some(HLOCAL(text.0.cast())));
        sid.context("Windows returned an invalid user SID")
    }
}
/// The owner what `token` makes gets: its account, or the Administrators
/// group for an elevated administrator.
fn token_owner_sid(token: &OwnedHandle) -> Result<String> {
    // SAFETY: `buffer` is usize-aligned and holds a TOKEN_OWNER once the call succeeds, whose SID
    // points into it.
    unsafe {
        let mut buffer = [0usize; 64];
        let mut needed = 0;
        GetTokenInformation(
            raw(token),
            TokenOwner,
            Some(buffer.as_mut_ptr().cast()),
            size_of::<[usize; 64]>() as u32,
            &mut needed,
        )?;
        anyhow::ensure!(
            needed as usize >= size_of::<TOKEN_OWNER>(),
            "Windows returned an invalid token owner"
        );
        sid_text((*buffer.as_ptr().cast::<TOKEN_OWNER>()).Owner)
    }
}
/// SYSTEM and Administrators own it and alone have access.
pub(super) const ADMINISTRATORS_ONLY: &str = "O:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)";
/// As [`ADMINISTRATORS_ONLY`], and Users may read (list, read data,
/// attributes and permissions, never write).
pub(super) const USERS_READ: &str = "O:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;0x1200a9;;;BU)";
/// Give only SYSTEM and Administrators access to `folder` and the files in
/// it, replacing inherited permissions, as Vibepollo does for the service's
/// credentials.
pub fn restrict_to_administrators(folder: &Path) -> Result<()> {
    restrict(folder, ADMINISTRATORS_ONLY)
}
/// The permissions `sddl` describes on `folder` and the files in it.
pub fn restrict(folder: &Path, sddl: &str) -> Result<()> {
    use windows::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SE_FILE_OBJECT, SetNamedSecurityInfoW,
    };
    let sddl = wide(std::ffi::OsStr::new(sddl))?;
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: `sddl` and `path` are NUL-terminated and outlive the calls, and `dacl` points into
    // `descriptor`, which is freed once after its last use.
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(sddl.as_ptr()),
            1,
            &mut descriptor,
            None,
        )?;
        let result = (|| -> Result<()> {
            let mut present = BOOL(0);
            let mut defaulted = BOOL(0);
            let mut dacl: *mut ACL = std::ptr::null_mut();
            let mut owner = PSID::default();
            GetSecurityDescriptorOwner(descriptor, &mut owner, &mut defaulted)?;
            GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted)?;
            let mut pending = vec![folder.to_path_buf()];
            while let Some(path) = pending.pop() {
                use std::os::windows::fs::MetadataExt;
                let metadata = std::fs::symlink_metadata(&path)?;
                if metadata.file_attributes() & 0x400 != 0 {
                    bail!("{} is a reparse point", path.display());
                }
                let name = wide(path.as_os_str())?;
                SetNamedSecurityInfoW(
                    PCWSTR(name.as_ptr()),
                    SE_FILE_OBJECT,
                    OWNER_SECURITY_INFORMATION
                        | DACL_SECURITY_INFORMATION
                        | PROTECTED_DACL_SECURITY_INFORMATION,
                    Some(owner),
                    None,
                    Some(dacl),
                    None,
                )
                .ok()
                .with_context(|| format!("setting permissions on {}", path.display()))?;
                if metadata.is_dir() {
                    for entry in std::fs::read_dir(&path)? {
                        pending.push(entry?.path());
                    }
                }
            }
            Ok(())
        })();
        let _ = LocalFree(Some(HLOCAL(descriptor.0)));
        result
    }
}
/// Create the folder `path` with [`USERS_READ`]'s owner and permissions from
/// the start: it never has the inherited ones, not even for a moment.
/// AlreadyExists when anything has its name, which is left as it is. Making
/// Administrators the owner takes administrator rights.
pub fn create_folder_administrators_readable(path: &Path) -> std::io::Result<()> {
    create_folder(path, USERS_READ)
}
/// Create the folder `path` as [`create_folder_administrators_readable`]
/// does, with [`ADMINISTRATORS_ONLY`]'s owner and permissions: nobody else
/// can open it, not even between its creation and its permissions.
pub fn create_folder_administrators_only(path: &Path) -> std::io::Result<()> {
    create_folder(path, ADMINISTRATORS_ONLY)
}
/// Create the folder `path` with `sddl`'s owner and permissions from the start.
pub(crate) fn create_folder(path: &Path, sddl: &str) -> std::io::Result<()> {
    use windows::Win32::Storage::FileSystem::CreateDirectoryW;
    let name = wide(path.as_os_str()).map_err(std::io::Error::other)?;
    with_attributes(sddl, |attributes| {
        // SAFETY: `name` is NUL-terminated and `attributes` points at a live descriptor, both
        // outliving the call.
        unsafe { CreateDirectoryW(PCWSTR(name.as_ptr()), Some(attributes)) }.map_err(io_error)
    })
}
/// Create the new file `path` as [`create_folder_administrators_readable`]
/// does, open for writing alone: nothing else can open it while it is open.
/// AlreadyExists when anything has its name, a link too, which is never
/// followed.
pub fn create_file_administrators_readable(path: &Path) -> std::io::Result<std::fs::File> {
    use windows::Win32::Storage::FileSystem::{
        CREATE_NEW, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_SHARE_MODE,
    };
    let name = wide(path.as_os_str()).map_err(std::io::Error::other)?;
    with_attributes(USERS_READ, |attributes| {
        // SAFETY: as above; the handle is new, and the File alone owns it.
        unsafe {
            let handle = CreateFileW(
                PCWSTR(name.as_ptr()),
                GENERIC_WRITE.0,
                FILE_SHARE_MODE(0),
                Some(attributes),
                CREATE_NEW,
                FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
                None,
            )
            .map_err(io_error)?;
            Ok(std::fs::File::from_raw_handle(handle.0))
        }
    })
}
/// `create` with the security attributes `sddl` describes.
fn with_attributes<T>(
    sddl: &str,
    create: impl FnOnce(*const SECURITY_ATTRIBUTES) -> std::io::Result<T>,
) -> std::io::Result<T> {
    use windows::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
    let sddl = wide(std::ffi::OsStr::new(sddl)).map_err(std::io::Error::other)?;
    let mut descriptor = crate::ipc::Descriptor(PSECURITY_DESCRIPTOR::default());
    // SAFETY: `sddl` is NUL-terminated and outlives the call; the descriptor is freed once, when
    // `descriptor` drops after `create`.
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(sddl.as_ptr()),
            1,
            &mut descriptor.0,
            None,
        )
    }
    .map_err(io_error)?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0.0,
        bInheritHandle: BOOL(0),
    };
    create(&attributes)
}
/// `error` as the Windows error it carries, so its kind (NotFound,
/// AlreadyExists) is std's.
fn io_error(error: windows::core::Error) -> std::io::Error {
    let code = error.code().0 as u32;
    if code >> 16 == 0x8007 {
        std::io::Error::from_raw_os_error((code & 0xFFFF) as i32)
    } else {
        std::io::Error::other(error)
    }
}
/// Who may write the file or folder `file` has open, change its permissions
/// or owner, or delete it: the SIDs its permissions allow any of that,
/// Everyone (`S-1-1-0`) when it has none. An entry only for what is created
/// inside it is skipped; an allowing entry of another kind is named by its
/// type ("ACE type 9"), never skipped.
pub fn open_file_writers(file: &std::fs::File) -> Result<Vec<String>> {
    open_file_allowed(file, WRITE)
}
/// Who may move or delete what the folder `file` has open holds (delete children), or change the
/// folder's permissions or owner: as [`open_file_writers`], without those who may only add to it.
pub fn open_file_child_movers(file: &std::fs::File) -> Result<Vec<String>> {
    open_file_allowed(file, CHILD_MOVE)
}
/// Who may move or delete the file or folder `file` has open itself, or what a folder holds, or
/// change its permissions or owner: as [`open_file_child_movers`], with those who may delete it.
pub fn open_file_movers(file: &std::fs::File) -> Result<Vec<String>> {
    open_file_allowed(file, CHILD_MOVE | 0x1_0000)
}
pub(super) fn open_file_allowed(file: &std::fs::File, mask: u32) -> Result<Vec<String>> {
    use windows::Win32::Security::Authorization::{GetSecurityInfo, SE_FILE_OBJECT};
    let mut dacl: *mut ACL = std::ptr::null_mut();
    let mut descriptor = crate::ipc::Descriptor(PSECURITY_DESCRIPTOR::default());
    // SAFETY: `dacl` points into `descriptor`, which is freed once, after its last use here.
    unsafe {
        GetSecurityInfo(
            HANDLE(file.as_raw_handle()),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            None,
            None,
            Some(&mut dacl),
            None,
            Some(&mut descriptor.0),
        )
        .ok()
        .context("reading the permissions")?;
        dacl_allowed(dacl, mask)
    }
}
// Write data or add a file, append or add a folder, write extended attributes, delete
// children, write attributes, delete, write permissions, take ownership, all, write.
pub(super) const WRITE: u32 =
    0x2 | 0x4 | 0x10 | 0x40 | 0x100 | 0x1_0000 | 0x4_0000 | 0x8_0000 | 0x1000_0000 | 0x4000_0000;
// Delete children, write permissions, take ownership, all, write.
pub(super) const CHILD_MOVE: u32 = 0x40 | 0x4_0000 | 0x8_0000 | 0x1000_0000 | 0x4000_0000;
/// The SIDs `dacl` allows any of `mask`, as [`open_file_writers`] names them.
///
/// # Safety
/// `dacl` is null or points at a valid ACL.
pub(super) unsafe fn dacl_allowed(dacl: *const ACL, mask: u32) -> Result<Vec<String>> {
    const INHERIT_ONLY: u8 = 0x8;
    if dacl.is_null() {
        return Ok(vec!["S-1-1-0".into()]);
    }
    let mut writers = Vec::new();
    // SAFETY: the caller's ACL is valid, so are the entries GetAce finds in it, and an allowing
    // entry's SID starts at its SidStart.
    unsafe {
        for index in 0..u32::from((*dacl).AceCount) {
            let mut ace = std::ptr::null_mut();
            GetAce(dacl, index, &mut ace).context("reading the permissions")?;
            let header = &*ace.cast::<ACE_HEADER>();
            if header.AceFlags & INHERIT_ONLY != 0 {
                continue;
            }
            match header.AceType {
                // ACCESS_ALLOWED_ACE_TYPE.
                0 => {
                    let allowed = &*ace.cast::<ACCESS_ALLOWED_ACE>();
                    if allowed.Mask & mask != 0 {
                        writers.push(sid_text(PSID(
                            (&raw const allowed.SidStart).cast_mut().cast(),
                        ))?);
                    }
                }
                // Compound, object, callback and callback object allowing entries.
                kind @ (4 | 5 | 9 | 11) => writers.push(format!("ACE type {kind}")),
                // Denying entries allow nothing.
                _ => {}
            }
        }
    }
    Ok(writers)
}
