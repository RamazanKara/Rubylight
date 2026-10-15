include!("../assets/version_info.rs");

// The layer DLL's version information names the release.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    println!("cargo:rerun-if-changed=../assets/version_info.rs");
    let rc = write_resource("vulkan-layer", "", "Rubylight Vulkan HDR layer", true);
    embed_resource::compile_for_cdylib(&rc, embed_resource::NONE)
        .manifest_optional()
        .unwrap();
}
