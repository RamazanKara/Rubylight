#[path = "../version_resource.rs"]
mod version_resource;

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let rc = version_resource::write(
        "protocol-probes",
        "Rubylight protocol performance probes",
        false,
    );
    embed_resource::compile_for_examples(&rc, embed_resource::NONE)
        .manifest_optional()
        .unwrap();
}
