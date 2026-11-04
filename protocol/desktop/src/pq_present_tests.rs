//! The PQ present pass's shaders (`pq_present.hlsl`): the SPIR-V the client hands the patched SDL
//! is what the HLSL compiles to, and on a Vulkan device it writes scRGB as BT.2020 PQ.

use super::*;

/// The glslangValidator lines in the HLSL's header, as arguments.
fn glslang_commands(hlsl: &str) -> Vec<Vec<&str>> {
    hlsl.lines()
        .filter_map(|line| line.strip_prefix("//").map(str::trim_start))
        .filter(|line| line.starts_with("glslangValidator "))
        .map(|line| line.split_whitespace().skip(1).collect())
        .collect()
}

#[test]
fn the_header_names_one_glslang_command_per_shader() {
    let commands = glslang_commands(include_str!("pq_present.hlsl"));
    let outputs: Vec<&str> = commands.iter().filter_map(|args| args.iter().position(|a| *a == "-o").map(|i| args[i + 1])).collect();
    assert_eq!(outputs, ["pq_present_vertex.spv", "pq_present_fragment.spv"]);
}

/// Why the SPIR-V cannot be compared with the glslangValidator `--version` printed (None if it
/// can): not installed, or not the major version `pinned` that built it.
fn glslang_mismatch(installed: Option<&str>, pinned: &str) -> Option<String> {
    let Some(installed) = installed else {
        return Some("glslangValidator not found".into());
    };
    // "Glslang Version: 11:15.1.0": the major version after the colon.
    let major = installed.rsplit(':').next().and_then(|v| v.split('.').next()).unwrap_or("");
    (major != pinned).then(|| format!("{installed} is not the glslang {pinned} that built the SPIR-V"))
}

#[test]
fn a_missing_or_other_glslang_is_named() {
    assert_eq!(glslang_mismatch(Some("Glslang Version: 11:15.1.0"), "15"), None);
    assert_eq!(glslang_mismatch(Some("Glslang Version: 11:16.0.0"), "15").unwrap(), "Glslang Version: 11:16.0.0 is not the glslang 15 that built the SPIR-V");
    assert_eq!(glslang_mismatch(None, "15").unwrap(), "glslangValidator not found");
}

/// The checked-in SPIR-V is the HLSL compiled with the header's own commands. Needs the
/// glslangValidator major version the header names on PATH (`glslang-tools` 15); without it
/// the test says so and passes, unless NIGHTJAR_REQUIRE_GLSLANG=1 (CI's desktop job) makes
/// that a failure.
#[test]
fn the_spirv_is_the_hlsl_compiled() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let hlsl = include_str!("pq_present.hlsl");
    let pinned = hlsl.lines().find_map(|line| line.split_once("from glslang ").map(|(_, v)| v.split_whitespace().next().unwrap())).expect("the header names the glslang version");
    let installed = match std::process::Command::new("glslangValidator").arg("--version").output() {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => panic!("glslangValidator: {e}"),
        Ok(run) => Some(String::from_utf8_lossy(&run.stdout).lines().next().unwrap_or("").trim().to_owned()),
    };
    if let Some(why) = glslang_mismatch(installed.as_deref(), pinned) {
        assert!(std::env::var_os("NIGHTJAR_REQUIRE_GLSLANG").is_none_or(|v| v != "1"), "{why}, and NIGHTJAR_REQUIRE_GLSLANG=1 asks for the comparison");
        eprintln!("{why}: the SPIR-V is not compared");
        return;
    }
    let out = std::env::temp_dir().join(format!("nightjar-pq-spirv-{}", std::process::id()));
    std::fs::create_dir_all(&out).unwrap();
    for args in glslang_commands(hlsl) {
        let output = args[args.iter().position(|a| *a == "-o").unwrap() + 1];
        let args: Vec<std::ffi::OsString> = args
            .iter()
            .map(|a| match *a {
                "pq_present.hlsl" => dir.join(a).into_os_string(),
                a if a == output => out.join(a).into_os_string(),
                a => a.into(),
            })
            .collect();
        let run = std::process::Command::new("glslangValidator").args(&args).output().expect("glslangValidator");
        assert!(run.status.success(), "glslangValidator {args:?}: {}", String::from_utf8_lossy(&run.stdout));
        let built = std::fs::read(out.join(output)).unwrap();
        let checked_in = match output {
            "pq_present_vertex.spv" => PQ_VERTEX_SPIRV,
            _ => PQ_FRAGMENT_SPIRV,
        };
        assert!(built == checked_in, "{output} is not pq_present.hlsl compiled: rebuild it with the header's command");
    }
    let _ = std::fs::remove_dir_all(&out);
}

/// ST 2084's encode of `nits`, written apart from the shader's.
#[cfg(unix)]
fn pq(nits: f64) -> f64 {
    let (m1, m2) = (0.1593017578125, 78.84375);
    let (c1, c2, c3) = (0.8359375, 18.8515625, 18.6875);
    let y = (nits / 10000.0).clamp(0.0, 1.0).powf(m1);
    ((c1 + c2 * y) / (1.0 + c3 * y)).powf(m2)
}

