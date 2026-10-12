#[path = "../version_resource.rs"]
mod version_resource;

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    for (name, description) in [
        (
            "butterpollo",
            "Rubylight streaming host and session helpers",
        ),
        ("butterpollo-service", "Rubylight Windows service"),
        ("butterpollo-start", "Rubylight launcher"),
    ] {
        let rc = version_resource::write(name, description, false);
        embed_resource::compile_for(&rc, [name], embed_resource::NONE)
            .manifest_optional()
            .unwrap();
    }
}
