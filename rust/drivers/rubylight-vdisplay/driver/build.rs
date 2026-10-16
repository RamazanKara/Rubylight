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

/// Link IddCx 1.11's `IddCxStub.lib` from `RLVD_IDDCX_ROOT\lib` (fetch-iddcx.ps1). The stub
/// must match the headers wdk-sys bound: its `IddFunctions` table and the client config size
/// are per version, and a 1.10 stub caps the table before `IddCxCheckOsFeatureSupport`.
fn link_iddcx_stub() {
    println!("cargo:rerun-if-env-changed=RLVD_IDDCX_ROOT");
    let root = std::env::var("RLVD_IDDCX_ROOT")
        .expect("set RLVD_IDDCX_ROOT to the IddCx 1.11 files: dot-source driver-env.ps1");
    let dir = std::path::Path::new(&root).join("lib");
    assert!(
        dir.join("IddCxStub.lib").is_file(),
        "{} has no IddCxStub.lib; run fetch-iddcx.ps1",
        dir.display()
    );
    println!("cargo:rustc-link-search={}", dir.display());
    println!("cargo:rustc-link-lib=static=IddCxStub");
}
