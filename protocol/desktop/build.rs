fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=NIGHTJAR_FFMPEG_DIR");
    let windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    let prefix = std::env::var("NIGHTJAR_FFMPEG_DIR")
        .ok()
        .or_else(|| windows.then(|| r"C:\msys64\ucrt64".to_owned()));
    let mut bindings = bindgen::Builder::default()
        .header_contents("nightjar-media.h", if windows {
            "#include <libavcodec/avcodec.h>\n#include <libavutil/hwcontext.h>\n#include <libavutil/pixdesc.h>\n#include <libavutil/hwcontext_d3d11va.h>\n"
        } else {
            "#include <libavcodec/avcodec.h>\n#include <libavutil/hwcontext.h>\n#include <libavutil/pixdesc.h>\n"
        })
        .allowlist_type("AV.*|RcOverride|UINT")
        .allowlist_function("avcodec_.*|av_packet_.*|av_frame_.*|av_hwdevice_.*|av_hwframe_transfer_data|av_pix_fmt_desc_get|av_get_pix_fmt_name|av_strerror|av_new_packet|av_buffer_unref|av_dict_set|av_dict_free")
        .allowlist_var("AV_.*|FF_THREAD_SLICE")
        .allowlist_recursively(false)
        .blocklist_type("ID3D.*")
        .prepend_enum_name(false)
        .layout_tests(false)
        .generate_comments(false)
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()));
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
            for library in ["avcodec", "avutil", "opus"] {
                println!("cargo:rustc-link-lib=dylib={library}");
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
