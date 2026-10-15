include!("../assets/version_info.rs");

// The protocol and video packet probes are examples of this crate and ship in
// the package as butterpollo-protocol-performance.exe and
// butterpollo-video-packet-performance.exe; their version information names
// the release.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    println!("cargo:rerun-if-changed=../assets/version_info.rs");
    let rc = write_resource("examples", "", "Rubylight diagnostic tool", false);
    embed_resource::compile_for_examples(&rc, embed_resource::NONE)
        .manifest_optional()
        .unwrap();
}
