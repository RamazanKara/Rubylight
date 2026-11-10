## desktop
- Linux: PyroWave pictures are drawn straight from the decoder's images instead of being copied into separate textures first, which leaves the GPU one copy of every frame less to do.
- Linux: HDR PyroWave streams with nothing drawn over them go straight to the window instead of through an extra full-screen pass.
- Linux: with VA-API decoding on Vulkan, each decoder surface is imported once and kept for its later pictures instead of being exported and imported again for every frame.
- The log says which of these paths a stream takes.
