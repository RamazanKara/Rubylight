//! Links the app's prebuilt libopus (the same archive libmoonlight-core.so uses) and liblog
//! into libnightjar_core.so. Host builds (unit tests) link neither; see `src/opus.rs`.

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=NIGHTJAR_OPUS_DIR");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("android") {
        return;
    }
    let abi = match std::env::var("CARGO_CFG_TARGET_ARCH").unwrap().as_str() {
        "aarch64" => "arm64-v8a",
        "arm" => "armeabi-v7a",
        "x86" => "x86",
        "x86_64" => "x86_64",
        other => panic!("no Android ABI for {other}"),
    };
    let dir = std::env::var_os("NIGHTJAR_OPUS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap())
                .join("../../app/src/main/jni/moonlight-core/opus")
        })
        .join(abi);
    assert!(
        dir.join("libopus.a").exists(),
        "libopus.a not found in {}",
        dir.display()
    );
    println!("cargo:rerun-if-changed={}", dir.join("libopus.a").display());
    println!("cargo:rustc-link-search=native={}", dir.display());
    println!("cargo:rustc-link-lib=static=opus");
    println!("cargo:rustc-link-lib=log");
    println!("cargo:rustc-link-lib=m");
}
