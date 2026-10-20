//! With the `ffmpeg` feature: compiles `src/ffmpeg_shim.c` and links libavcodec, libavutil
//! and libopus. `NIGHTJAR_FFMPEG_DIR` names a prefix with `include/` and `lib/` (default
//! `C:\msys64\ucrt64` on Windows, the system paths elsewhere).

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    #[cfg(feature = "ffmpeg")]
    ffmpeg();
}

#[cfg(feature = "ffmpeg")]
fn ffmpeg() {
    println!("cargo:rerun-if-changed=src/ffmpeg_shim.c");
    println!("cargo:rerun-if-env-changed=NIGHTJAR_FFMPEG_DIR");
    let windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    let prefix = std::env::var("NIGHTJAR_FFMPEG_DIR")
        .ok()
        .or_else(|| windows.then(|| r"C:\msys64\ucrt64".to_owned()));
    let mut build = cc::Build::new();
    build.file("src/ffmpeg_shim.c").warnings(true);
    if let Some(prefix) = &prefix {
        build.include(format!("{prefix}/include"));
        println!("cargo:rustc-link-search=native={prefix}/lib");
    }
    build.compile("nightjar_ffmpeg_shim");
    for library in ["avcodec", "avutil", "opus"] {
        println!("cargo:rustc-link-lib=dylib={library}");
    }
}
