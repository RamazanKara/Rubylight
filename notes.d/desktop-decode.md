## desktop
- A GPU that cannot decode HEVC Main10 or AV1 is no longer offered them: Nightjar checks what the hardware decoder decodes before the stream starts, and the PC streams a format it does decode instead of one Nightjar would decode on the CPU, or for AV1 not at all.
- Should the hardware decoder still give up on a stream, Nightjar says so, names it on the overlay and goes on with the next decoder, instead of decoding on the CPU under the hardware decoder's name.
- A frame the decoder could not turn into a picture, or decoded damaged, now asks the PC for a fresh picture, so artifacts no longer stay on screen until the next scene change.
- Lost frames recover faster with HEVC and AV1 on D3D11VA, VA-API and software decoding: Nightjar asks the PC to stop referencing the lost frame instead of waiting for a full key frame. Software decoding asks for frames in slices it decodes in parallel.
- Linux: on the GPU renderer, Vulkan video now decodes before VA-API, so HDR and 10-bit pictures (Steam Deck included) stay on the GPU instead of a round trip through memory.
- Linux: the V4L2 decoders of ARM handhelds and boards hand each picture over with its own frame instead of one frame late, and the overlay's latency counts that wait.
