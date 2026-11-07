## desktop
- Linux: turning on HDR, upscaling, PyroWave, 4:4:4, 10-bit SDR or full chroma after Nightjar started without them no longer needs a restart: the stream's start switches from OpenGL to the GPU renderer on its own, and the launcher looks the same after the switch. If the GPU renderer cannot be made, the dialog says why.
- Linux: a new Renderer setting under Debug: Automatic (OpenGL until a stream needs the GPU renderer), OpenGL, or GPU (Vulkan). A change applies at once, never during a stream. On OpenGL, a stream that needs the GPU renderer is refused with the setting named.
- Linux: an HDR stream refused on a display with HDR on now names what the renderer could not do, instead of asking you to move the window to a display it is already on.
- Linux: the log says when the GPU renderer could not be made and Nightjar drew with another one.
