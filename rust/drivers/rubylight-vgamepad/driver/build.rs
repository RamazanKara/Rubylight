//! WDK link flags, VHF/CM libraries, and Rubylight VERSIONINFO resource for rubylight_vgamepad.dll.
include!("../../../assets/version_info.rs");

fn main() -> Result<(), wdk_build::ConfigError> {
    // Vendored wdk-build leaves /INTEGRITYCHECK off for UMDF so WUDFHost accepts release and
    // local-signed catalogs alike.
    wdk_build::configure_wdk_binary_build()?;
    println!("cargo:rustc-link-lib=VhfUm");
    println!("cargo:rustc-link-lib=cfgmgr32");
    link_version_resource();
    Ok(())
}

/// Compile the VERSIONINFO with the SDK's rc.exe and pass the .res to the MSVC linker.
fn link_version_resource() {
    println!("cargo:rerun-if-changed=../../../assets/version_info.rs");
    let rc = write_resource(
        "rubylight-vgamepad",
        Some("rubylight_vgamepad.dll"),
        "",
        "Rubylight virtual gamepad driver",
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

/// rc.exe from the newest Windows Kit, or from PATH (developer shell).
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
