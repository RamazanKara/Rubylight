//! Rubylight's host runs no D3D11 device: capture, colour conversion and both encoders work on
//! D3D12 (and PyroWave on Vulkan). Two checks keep it that way.
//!
//! - In code: a unit test reads every runtime source file of the Windows, host and core crates
//!   and fails if one names the D3D11 device type or a D3D11 device-creation function. Only test
//!   files (`tests.rs`, `*_tests.rs`, anything under a `tests` directory) may use D3D11, to
//!   render fixtures. Runtime modules must not contain those names at all, in code or comments.
//! - At run time, in debug builds: [`report`] logs at the end of every stream (target
//!   `d3d11_audit`) how many D3D11 devices host code created, which must be zero, and whether
//!   `d3d11.dll` is loaded in the process. Something else, such as the AMF runtime or the GPU
//!   driver, may load it; the log says so rather than hiding it.

use std::sync::atomic::{AtomicUsize, Ordering};

/// D3D11 devices host code created. There is no such code; a change that adds one must count
/// it here, and the debug check fails.
static CREATED: AtomicUsize = AtomicUsize::new(0);

/// Record a D3D11 device that host code created.
pub fn created_device() {
    CREATED.fetch_add(1, Ordering::Relaxed);
}

/// D3D11 devices host code created so far.
pub fn devices_created() -> usize {
    CREATED.load(Ordering::Relaxed)
}

/// Whether `d3d11.dll` is loaded in this process, by anything.
pub fn module_loaded() -> bool {
    // SAFETY: GetModuleHandleW only looks the name up; the handle is not used or freed.
    unsafe {
        windows::Win32::System::LibraryLoader::GetModuleHandleW(windows::core::w!("d3d11.dll"))
            .is_ok()
    }
}

/// Log the audit for `when` (such as "stream end") in debug builds, and fail if host code
/// created a D3D11 device. Release builds do nothing.
pub fn report(when: &str) {
    if cfg!(debug_assertions) {
        let created = devices_created();
        let loaded = module_loaded();
        tracing::info!(
            target: "d3d11_audit",
            when,
            devices_created = created,
            d3d11_dll_loaded = if loaded { "yes" } else { "no" },
            "D3D11 audit: the host creates no D3D11 device{}",
            if loaded {
                "; d3d11.dll is loaded in the process by another component (AMF runtime, GPU driver or system DLL)"
            } else {
                ""
            }
        );
        debug_assert_eq!(created, 0, "host code created a D3D11 device");
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    /// Names that mean a D3D11 device. Assembled so that this file does not contain them.
    fn forbidden() -> [String; 3] {
        [
            concat!("D3D11", "CreateDevice").to_owned(),
            concat!("ID3D11", "Device").to_owned(),
            concat!("D3D11On12", "CreateDevice").to_owned(),
        ]
    }

    fn test_file(path: &Path) -> bool {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        name == "tests.rs"
            || name.ends_with("_tests.rs")
            || path.components().any(|part| part.as_os_str() == "tests")
    }

    fn sources(directory: &Path, found: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(directory).expect("read a source directory") {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                sources(&path, found);
            } else if path.extension().is_some_and(|e| e == "rs") && !test_file(&path) {
                found.push(path);
            }
        }
    }

    /// Runtime files of `roots` that name a D3D11 device, with the name.
    fn offenders(roots: &[PathBuf]) -> Vec<(PathBuf, String)> {
        let mut files = Vec::new();
        for root in roots {
            sources(root, &mut files);
        }
        assert!(files.len() > 50, "found only {} runtime files", files.len());
        let forbidden = forbidden();
        files
            .into_iter()
            .filter_map(|path| {
                let text = std::fs::read_to_string(&path).expect("read a source file");
                forbidden
                    .iter()
                    .find(|name| text.contains(name.as_str()))
                    .map(|name| (path, name.clone()))
            })
            .collect()
    }

    #[test]
    fn no_runtime_module_creates_a_d3d11_device() {
        let rust = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let roots = ["windows/src", "host/src", "core/src"].map(|root| rust.join(root));
        let offenders = offenders(&roots);
        assert!(
            offenders.is_empty(),
            "runtime code names a D3D11 device; capture, conversion and encoding run on D3D12: {offenders:?}"
        );
    }

    #[test]
    fn the_scan_finds_a_device_in_runtime_files_but_not_in_tests() {
        let root = tempfile::tempdir().unwrap();
        let module = root.path().join("module");
        std::fs::create_dir_all(module.join("tests")).unwrap();
        let device = format!("let device: {} = make();", forbidden()[1]);
        for name in [
            "module/tests.rs",
            "module/decode_tests.rs",
            "module/tests/a.rs",
        ] {
            std::fs::write(root.path().join(name), &device).unwrap();
        }
        for index in 0..60 {
            std::fs::write(module.join(format!("clean{index}.rs")), "fn main() {}").unwrap();
        }
        assert!(offenders(&[root.path().to_owned()]).is_empty());
        std::fs::write(module.join("runtime.rs"), &device).unwrap();
        let found = offenders(&[root.path().to_owned()]);
        assert_eq!(found.len(), 1);
        assert!(found[0].0.ends_with("runtime.rs"));
    }

    #[test]
    fn the_audit_counts_no_device() {
        assert_eq!(super::devices_created(), 0);
        super::report("test");
    }
}
