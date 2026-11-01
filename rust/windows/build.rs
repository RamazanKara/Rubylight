use std::{env, path::PathBuf};
#[path = "build_shaders.rs"]
mod shaders;
include!("../assets/version_info.rs");
fn main() {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    // The diagnostic examples name the release in their version information.
    println!("cargo:rerun-if-changed=../assets/version_info.rs");
    // One resource for every example, so it names no file.
    let rc = write_resource("examples", None, "", "Rubylight diagnostic tool", false);
    embed_resource::compile_for_examples(&rc, embed_resource::NONE)
        .manifest_optional()
        .unwrap();
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    shaders::compile(&out);
    // AMD's AMF headers, vendored in include/AMF.
    println!("cargo:rerun-if-changed=include/AMF");
    let amf_include = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("include");
    let amf=bindgen::Builder::default().header_contents("amf.h","#include <AMF/core/Factory.h>\n#include <AMF/components/VideoEncoderVCE.h>\n#include <AMF/components/VideoEncoderHEVC.h>\n#include <AMF/components/VideoEncoderAV1.h>\n")
        .clang_arg(format!("-I{}",amf_include.display())).allowlist_type("AMF.*|amf_.*")
        .allowlist_var("AMF.*|amf_.*").layout_tests(false).derive_debug(false).generate_comments(false).generate().expect("generate AMF C ABI");
    amf.write_to_file(out.join("amf.rs")).unwrap();
    // NVIDIA's NVENC API 13.0 and FFmpeg's dynamic CUDA loader headers, vendored in include.
    println!("cargo:rerun-if-changed=include/nvEncodeAPI.h");
    println!("cargo:rerun-if-changed=include/dynlink_cuda.h");
    for (header, file, types, vars) in [
        (
            "nvEncodeAPI.h",
            "nvenc.rs",
            "NV.*|GUID|_GUID|CONTENT_LIGHT_LEVEL|MASTERING_DISPLAY_INFO|CHROMA_POINTS",
            "NV.*",
        ),
        (
            "dynlink_cuda.h",
            "cuda.rs",
            "CU.*|CUDA.*|tcu.*",
            "CU.*|CUDA.*",
        ),
    ] {
        let bindings = bindgen::Builder::default()
            .header(format!("include/{header}"))
            .allowlist_type(types)
            .allowlist_var(vars)
            .blocklist_var("NV_ENC_.*_GUID")
            .opaque_type("_?NVENC_EXTERNAL_ME_(SB_)?HINT")
            .layout_tests(false)
            .derive_debug(false)
            .derive_default(true)
            .generate_comments(false)
            .generate()
            .expect("generate pinned NVIDIA driver ABI");
        bindings.write_to_file(out.join(file)).unwrap();
    }
    // bindgen treats C's static GUIDs as extern symbols. Materialize the pinned
    // header values so the runtime adapter never needs NVIDIA's import library.
    let header = std::fs::read_to_string("include/nvEncodeAPI.h").unwrap();
    let mut guids = String::new();
    for item in header.split("static const GUID").skip(1) {
        let (name, value) = item.split_once('=').expect("GUID declaration");
        let value = value.split(';').next().unwrap();
        let fields: Vec<_> = value
            .split(|c: char| matches!(c, '{' | '}' | ',') || c.is_whitespace())
            .filter(|v| !v.is_empty())
            .collect();
        assert_eq!(fields.len(), 11, "GUID {}", name.trim());
        guids.push_str(&format!(
            "pub const {}: GUID = GUID {{ Data1: {}, Data2: {}, Data3: {}, Data4: [{}] }};
",
            name.trim(),
            fields[0],
            fields[1],
            fields[2],
            fields[3..].join(",")
        ));
    }
    std::fs::write(out.join("nvenc_guids.rs"), guids).unwrap();
    println!("cargo:rerun-if-env-changed=BUTTERPOLLO_PYROWAVE_ROOT");
    println!("cargo:rerun-if-env-changed=BUTTERPOLLO_VULKAN_INCLUDE");
    let pyro = PathBuf::from(
        env::var("BUTTERPOLLO_PYROWAVE_ROOT")
            .expect("set BUTTERPOLLO_PYROWAVE_ROOT to the pinned PyroWave 1.0 SDK"),
    );
    let vulkan = env::var("BUTTERPOLLO_VULKAN_INCLUDE")
        .expect("set BUTTERPOLLO_VULKAN_INCLUDE to Vulkan's include directory");
    let bindings = bindgen::Builder::default()
        .header_contents("pyro.h", "#include <vulkan/vulkan.h>\n#include <pyrowave/pyrowave.h>\n")
        .clang_arg(format!("-I{}", pyro.join("include").display()))
        .clang_arg(format!("-I{vulkan}"))
        .allowlist_type("pyrowave_.*|VkQueueGlobalPriority|VkQueueFlagBits|VkImageUsageFlagBits|VkSemaphoreWaitInfo|PFN_vkGetDeviceProcAddr|PFN_vkWaitSemaphores")
        .allowlist_var("PYROWAVE_.*|VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO|VK_IMAGE_TYPE_2D|VK_FORMAT_.*|VK_IMAGE_.*|VK_SAMPLE_COUNT_1_BIT|VK_SHARING_MODE_EXCLUSIVE|VK_QUEUE_.*|VK_EXTERNAL_MEMORY_HANDLE_TYPE_D3D11_TEXTURE_BIT|VK_EXTERNAL_SEMAPHORE_HANDLE_TYPE_D3D12_FENCE_BIT|VK_SEMAPHORE_TYPE_TIMELINE|VK_COLOR_SPACE_.*")
        .layout_tests(false).derive_debug(false).generate_comments(false)
        .generate().expect("generate PyroWave C ABI");
    bindings.write_to_file(out.join("pyrowave.rs")).unwrap();
}