/// The pass on a Vulkan device (so ignored: `cargo test -p nightjar-desktop -- --ignored
/// pq_present`): four scRGB pixels in RGBA16F, drawn into an RGB10A2 target as the patched SDL
/// does onto an HDR10 swapchain, read back. 2.5 (200 nits) white, BT.709 red at 80 nits, black,
/// and 125 (10000 nits) come out as BT.2020 PQ to the 10-bit code.
#[cfg(unix)]
#[test]
#[ignore = "needs a Vulkan device"]
fn the_pq_pass_writes_scrgb_as_bt2020_pq() {
    // RGBA16F: 2.5 = 0x4100, 1.0 = 0x3C00, 125.0 = 0x57D0.
    let source: [[u16; 4]; 4] = [[0x4100, 0x4100, 0x4100, 0x3C00], [0x3C00, 0, 0, 0x3C00], [0, 0, 0, 0x3C00], [0x57D0, 0x57D0, 0x57D0, 0x3C00]];
    let scrgb = [[2.5, 2.5, 2.5], [1.0, 0.0, 0.0], [0.0, 0.0, 0.0], [125.0, 125.0, 125.0]];
    let to2020 = [[0.6274040, 0.3292820, 0.0433136], [0.0690970, 0.9195400, 0.0113612], [0.0163916, 0.0880132, 0.8955950]];
    let width = source.len() as u32;
    // SAFETY: plain SDL GPU calls on objects made here, on one thread; the mapped buffers are
    // as large as written and read.
    let packed: Vec<u32> = unsafe {
        let _sdl = TEST_LOCK.lock().unwrap();
        SDL_SetHint(SDL_HINT_VIDEO_DRIVER, c"offscreen".as_ptr());
        assert!(SDL_Init(SDL_INIT_VIDEO), "{}", error());
        let device = SDL_CreateGPUDevice(SDL_GPU_SHADERFORMAT_SPIRV, false, c"vulkan".as_ptr());
        assert!(!device.is_null(), "{}", error());
        let shader = |code: &[u8], entry: &CStr, stage, samplers| {
            let mut info: SDL_GPUShaderCreateInfo = std::mem::zeroed();
            info.code_size = code.len();
            info.code = code.as_ptr();
            info.entrypoint = entry.as_ptr();
            info.format = SDL_GPU_SHADERFORMAT_SPIRV;
            info.stage = stage;
            info.num_samplers = samplers;
            let shader = SDL_CreateGPUShader(device, &info);
            assert!(!shader.is_null(), "{}", error());
            shader
        };
        let vertex = shader(PQ_VERTEX_SPIRV, c"vertex", SDL_GPU_SHADERSTAGE_VERTEX, 0);
        let fragment = shader(PQ_FRAGMENT_SPIRV, c"fragment", SDL_GPU_SHADERSTAGE_FRAGMENT, 1);
        let mut target_description: SDL_GPUColorTargetDescription = std::mem::zeroed();
        target_description.format = SDL_GPU_TEXTUREFORMAT_R10G10B10A2_UNORM;
        let mut pipeline_info: SDL_GPUGraphicsPipelineCreateInfo = std::mem::zeroed();
        pipeline_info.vertex_shader = vertex;
        pipeline_info.fragment_shader = fragment;
        pipeline_info.primitive_type = SDL_GPU_PRIMITIVETYPE_TRIANGLELIST;
        pipeline_info.target_info.color_target_descriptions = &target_description;
        pipeline_info.target_info.num_color_targets = 1;
        let pipeline = SDL_CreateGPUGraphicsPipeline(device, &pipeline_info);
        assert!(!pipeline.is_null(), "{}", error());
        let texture = |format, usage| {
            let mut info: SDL_GPUTextureCreateInfo = std::mem::zeroed();
            info.r#type = SDL_GPU_TEXTURETYPE_2D;
            info.format = format;
            info.usage = usage;
            info.width = width;
            info.height = 1;
            info.layer_count_or_depth = 1;
            info.num_levels = 1;
            let texture = SDL_CreateGPUTexture(device, &info);
            assert!(!texture.is_null(), "{}", error());
            texture
        };
        let frame = texture(SDL_GPU_TEXTUREFORMAT_R16G16B16A16_FLOAT, SDL_GPU_TEXTUREUSAGE_SAMPLER);
        let target = texture(SDL_GPU_TEXTUREFORMAT_R10G10B10A2_UNORM, SDL_GPU_TEXTUREUSAGE_COLOR_TARGET);
        let mut sampler_info: SDL_GPUSamplerCreateInfo = std::mem::zeroed();
        sampler_info.min_filter = SDL_GPU_FILTER_NEAREST;
        sampler_info.mag_filter = SDL_GPU_FILTER_NEAREST;
        sampler_info.address_mode_u = SDL_GPU_SAMPLERADDRESSMODE_CLAMP_TO_EDGE;
        sampler_info.address_mode_v = SDL_GPU_SAMPLERADDRESSMODE_CLAMP_TO_EDGE;
        let sampler = SDL_CreateGPUSampler(device, &sampler_info);
        assert!(!sampler.is_null(), "{}", error());
        let buffer = |usage, size: usize| {
            let mut info: SDL_GPUTransferBufferCreateInfo = std::mem::zeroed();
            info.usage = usage;
            info.size = size as u32;
            let buffer = SDL_CreateGPUTransferBuffer(device, &info);
            assert!(!buffer.is_null(), "{}", error());
            buffer
        };
        let upload = buffer(SDL_GPU_TRANSFERBUFFERUSAGE_UPLOAD, std::mem::size_of_val(&source));
        let download = buffer(SDL_GPU_TRANSFERBUFFERUSAGE_DOWNLOAD, width as usize * 4);
        let mapped = SDL_MapGPUTransferBuffer(device, upload, false).cast::<[u16; 4]>();
        assert!(!mapped.is_null(), "{}", error());
        std::ptr::copy_nonoverlapping(source.as_ptr(), mapped, source.len());
        SDL_UnmapGPUTransferBuffer(device, upload);

        let region = |texture| {
            let mut region: SDL_GPUTextureRegion = std::mem::zeroed();
            region.texture = texture;
            region.w = width;
            region.h = 1;
            region.d = 1;
            region
        };
        let transfer = |buffer| {
            let mut info: SDL_GPUTextureTransferInfo = std::mem::zeroed();
            info.transfer_buffer = buffer;
            info
        };
        let commands = SDL_AcquireGPUCommandBuffer(device);
        assert!(!commands.is_null(), "{}", error());
        let copy = SDL_BeginGPUCopyPass(commands);
        SDL_UploadToGPUTexture(copy, &transfer(upload), &region(frame), false);
        SDL_EndGPUCopyPass(copy);
        let mut color: SDL_GPUColorTargetInfo = std::mem::zeroed();
        color.texture = target;
        color.load_op = SDL_GPU_LOADOP_CLEAR;
        color.store_op = SDL_GPU_STOREOP_STORE;
        let pass = SDL_BeginGPURenderPass(commands, &color, 1, std::ptr::null());
        SDL_BindGPUGraphicsPipeline(pass, pipeline);
        let binding = SDL_GPUTextureSamplerBinding { texture: frame, sampler };
        SDL_BindGPUFragmentSamplers(pass, 0, &binding, 1);
        SDL_DrawGPUPrimitives(pass, 3, 1, 0, 0);
        SDL_EndGPURenderPass(pass);
        let copy = SDL_BeginGPUCopyPass(commands);
        SDL_DownloadFromGPUTexture(copy, &region(target), &transfer(download));
        SDL_EndGPUCopyPass(copy);
        let fence = SDL_SubmitGPUCommandBufferAndAcquireFence(commands);
        assert!(!fence.is_null(), "{}", error());
        assert!(SDL_WaitForGPUFences(device, true, &fence, 1), "{}", error());
        SDL_ReleaseGPUFence(device, fence);
        let mapped = SDL_MapGPUTransferBuffer(device, download, false).cast::<u32>();
        assert!(!mapped.is_null(), "{}", error());
        let packed = std::slice::from_raw_parts(mapped, width as usize).to_vec();
        SDL_UnmapGPUTransferBuffer(device, download);
        SDL_ReleaseGPUTransferBuffer(device, upload);
        SDL_ReleaseGPUTransferBuffer(device, download);
        SDL_ReleaseGPUSampler(device, sampler);
        SDL_ReleaseGPUTexture(device, frame);
        SDL_ReleaseGPUTexture(device, target);
        SDL_ReleaseGPUGraphicsPipeline(device, pipeline);
        SDL_ReleaseGPUShader(device, vertex);
        SDL_ReleaseGPUShader(device, fragment);
        SDL_DestroyGPUDevice(device);
        SDL_Quit();
        packed
    };
    for (pixel, (packed, linear)) in packed.iter().zip(scrgb).enumerate() {
        // A2B10G10R10: red in the low bits.
        let got = [packed & 0x3ff, packed >> 10 & 0x3ff, packed >> 20 & 0x3ff];
        let want = to2020.map(|row: [f64; 3]| (pq(80.0 * row.iter().zip(linear).map(|(m, c)| m * c).sum::<f64>()) * 1023.0).round() as u32);
        eprintln!("pixel {pixel}: {linear:?} scRGB -> {got:?}, BT.2020 PQ {want:?}");
        for (g, w) in got.iter().zip(want) {
            assert!(g.abs_diff(w) <= 1, "pixel {pixel}: {got:?}, not BT.2020 PQ {want:?}");
        }
        assert_eq!(packed >> 30, 3, "opaque");
    }
}
