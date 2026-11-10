//! The same SDL fragment passes compiled for the selected desktop GPU API.
use sdl3_sys::everything::*;

#[derive(Clone, Copy)]
pub(super) enum Shader {
    Upscale,
    Wavelet,
    #[cfg(unix)]
    Planes,
    Packed,
}

impl Shader {
    #[cfg(windows)]
    pub fn filter_hlsl(self) -> &'static [u8] {
        match self {
            Self::Upscale => include_bytes!("../upscale.hlsl"),
            Self::Wavelet => include_bytes!("../pyrowave.hlsl"),
            _ => unreachable!("not a texture filter"),
        }
    }

    pub fn info(self) -> SDL_GPUShaderCreateInfo {
        #[cfg(target_os = "macos")]
        let (code, entrypoint, format): (&[u8], _, _) = (
            match self {
                Self::Upscale => include_bytes!("../metal/upscale.metal"),
                Self::Wavelet => include_bytes!("../metal/pyrowave.metal"),
                Self::Planes => include_bytes!("../metal/pyrowave-planes.metal"),
                Self::Packed => include_bytes!("../metal/packed444.metal"),
            },
            c"nightjar_fragment",
            SDL_GPU_SHADERFORMAT_MSL,
        );
        #[cfg(not(target_os = "macos"))]
        let (code, entrypoint, format): (&[u8], _, _) = match self {
            Self::Upscale => (
                include_bytes!("../upscale.spv"),
                c"fragment",
                SDL_GPU_SHADERFORMAT_SPIRV,
            ),
            Self::Wavelet => (
                include_bytes!("../pyrowave.spv"),
                c"fragment",
                SDL_GPU_SHADERFORMAT_SPIRV,
            ),
            #[cfg(unix)]
            Self::Planes => (
                include_bytes!("../pyrowave-planes.spv"),
                c"planes",
                SDL_GPU_SHADERFORMAT_SPIRV,
            ),
            Self::Packed => (
                include_bytes!("../packed444.spv"),
                c"fragment",
                SDL_GPU_SHADERFORMAT_SPIRV,
            ),
        };
        SDL_GPUShaderCreateInfo {
            code: code.as_ptr(),
            code_size: code.len(),
            entrypoint: entrypoint.as_ptr(),
            format,
            stage: SDL_GPU_SHADERSTAGE_FRAGMENT,
            num_samplers: match self {
                Self::Packed => 0,
                #[cfg(unix)]
                Self::Planes => 3,
                _ => 1,
            },
            num_uniform_buffers: 1,
            num_storage_buffers: u32::from(matches!(self, Self::Packed)),
            ..Default::default()
        }
    }
}
