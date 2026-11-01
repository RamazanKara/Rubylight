include!("../assets/version_info.rs");

// The Rubylight icon (rust/assets/butterpollo.ico) and version information
// with the service's own version, not the release's (Cargo.toml). The link
// leaves out the time stamp, so the same sources give the same file.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    match std::env::var("CARGO_CFG_TARGET_ENV").as_deref() {
        Ok("gnu") => println!("cargo:rustc-link-arg-bins=-Wl,--no-insert-timestamp"),
        Ok("msvc") => println!("cargo:rustc-link-arg-bins=/Brepro"),
        _ => {}
    }
    let manifest = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let icon = manifest.join("../assets/butterpollo.ico");
    println!("cargo:rerun-if-changed={}", icon.display());
    println!("cargo:rerun-if-changed=../assets/version_info.rs");
    let path = icon.display().to_string().replace('\\', "/");
    let icon = format!("1 ICON \"{path}\"\n");
    let rc = write_versioned_resource(
        "butterpollo-service",
        Some("butterpollo-service.exe"),
        &icon,
        "Rubylight service",
        false,
        env!("CARGO_PKG_VERSION"),
    );
    embed_resource::compile_for(&rc, ["butterpollo-service"], embed_resource::NONE)
        .manifest_optional()
        .unwrap();
}
