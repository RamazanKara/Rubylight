//! Compiles `src/media_shim.c` (libavcodec video decoding and libopus) and links FFmpeg and
//! Opus. `NIGHTJAR_FFMPEG_DIR` names a prefix with `include/` and `lib/` holding both; the
//! default is MSYS2's `C:\msys64\ucrt64` on Windows and pkg-config elsewhere.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/media_shim.c");
    println!("cargo:rerun-if-env-changed=NIGHTJAR_FFMPEG_DIR");
    let windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    let prefix = std::env::var("NIGHTJAR_FFMPEG_DIR")
        .ok()
        .or_else(|| windows.then(|| r"C:\msys64\ucrt64".to_owned()));
    let mut build = cc::Build::new();
    if windows {
        println!("cargo:rerun-if-changed=src/packed444_d3d11.c");
        build.file("src/packed444_d3d11.c");
        println!("cargo:rustc-link-lib=d3dcompiler");
    }
    build.file("src/media_shim.c").warnings(true);
    let mut probed = Vec::new();
    match &prefix {
        Some(prefix) => {
            build.include(format!("{prefix}/include"));
            build.include(format!("{prefix}/include/opus"));
        }
        None => {
            for library in ["libavcodec", "libavutil", "opus"] {
                let found = pkg_config::Config::new()
                    .cargo_metadata(false)
                    .probe(library)
                    .unwrap_or_else(|e| {
                        panic!("{library} not found ({e}); install its -dev package or set NIGHTJAR_FFMPEG_DIR")
                    });
                for path in &found.include_paths {
                    build.include(path);
                }
                probed.push(found);
            }
        }
    }
    // The shim first, then the libraries it needs: GNU ld resolves left to right.
    build.compile("nightjar_media_shim");
    match &prefix {
        Some(prefix) => {
            println!("cargo:rustc-link-search=native={prefix}/lib");
            for library in ["avcodec", "avutil", "opus"] {
                println!("cargo:rustc-link-lib=dylib={library}");
            }
        }
        None => {
            for found in probed {
                for path in found.link_paths {
                    println!("cargo:rustc-link-search=native={}", path.display());
                }
                for library in found.libs {
                    println!("cargo:rustc-link-lib={library}");
                }
            }
        }
    }
}
