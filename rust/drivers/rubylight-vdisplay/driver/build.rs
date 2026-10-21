//! WDK link flags for the cdylib (wdk-build) and `IddCxStub.lib`, through which the driver
//! calls the IddCx DDIs and which needs `IddMinimumVersionRequired` exported.
//!
//! From punktfunk's pf-vdisplay build script by Enrico Bühler (unom), MIT OR Apache-2.0.
//! The VERSIONINFO resource is Rubylight's own: SignPath signs the DLL only when it names
//! Rubylight and the release (rust/release/sign-artifacts.ps1).
include!("../../../assets/version_info.rs");

fn main() -> Result<(), wdk_build::ConfigError> {
    // The vendored wdk-build leaves /INTEGRITYCHECK off for UMDF: with it, WUDFHost refused
    // the self-signed DLL (CodeIntegrity 3004/3089, invalid root; Code 31).
    wdk_build::configure_wdk_binary_build()?;
    link_iddcx_stub();
    link_version_resource();
    Ok(())
}

/// Compile the VERSIONINFO with the SDK's rc.exe and hand the .res to the MSVC linker, which
/// links resource files directly.
fn link_version_resource() {
    println!("cargo:rerun-if-changed=../../../assets/version_info.rs");
    let rc = write_resource(
        "rubylight-vdisplay",
        "",
        "Rubylight virtual display driver",
        true,
    );
    let res = rc.with_extension("res");
    let status = std::process::Command::new(resource_compiler())
        .args(["/nologo", "/fo"])
        .arg(&res)
        .arg(&rc)
        .status()
        .expect("running rc.exe");
    assert!(status.success(), "rc.exe failed on {}", rc.display());
    println!("cargo:rustc-link-arg-cdylib={}", res.display());
}

/// rc.exe from the newest Windows Kit, or from PATH (a developer shell).
fn resource_compiler() -> std::path::PathBuf {
    let bin = std::path::Path::new(r"C:\Program Files (x86)\Windows Kits\10\bin");
    let mut kits: Vec<_> = std::fs::read_dir(bin)
        .into_iter()
        .flatten()
        .flatten()
        .map(|kit| kit.path().join(r"x64\rc.exe"))
        .filter(|rc| rc.is_file())
        .collect();
    kits.sort_by_key(|rc| {
        let name = rc
            .parent()
            .and_then(|x64| x64.parent())
            .and_then(|kit| kit.file_name());
        name.map(|name| {
            name.to_string_lossy()
                .split('.')
                .map(|part| part.parse::<u32>().unwrap_or(0))
                .collect::<Vec<_>>()
        })
    });
    kits.pop().unwrap_or_else(|| "rc.exe".into())
}

/// Link `IddCxStub.lib` from `Lib\<sdk>\um\<arch>\iddcx\<iddcx version>\`. The versions are
/// not interchangeable: the stub's `IddFunctions` table must match the framework the driver
/// runs on, and the 1.0 stub fails even `IddCxDeviceInitConfig` on a 1.10 framework. So take
/// the highest version that has the library, compared numerically ("1.10" > "1.9").
fn link_iddcx_stub() {
    let target_arch = std::env::var("CARGO_CFG_TARGET_ARCH").expect("CARGO_CFG_TARGET_ARCH");
    let cpu = wdk_build::CpuArchitecture::try_from_cargo_str(&target_arch)
        .unwrap_or_else(|| panic!("no WDK lib dir for target arch {target_arch}"));
    let arch = cpu.as_windows_str();
    // A folder with a newer stub than the installed kit has (IddCx 1.11 ships in WDK 28000's
    // NuGet package, not in the 26100 kit).
    println!("cargo:rerun-if-env-changed=RLVD_IDDCX_LIB");
    if let Ok(dir) = std::env::var("RLVD_IDDCX_LIB") {
        let dir = std::path::PathBuf::from(dir);
        assert!(
            dir.join("IddCxStub.lib").is_file(),
            "RLVD_IDDCX_LIB={} has no IddCxStub.lib",
            dir.display()
        );
        println!("cargo:rustc-link-search={}", dir.display());
        println!("cargo:rustc-link-lib=static=IddCxStub");
        return;
    }
    const ROOTS: [&str; 2] = [
        r"C:\Program Files (x86)\Windows Kits\10\Lib",
        r"C:\Program Files\Windows Kits\10\Lib",
    ];
    let mut best: Option<((u32, u32), std::path::PathBuf)> = None;
    for root in ROOTS {
        let Ok(versions) = std::fs::read_dir(root) else {
            continue;
        };
        for ver in versions.flatten() {
            let iddcx = ver.path().join("um").join(arch).join("iddcx");
            let Ok(subdirs) = std::fs::read_dir(&iddcx) else {
                continue;
            };
            for sub in subdirs.flatten() {
                if !sub.path().join("IddCxStub.lib").is_file() {
                    continue;
                }
                let name = sub.file_name().to_string_lossy().into_owned();
                let mut parts = name.split('.');
                let v = (
                    parts.next().and_then(|x| x.parse().ok()).unwrap_or(0u32),
                    parts.next().and_then(|x| x.parse().ok()).unwrap_or(0u32),
                );
                if best.as_ref().is_none_or(|(bv, _)| v > *bv) {
                    best = Some((v, sub.path()));
                }
            }
        }
    }
    let Some((_, dir)) = best else {
        panic!(
            "IddCxStub.lib not found under any Windows Kits Lib\\<ver>\\um\\{arch}\\iddcx\\<iddcxver>\\"
        );
    };
    println!("cargo:rustc-link-search={}", dir.display());
    println!("cargo:rustc-link-lib=static=IddCxStub");
}
