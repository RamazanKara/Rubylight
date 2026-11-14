include!("../assets/version_info.rs");

// Links NVIDIA's NGX library from the RTX Video SDK and gives the adapter DLL
// the release's version information. Only Rubylight's separate, unsigned
// NVIDIA build (`rust/build.ps1 -Nvidia`) builds this crate.
fn main() {
    if !std::env::var("TARGET").unwrap().ends_with("windows-msvc") {
        panic!("NVIDIA's vendor library requires --target x86_64-pc-windows-msvc");
    }
    println!("cargo:rerun-if-env-changed=NV_RTX_VIDEO_SDK");
    let sdk = std::path::PathBuf::from(
        std::env::var("NV_RTX_VIDEO_SDK").expect("set NV_RTX_VIDEO_SDK to RTX Video SDK 1.1.0"),
    );
    for header in ["nvsdk_ngx_defs_truehdr.h", "nvsdk_ngx_defs_vsr.h"] {
        assert!(
            sdk.join("include").join(header).exists(),
            "SDK header {header} is missing"
        );
    }
    println!(
        "cargo:rustc-link-search=native={}",
        sdk.join("lib/Windows/x64").display()
    );
    // NVIDIA's library comes for the static C runtime (_s) and its DLLs (_d). The static one
    // spares the adapter the Visual C++ Redistributable, which a PC need not have;
    // rust/build.ps1 builds the adapter with crt-static.
    let static_crt = std::env::var("CARGO_CFG_TARGET_FEATURE")
        .is_ok_and(|features| features.split(',').any(|feature| feature == "crt-static"));
    let ngx = if static_crt {
        "nvsdk_ngx_s"
    } else {
        "nvsdk_ngx_d"
    };
    println!("cargo:rustc-link-lib=static={ngx}");
    println!("cargo:rustc-link-lib=advapi32");
    println!("cargo:rustc-link-lib=user32");
    println!("cargo:rerun-if-changed=../assets/version_info.rs");
    let rc = write_resource(
        "truehdr",
        Some("butterpollo_truehdr.dll"),
        "",
        "Rubylight NVIDIA image enhancement adapter",
        true,
    );
    embed_resource::compile_for_cdylib(&rc, embed_resource::NONE)
        .manifest_optional()
        .unwrap();
}
