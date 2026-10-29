//! nightjar.exe carries packaging/nightjar.exe.manifest as its manifest resource (build.rs), not
//! MinGW's default one: Windows reads resource 1 of type RT_MANIFEST when it starts the process.
#![cfg(windows)]

use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::FreeLibrary;
use windows::Win32::System::LibraryLoader::{
    FindResourceW, LoadLibraryExW, LoadResource, LockResource, SizeofResource, LOAD_LIBRARY_AS_DATAFILE,
    LOAD_LIBRARY_AS_IMAGE_RESOURCE,
};

/// MAKEINTRESOURCE(n).
fn int_resource(n: u16) -> PCWSTR {
    PCWSTR(n as usize as *const u16)
}

#[test]
fn the_exe_declares_the_segment_heap_in_its_manifest() {
    let exe = HSTRING::from(env!("CARGO_BIN_EXE_nightjar"));
    // SAFETY: the exe is mapped as data only; the resource lives until FreeLibrary.
    let manifest = unsafe {
        let module = LoadLibraryExW(&exe, None, LOAD_LIBRARY_AS_DATAFILE | LOAD_LIBRARY_AS_IMAGE_RESOURCE).expect("map nightjar.exe");
        let found = FindResourceW(Some(module), int_resource(1), int_resource(24));
        assert!(!found.is_invalid(), "nightjar.exe has no manifest resource 1");
        let size = SizeofResource(Some(module), found) as usize;
        let data = LoadResource(Some(module), found).expect("load the manifest resource");
        let bytes = std::slice::from_raw_parts(LockResource(data) as *const u8, size).to_vec();
        FreeLibrary(module).expect("unmap nightjar.exe");
        String::from_utf8(bytes).expect("UTF-8 manifest")
    };
    let expected = include_str!("../packaging/nightjar.exe.manifest");
    assert_eq!(manifest.trim_end_matches('\0').trim(), expected.trim());
    assert!(manifest.contains(">SegmentHeap</heapType>"));
    assert!(manifest.contains("level=\"asInvoker\""));
    // DPI awareness stays SDL's to declare at run time.
    assert!(!manifest.contains("dpiAware"));
}
