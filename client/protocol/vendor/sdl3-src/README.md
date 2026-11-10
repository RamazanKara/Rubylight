# sdl3-src with the Vulkan interop, PQ present and window target patches

The Linux desktop client builds SDL from source (sdl3-sys `build-from-source-static`). This
crate replaces the crates.io `sdl3-src` 3.4.20 there (`[patch.crates-io]` in
`protocol/Cargo.toml`) so that PyroWave GPU decoding and FFmpeg's Vulkan video decoding can
share the renderer's Vulkan device instead of reading every frame back to the CPU. SDL 3.4.20
has no way to share it: SDL_GPU
neither exposes its Vulkan handles nor takes external images, and upstream `main` has nothing
either (libsdl-org/SDL issue 10876; pull request 14157 adds no Vulkan path).

- `SDL` is the upstream repository at tag `release-3.4.20`, a git submodule: run
  `git submodule update --init --depth 1 client/protocol/vendor/sdl3-src/SDL` (in the nightjar
  repository `protocol/vendor/sdl3-src/SDL`) before building on Linux. Windows uses MSYS2's SDL3
  DLL and never builds this.
- `patches/vulkan-interop.patch` (Vulkan backend only) adds device properties with the
  `VkInstance`, `VkPhysicalDevice`, `VkDevice`, the queue, its family and the mutex SDL holds
  around it, `SDL_PROP_GPU_DEVICE_CREATE_VULKAN_VIDEO_DECODE_QUEUES_BOOLEAN`, which gives the
  device one extra queue in each video decode family for the app (SDL never uses them; a device
  property names their families), `SDL_PROP_GPU_DEVICE_CREATE_VULKAN_COMPUTE_QUEUE_BOOLEAN`,
  which gives it one compute queue for PyroWave's decodes (Granite's pick: a family with compute
  but not graphics first, else a second queue of a compute family; device properties name it,
  its family and index), and `SDL_PROP_GPU_TEXTURE_CREATE_VULKAN_IMAGE_NUMBER`, which
  wraps an image the app owns in an `SDL_GPUTexture` (never cycled, moved, transitioned or freed
  by SDL). The comments it adds to `SDL_gpu.h` are the contract.
- `patches/pq-present.patch` (GPU renderer only) lets the app hand the renderer a SPIR-V
  vertex and fragment shader (`SDL.renderer.create.gpu.pq_vertex_spirv`/`_size` and
  `pq_fragment_spirv`/`_size`) with `SRGB_LINEAR` output: the renderer then draws into an
  RGBA16F backbuffer as for scRGB, asks the swapchain for `HDR10_ST2084` and presents through
  that shader (the PQ encode in `desktop/src/pq_present.hlsl`) instead of the blit. Renderer
  creation fails if the swapchain refuses HDR10. SDL 3.4.20's GPU renderer has no HDR10 output
  of its own; it is for gamescope swapchains that offer HDR10 and not scRGB, and for HDR streams
  on desktop compositors that offer HDR10 (`handheld::choose`). The overlay and menus are drawn
  in linear light by SDL's own shaders, so this output keeps the backbuffer and its pass.
- `patches/window-hdr-metadata.patch` (Vulkan backend only) enables `VK_EXT_hdr_metadata` where
  the device has it and calls `vkSetHdrMetadataEXT` for an `HDR10_ST2084` swapchain with the
  `VkHdrMetadataEXT` the app keeps in the window property `SDL.window.gpu.hdr_metadata_pointer`,
  each time `SDL.window.gpu.hdr_metadata_serial` changes and on each new swapchain. SDL 3.4.20
  never sets HDR metadata.
- `patches/window-target.patch` (GPU renderer only) draws a frame straight into its swapchain
  image instead of the backbuffer, which SDL 3.4.20 blits to the swapchain at present. The image
  is acquired on the frame's first draw into the window's target (a clear before it is folded into
  that draw's pass; a frame that only clears takes it at present), so with FIFO the wait moves from
  the present to that draw. The backbuffer stays for the PQ present, for a frame drawn while the
  renderer property `SDL.renderer.gpu.read_back` is true (`SDL_RenderReadPixels` of the window's
  target, which the swapchain image cannot serve), and for a frame whose image differs from it in
  size or format (a resize). A read-back after the frame took its image fails instead of
  presenting it early. `SDL.renderer.gpu.window_target_frames` counts the frames drawn straight in.
- `patches/display-hdr-state.patch` keeps `SDL_PROP_DISPLAY_HDR_ENABLED_BOOLEAN` true to the
  display's HDR state. SDL 3.4.20 sets it once, when the display is added, and a Wayland
  compositor's answer about an output can come later. Drop it once upstream SDL updates it.
- `patches/renderer-sdr-white.patch` lets the app set the SDR white a renderer with `SRGB_LINEAR`
  output draws with, in the float window property `SDL.window.renderer.sdr_white_level` (above
  zero; read where the renderer reads the window's own level, at creation and on each HDR report).
  SDL 3.4.20 reports a Wayland window's SDR white as the compositor's reference luminance over
  80 nits and multiplies all it draws by that, which is right for the PQ present (1.0 is 80 nits
  there) and about 2.5 times too bright on Mesa's scRGB swapchain, where 1.0 already is the
  reference white. `handheld::renderer_sdr_white` says which output gets which. Check it again
  when upstream SDL changes what the Wayland backend reports (its `main` sets 1.0 there).
- `build.rs` copies the submodule into the build directory and applies the patches there; a hunk
  that does not match fails the build, and so does a patch that would change nothing (no
  `+++ b/` file, a file without hunks). Its tests: see the top of `build.rs`.

To move to another SDL release: check out the new tag in the submodule, bump the version here
and in `src/lib.rs` to the one sdl3-sys pins, and regenerate the patches against the new files
(`diff -u` of `include/SDL3/SDL_gpu.h` and `src/gpu/vulkan/SDL_gpu_vulkan.c`, of `src/render/gpu/SDL_render_gpu.c`, of `src/render/SDL_render.c`, and of `src/video/SDL_video.c`, paths `a/` and `b/`).
Drop the interop patch once upstream SDL can share the device, and the PQ patch once its GPU renderer outputs HDR10.
