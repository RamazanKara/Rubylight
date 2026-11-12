#[path = "src/version_name.rs"]
mod version_name;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // The release version is Android's versionName, so both clients report the same one.
    let gradle = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../app/build.gradle");
    println!("cargo:rerun-if-changed={}", gradle.display());
    println!("cargo:rerun-if-changed=src/version_name.rs");
    let gradle_text = std::fs::read_to_string(&gradle)
        .unwrap_or_else(|e| panic!("read {}: {e}", gradle.display()));
    let version = version_name::parse(&gradle_text)
        .unwrap_or_else(|| panic!("no versionName \"<version>\" line in {}", gradle.display()));
    println!("cargo:rustc-env=NIGHTJAR_VERSION={version}");
    println!("cargo:rerun-if-env-changed=NIGHTJAR_FFMPEG_DIR");
    let windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    if windows {
        // packaging/icons/nightjar.ico (packaging/make-icons.py) becomes icon resource 1 of
        // nightjar.exe: Explorer, the taskbar, pins and the window (SDL takes the executable's
        // first icon) show it. Without windres (MSYS2 binutils) or rc.exe the build fails rather
        // than ship an exe with no icon at all.
        // packaging/nightjar.exe.manifest becomes manifest resource 1 (RT_MANIFEST is 24) in the
        // same script; tests/manifest.rs reads it back from the built exe.
        let icon = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("packaging/icons/nightjar.ico");
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("packaging/nightjar.exe.manifest");
        println!("cargo:rerun-if-changed={}", icon.display());
        println!("cargo:rerun-if-changed={}", manifest.display());
        let rc = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("nightjar.rc");
        let path = icon.display().to_string().replace('\\', "/");
        let manifest_path = manifest.display().to_string().replace('\\', "/");
        std::fs::write(&rc, format!("1 ICON \"{path}\"\n1 24 \"{manifest_path}\"\n"))
            .unwrap_or_else(|e| panic!("write {}: {e}", rc.display()));
        embed_resource::compile_for(&rc, ["nightjar"], embed_resource::NONE)
            .manifest_required()
            .unwrap_or_else(|e| panic!("embed the icon and manifest (needs windres or rc.exe on PATH): {e}"));
    }
    // MSYS2's UCRT64 on x86_64 PCs; ARM64 Windows has CLANGARM64 (clang, libc++, the UCRT).
    let msys = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => r"C:\msys64\clangarm64",
        _ => r"C:\msys64\ucrt64",
    };
    let prefix = std::env::var("NIGHTJAR_FFMPEG_DIR")
        .ok()
        .or_else(|| windows.then(|| msys.to_owned()));
    let mut bindings = bindgen::Builder::default()
        .header_contents("nightjar-media.h", if windows {
            "#include <libavcodec/avcodec.h>\n#include <libavutil/hwcontext.h>\n#include <libavutil/pixdesc.h>\n#include <libavutil/hwcontext_d3d11va.h>\n#include <libavutil/hdr_dynamic_metadata.h>\n#include <libavutil/dovi_meta.h>\n"
        } else {
            "#include <libavcodec/avcodec.h>\n#include <libavutil/hwcontext.h>\n#include <libavutil/pixdesc.h>\n"
        })
        .allowlist_type("AV.*|RcOverride|UINT")
        .allowlist_function("avcodec_.*|av_packet_.*|av_frame_.*|av_hwdevice_.*|av_hwframe_map|av_hwframe_transfer_data|av_pix_fmt_desc_get|av_get_pix_fmt_name|av_strerror|av_buffer_unref|avutil_version|av_buffer_pool_.*|av_dict_set|av_dict_free|av_dovi_find_level")
        .allowlist_var("AV_.*|FF_THREAD_SLICE")
        .allowlist_recursively(false)
        .blocklist_type("ID3D.*")
        .prepend_enum_name(false)
        .layout_tests(false)
        .generate_comments(false)
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()));
    // clang has no "gnullvm" environment, which bindgen passes on when it cross-compiles (an ARM64
    // build from an x86_64 PC): the MinGW triple clang knows stands for the same ABI.
    if let Some(arch) = std::env::var("TARGET").ok().and_then(|t| t.strip_suffix("-pc-windows-gnullvm").map(str::to_owned)) {
        bindings = bindings.clang_arg(format!("--target={arch}-w64-windows-gnu"));
    }
    if windows {
        for name in [
            "ID3D11Device",
            "ID3D11DeviceContext",
            "ID3D11VideoDevice",
            "ID3D11VideoContext",
            "ID3D11Texture2D",
        ] {
            bindings = bindings.raw_line(format!("pub type {name} = std::ffi::c_void;"));
        }
    }
    match prefix {
        Some(prefix) => {
            bindings = bindings.clang_arg(format!("-I{prefix}/include"));
            println!("cargo:rustc-link-search=native={prefix}/lib");
            // Windows packages bring Opus in the same prefix. On Linux Opus is the system's
            // (libopus.so.0 keeps its soname) and the package ships FFmpeg in lib/ beside the
            // executable: the run path finds it there, whatever FFmpeg the machine has.
            let libraries: &[&str] = if windows {
                &["avcodec", "avutil", "opus"]
            } else {
                &["avcodec", "avutil"]
            };
            for library in libraries {
                println!("cargo:rustc-link-lib=dylib={library}");
            }
            if !windows {
                pkg_config::Config::new()
                    .probe("opus")
                    .unwrap_or_else(|e| panic!("opus not found: {e}"));
                if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
                    println!("cargo:rustc-link-arg-bins=-Wl,-rpath,$ORIGIN/lib");
                }
            }
        }
        None => {
            for library in ["libavcodec", "libavutil", "opus"] {
                let found = pkg_config::Config::new()
                    .probe(library)
                    .unwrap_or_else(|e| panic!("{library} not found: {e}"));
                for path in found.include_paths {
                    bindings = bindings.clang_arg(format!("-I{}", path.display()));
                }
            }
        }
    }
    bindings
        .generate()
        .expect("FFmpeg bindings")
        .write_to_file(
            std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("media.rs"),
        )
        .expect("write FFmpeg bindings");
}
