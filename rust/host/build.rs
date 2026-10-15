include!("../assets/version_info.rs");

// Embeds the Rubylight icon (rust/assets/butterpollo.ico) as icon resource 1
// of every binary in this crate: Explorer, Start, the taskbar and shortcuts
// show it, and the tray loads it from the executable. Each binary also
// carries version information with its own description.
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
    for (bin, description) in [
        ("butterpollo", "Rubylight host"),
        ("butterpollo-service", "Rubylight service"),
        ("butterpollo-start", "Start Rubylight"),
    ] {
        let rc = write_resource(bin, &icon, description, false);
        embed_resource::compile_for(&rc, [bin], embed_resource::NONE)
            .manifest_optional()
            .unwrap();
    }
}
