# sdl3-src with the Vulkan interop patch

The Linux desktop client builds SDL from source (sdl3-sys `build-from-source-static`). This
crate replaces the crates.io `sdl3-src` 3.4.18 there (`[patch.crates-io]` in
`protocol/Cargo.toml`) so that PyroWave GPU decoding can share the renderer's Vulkan device
instead of reading every frame back to the CPU. SDL 3.4.18 has no way to share it: SDL_GPU
neither exposes its Vulkan handles nor takes external images, and upstream `main` has nothing
either (libsdl-org/SDL issue 10876; pull request 14157 adds no Vulkan path).

- `SDL` is the upstream repository at tag `release-3.4.18`, a git submodule: run
  `git submodule update --init --depth 1 client/protocol/vendor/sdl3-src/SDL` (in the nightjar
  repository `protocol/vendor/sdl3-src/SDL`) before building on Linux. Windows uses MSYS2's SDL3
  DLL and never builds this.
- `patches/vulkan-interop.patch` (Vulkan backend only) adds device properties with the
  `VkInstance`, `VkPhysicalDevice`, `VkDevice`, the queue, its family and the mutex SDL holds
  around it, and `SDL_PROP_GPU_TEXTURE_CREATE_VULKAN_IMAGE_NUMBER`, which wraps an image the app
  owns in an `SDL_GPUTexture` (never cycled, moved, transitioned or freed by SDL). The comments it
  adds to `SDL_gpu.h` are the contract.
- `build.rs` copies the submodule into the build directory and applies the patch there; a hunk
  that does not match fails the build, and so does a patch that would change nothing (no
  `+++ b/` file, a file without hunks). Its tests: see the top of `build.rs`.

To move to another SDL release: check out the new tag in the submodule, bump the version here
and in `src/lib.rs` to the one sdl3-sys pins, and regenerate the patch against the new files
(`diff -u` of `include/SDL3/SDL_gpu.h` and `src/gpu/vulkan/SDL_gpu_vulkan.c`, paths `a/` and `b/`).
Drop the patch once upstream SDL can share the device.
