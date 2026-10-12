#[path = "../version_resource.rs"]
mod version_resource;

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let rc = version_resource::write("setup", "Rubylight setup and uninstaller", false);
    embed_resource::compile(&rc, embed_resource::NONE)
        .manifest_optional()
        .unwrap();
}
