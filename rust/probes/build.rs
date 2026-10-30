include!("../assets/version_info.rs");

// The probes ship in the package; their version information names the release.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    println!("cargo:rerun-if-changed=../assets/version_info.rs");
    let rc = write_resource("probes", "", "Rubylight diagnostic tool", false);
    embed_resource::compile(&rc, embed_resource::NONE)
        .manifest_optional()
        .unwrap();
}
