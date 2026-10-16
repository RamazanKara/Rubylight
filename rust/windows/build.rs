use std::{env, path::PathBuf};
#[path = "build_shaders.rs"]
mod shaders;
include!("../assets/version_info.rs");
fn main() {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    // The performance probe example ships as butterpollo-performance.exe.
    println!("cargo:rerun-if-changed=../assets/version_info.rs");
    let rc = write_resource("examples", "", "Rubylight diagnostic tool", false);
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
        .allowlist_type("pyrowave_.*|VkQueueGlobalPriority|VkQueueFlagBits|VkImageUsageFlagBits")
        .allowlist_var("PYROWAVE_.*|VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO|VK_IMAGE_TYPE_2D|VK_FORMAT_.*|VK_IMAGE_.*|VK_SAMPLE_COUNT_1_BIT|VK_SHARING_MODE_EXCLUSIVE|VK_QUEUE_.*|VK_EXTERNAL_MEMORY_HANDLE_TYPE_D3D12_RESOURCE_BIT|VK_EXTERNAL_SEMAPHORE_HANDLE_TYPE_D3D12_FENCE_BIT|VK_SEMAPHORE_TYPE_TIMELINE|VK_COLOR_SPACE_.*")
        .layout_tests(false).derive_debug(false).generate_comments(false)
        .generate().expect("generate PyroWave C ABI");
    bindings.write_to_file(out.join("pyrowave.rs")).unwrap();
}
