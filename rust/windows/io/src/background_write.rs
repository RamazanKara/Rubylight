//! What a background writer such as Stream DVR's clip writer needs from Windows: the free space
//! on a folder's volume, and below-normal priority for its own thread so it never competes with
//! the stream's threads.
use std::io;
use std::path::Path;
use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
use windows::Win32::System::Threading::{
    GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL,
};
use windows::core::HSTRING;

/// The bytes the host may still write to `folder`'s volume (after any disk quota).
pub fn free_bytes(folder: &Path) -> io::Result<u64> {
    let path = HSTRING::from(folder.as_os_str());
    let mut free = 0u64;
    // SAFETY: `path` is a NUL-terminated wide string alive for the call; `free` is a writable
    // u64 the call fills.
    unsafe { GetDiskFreeSpaceExW(&path, Some(&mut free), None, None) }.map_err(io::Error::other)?;
    Ok(free)
}

/// Lowers the calling thread to below-normal priority; best effort.
pub fn lower_current_thread() {
    // SAFETY: the pseudo handle of the current thread needs no closing, and the call takes no
    // pointers.
    let _ = unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL) };
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_temp_volume_has_free_space() {
        assert!(super::free_bytes(&std::env::temp_dir()).unwrap() > 0);
        assert!(super::free_bytes(std::path::Path::new(r"Q:\no\such\folder")).is_err());
        super::lower_current_thread();
    }
}
