include!("../assets/version_info.rs");

// Embeds the Rubylight icon (rust/assets/butterpollo.ico) as icon resource 1
// of every binary in this crate: Explorer, Start, the taskbar and shortcuts
// show it, and the tray loads it from the executable. The version
// information names the release; the installer and uninstall.exe are this
// binary.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let manifest = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let icon = manifest.join("../assets/butterpollo.ico");
    println!("cargo:rerun-if-changed={}", icon.display());
    println!("cargo:rerun-if-changed=../assets/version_info.rs");
    let path = icon.display().to_string().replace('\\', "/");
    let icon = format!("1 ICON \"{path}\"\n");
    let rc = write_resource("setup", &icon, "Rubylight Setup", false);
    embed_resource::compile(&rc, embed_resource::NONE)
        .manifest_optional()
        .unwrap();
}
