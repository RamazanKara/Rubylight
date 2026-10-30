# How Rubylight works

[Documentation](README.md) · [Configuration](configuration.md) · [Performance](performance.md) · [Developer guide](../rust/README.md)

Rubylight turns a Windows display into a Moonlight stream. Its Radeon path keeps captured pixels on the GPU, prepares them on D3D12 compute queues and passes them to AMD's native encoder. The host limits outstanding work so a busy encoder or network does not become a growing queue of old pictures. Rubylight 3.0 encodes on AMD GPUs only: AMF for H.264, HEVC and AV1, and PyroWave's own encoder. The NVENC, Quick Sync and software encoders were removed; NVIDIA and Intel graphics stay on [Rubylight 2.6.x](https://github.com/RamazanKara/Rubylight/releases/tag/v2.6.0). Physical displays, WGC, Desktop Duplication and the D3D11 fallback are all still supported.

This guide follows a frame from capture to delivery, then covers the process boundaries, colour paths and recovery that keep the stream running. [Performance](performance.md) separates measured host processing from the complete render-to-client journey.

## Written in Rust

The Windows host, Moonlight protocol implementation, native helpers, service and installer are written in Rust. The released executables do not link the old C++ host. The browser console uses Svelte; codec libraries, GPU SDKs and Windows drivers are external components.

| Component | Responsibility |
| --- | --- |
| [Core](../rust/core) | Pairing, encryption, RTSP/SDP, error correction, input parsing, permissions, configuration and durable state, and the encoder, pacing and display policies the host applies. Builds and is tested on any OS. |
| [Windows integration](../rust/windows) | Capture, colour conversion, native encoders, audio, input, displays, frame limiting and Windows service integration. |
| [Host](../rust/host) | HTTP/TLS, Moonlight endpoints, the console API, control and media transport, scheduling and session lifecycle. |
| [Setup](../rust/setup) | Installation, upgrades and package recovery. |
| [Web console](../rust/web) | Devices, games, settings, stream statistics, logs and maintenance. |
| [Vulkan layer](../rust/vulkan-layer) | Native HDR integration for Vulkan games. |
| [Probes](../rust/probes) | The diagnostic probes the package ships: encoder throughput, error correction and video packetization. |

The earlier C++ host was removed after 2.0.0-rc.23; [that tag](https://github.com/RamazanKara/Rubylight/tree/2.0.0-rc.23) keeps its sources. The [Rust build](building.md) uses the workspace at the repository root.

## The Radeon frame path

For standard H.264, HEVC and AV1 streams on supported AMD GPUs:

| Stage | What happens | Source |
| --- | --- | --- |
| **Capture** | On a virtual display Rubylight created, its display driver pushes each composed frame into textures shared with the host. Elsewhere Windows Graphics Capture (WGC) supplies a display texture; Desktop Duplication (DDX) is an explicit choice and fallback. | [Capture backends](../rust/windows/src/capture.rs), [frame push](../rust/windows/src/capture/idd_push.rs) |
| **Take ownership** | A GPU copy moves the captured image into a texture the host owns. The capture frame stays alive until the copy finishes. Consumers share the latest published frame. | [Frame publication](../rust/host/src/stream/capture.rs), [compute handoff](../rust/windows/src/compute.rs) |
| **Prepare** | D3D12 compute converts RGB into the encoder's YUV format, including resizing and HDR conversion where required. | [Colour conversion](../rust/windows/src/compute.rs) |
| **Encode** | Native AMD AMF consumes the D3D12 surface and produces an H.264, HEVC or AV1 bitstream. | [AMF GPU integration](../rust/windows/src/amf/gpu.rs) |
| **Deliver** | The host packetizes encoded bytes, adds forward error correction and applies negotiated encryption, then sends them to Moonlight. | [Video sender](../rust/host/src/video_send.rs), [packet formats](../rust/core/src/packet.rs) |

The raw picture stays in GPU textures through preparation and encoding; compressed output returns to the CPU for transport. Moonlight receives, decodes and presents it on the client. Those client stages and network transit are separate from [host-processing time](performance.md#what-the-numbers-mean).

Threaded delivery has separate packetizer and paced sender workers. The packetizer fixes Moonlight's host-processing field before constructing FEC and encrypted packets, then hands prepared frames to the sender. Conventional codecs keep frame order and use the combined encoder/sender admission limit; a keyframe can discard queued older frames while preserving frame numbers and encryption counters. PyroWave keeps the newest waiting intra frame at each handoff. Prepared-packet queue time has its own conventional timing stage and remains part of present-to-sent latency.

PyroWave's optional extra detail protection observes the packetizer's arrival cadence, so a slow sender does not make a full-rate source look like a static source producing frames slowly.

Before claiming another picture, the [stream loop](../rust/host/src/stream.rs) checks both the encoder and the conventional video sender. Native AMF learns its application backlog from completed pictures: a driver that drains each input can settle at one outstanding encode, while pre-analysis and drivers that retain inputs keep the depth they need. Startup permits four inputs without pre-analysis, or six with the configured one-frame lookahead; the eight-surface ownership cap and driver queue settings are separate. If the driver's pre-analysis requirements cannot be read, the host keeps a limit of two.

After AMF demonstrates depth one, timed admission permits conversion of a second input near predicted output. A bounded startup drain probe lets a busy source demonstrate that depth without starving a driver that needs two inputs. The combined encoder/sender allowance stays at least two, so sending a packet can overlap the next encode. Larger AMF startup or lookahead requirements expand that combined allowance. PyroWave keeps a two-picture budget; keyframes reserve additional sending capacity. Once work drains, the host takes the newest available picture. The [Radeon measurements and unresolved limits](../rust/PERFORMANCE_WORK.md#october-13-adaptive-amf-backlog-candidate) distinguish the saturated picture-age improvement from inconclusive normal-load tails.

Recovery frames have a separate admission rule on proven depth-one AMF pipelines with a dedicated sender. While an IDR is encoding, a bounded drain probe prevents an early following input from ageing behind it. During transmission, actual batch progress can admit one fresh input near the remaining paced-wire deadline, using the fastest observed preparation-plus-encode duration minus one query allowance. Unknown or deeper pipelines keep their required priming depth. Packet order, the combined queue bound, FEC, pacing and the host-processing metric boundary stay the same; socket stalls or drops restore the strict sender barrier.

The `idd_push_fence_ring` transport is the default for direct driver push. It keeps the D3D11 display driver and all existing capture methods. The host creates D3D12-owned shared textures; per-slot atomic ownership and shared written/released fences protect them until every GPU reader completes. `idd_push_fence_ring = false` selects the keyed-mutex ring. Old drivers, failed fence setup and captures without in-place compute conversion also use the keyed-mutex ring, with a console warning. A synchronization failure retires an unsafe compute queue and reopens capture on a fresh device. The transport rejection survives capture reopen for the same display registration, driver process and render adapter; a real display recreation allows another attempt. CPU readback bounds producer-fence completion before submitting its staging copy, while GPU readers retain asynchronous ordering. This is separate from native D3D12 swap-chain support in IddCx 1.11. See the [driver transport contract](../rust/drivers/rubylight-vdisplay/README.md).

![Scheduling schematic comparing the reviewed Sunshine-derived D3D11 path with Rubylight's D3D12 compute path](media/compute-comparison.svg)

The reviewed Sunshine-derived D3D11 path and Rubylight both use GPU textures and native AMF. Rubylight changes where frame preparation runs: compute work can overlap graphics work instead of joining the same graphics queue. The diagram illustrates scheduling; [measured timings](performance.md) come from separate fixtures. The reviewed baseline is Vibepollo 2.0.

Compute and graphics still share GPU resources. Copy/conversion efficiency helps the host's part of the frame journey; game rendering and desktop composition also determine when a fresh picture exists. [The saturation investigation](../rust/PERFORMANCE.md#saturation-diagnosis-and-queue-drain-rejection) records that distinction.

## Synchronization and ownership

A captured texture can arrive before its producer has finished writing it. Rubylight waits for producer readiness on the GPU, copies it into an owned texture and holds the captured frame until that copy is safe. Owning the copy lets capture move on while the encoder still uses the previous picture. Each converted output texture has its own fence, so AMF's completion signal cannot release a different frame early.

Texture pools and native encoder queues are bounded. Texture ownership and per-frame HDR metadata survive until native codec references release them. Capture textures that cannot be shared use the D3D11 fallback. The [compute implementation](../rust/windows/src/compute.rs) and [synchronization measurements](../rust/PERFORMANCE.md#october-4-capture-copies-and-conversion-beside-a-game) contain the details.

The capture snapshot pool holds at most eight textures. If consumers own them all, capture preserves a pending native update until space returns instead of consuming and losing it. WGC retains the newest pending frame, and Desktop Duplication waits before acquiring another frame. A blocked WGC publication uses normal polling rather than repeatedly waking on an expired deadline. This preserves the final update of a desktop that becomes static without growing a queue.

## Frames from Rubylight's display driver

Rubylight's virtual displays come from its own IddCx driver ([`rubylight-vdisplay`](../rust/drivers/rubylight-vdisplay)). For each monitor the driver drains the swap chain Windows composes into, which keeps the monitor live. When a stream captures that monitor, the host creates four shared textures and a one-page header and hands them to the driver ([the ring protocol](../rust/drivers/rubylight-vdisplay-proto/src/ring.rs)). By default shared fences guard the textures (the fence ring described above); older drivers and captures without in-place compute conversion use keyed mutexes. Per frame the driver copies the surface into a texture the host is not reading, records its present time and publishes it; with every texture held it drops the frame rather than wait, because Windows waits on the driver for the next frame. The host takes only the newest publish, so neither side builds a queue. On AMD the compute conversion reads the texture in place and hands it back when it is done.

The ring belongs to the monitor, not to the swap chain: when Windows replaces the swap chain (a GPU or colour-mode change), the new one continues the ring's frame sequence. The driver keeps the newest picture no ring holds and starts a new ring with it, so a reconnect to a still desktop, which Windows stops presenting, has a picture at once. A process registers one ring per monitor and clears only its own. A ring that fails before its first picture is retried once, then that display's captures use WGC.

## WGC through the Windows service

The service starts a hidden capture worker as the signed-in user, giving WGC access to the user's Windows capture broker. Three shared GPU textures transfer frames to the host; a local pipe carries bounded metadata and checks the participating process identities. The worker belongs to the capture session and closes with it.

WGC startup failures select Desktop Duplication. The implementation also falls back for lock/UAC desktops and retries WGC on return to the normal desktop. While Windows is locked, the stream's display is set up, kept and restored from the lock screen's desktop, so you can stream the sign-in screen and unlock the PC remotely.

WGC requests an explicit zero minimum update interval where Windows supports it. Guarded source-phase pacing waits briefly for a predicted fresh update when capture history is stable and faster than the stream target. Irregular or slower sources use ordinary pacing. [Capture settings](configuration.md) expose the diagnostic overrides.

## Native HDR and full-resolution colour

The native HDR path captures FP16 scRGB, resizes in linear light and converts to ten-bit BT.2020/PQ, or BT.2020/HLG graded for 1000 nits for a device that asks for HLG. Shader math preserves absolute ST.2084 luminance. Windows HDR detection distinguishes an active HDR output from wide-gamut SDR colour management. [Independent decoding and reference-pixel checks](performance.md#hdr-and-pyrowave-validation) verify the tested pixel path.

Capture and HDR metadata share a cached description of the selected display. The cache checks the DXGI factory at most once per second and resolves the same output from a fresh factory when Windows invalidates it. Matching uses the display's owning adapter and monitor identity, independently of the encoder's GPU. An SDR/HDR colour-space change uses capture recovery to reopen the frame pool; a luminance-only change updates metadata without a capture restart. A missing output is never replaced by another monitor's description.

HDR mastering and content-light metadata use each codec's bitstream units. Encoded metadata changes request a keyframe, retained until the encoder accepts input. AMF tracks accepted properties and retries transient metadata failures with bounded backoff. After initialization, an observed AMF output-range mismatch fails encoder creation; drivers with unavailable range properties retain the documented limited-range default.

**PyroWave provides full 10-bit HDR 4:4:4** through a separate encoder and shared D3D11/Vulkan path. D3D12 compute prepares its three planar textures; a shared fence orders conversion and Vulkan access, and the encoded bitstream returns to the CPU. If compute conversion is unavailable, PyroWave uses graphics-queue conversion. The [PyroWave integration](../rust/windows/src/pyrowave.rs) owns this handoff independently of AMF.

Full-resolution chroma gives each pixel its own colour samples. Use [Nightjar](https://github.com/RamazanKara/nightjar), the Android and desktop client with PyroWave support, or on a PC [Nonary's compatible Moonlight client](https://github.com/Nonary/moonlight-qt), over a fast local network; [client selection](getting-started.md#choose-your-stream-format) explains the options.

## Session, audio and input

Pairing, app discovery and launch requests enter through the [Moonlight HTTP endpoints](../rust/host/src/nvhttp.rs). An authorized launch lets the [RTSP server](../rust/host/src/rtsp_server.rs) negotiate stream settings; the [stream host](../rust/host/src/stream.rs) then manages display preparation, capture, encoding and media workers. The browser console has a separate [administration API](api.md). Paired clients that want more than Moonlight offers use the [native API](native-api.md) on the same HTTPS port: library, session state, and, with the device's host control permission, session actions and Radeon settings.

The host keeps an explicit session state (idle, preparing, launching, streaming, suspended, stopping) that records what the stock Moonlight flows already do. Disconnect, End session and Close game are transitions between those states. Radeon settings a client writes are journaled on disk and restored when the session ends, so a crash cannot leave them changed. [Session actions](configuration.md#session-actions) and [Radeon settings](configuration.md#radeon-settings-with-restore) describe the behaviour.

Audio uses [WASAPI loopback capture and Opus encoding](../rust/windows/src/audio.rs) on its own worker. Client input follows the control path back to Windows. [Virtual controllers have a separate worker](../rust/windows/src/input/gamepad_thread.rs), so a blocking controller-driver call does not hold up keyboard, mouse or touch input. [Control messages](control-messages.md) and [microphone transport](microphone.md) describe Rubylight's client extensions.

## Stream extensions

Each of these sits beside the paths above; none replaces one.

- **Dolby Vision, HDR10+ and HLG.** While the compute conversion writes a frame, a GPU pass measures its brightness ([`compute_hdr_stats.rs`](../rust/windows/src/compute_hdr_stats.rs)), and the host turns the previous frame's statistics into HDR10+ and HDR Vivid metadata ([`hdr_dynamic`](../rust/core/src/hdr_dynamic.rs)) or a Dolby Vision RPU ([`dovi`](../rust/core/src/dovi.rs)), placed in the encoded frame: a NAL unit for HEVC, a metadata OBU for AV1. HLG changes only the transfer curve of the same conversion. A device that did not ask for a format gets the plain HDR10 or HLG stream.
- **Scene cuts and recovery inside error correction.** The automatic frame cap keeps every frame within what Moonlight's four FEC blocks can fully protect ([`network_pacing.rs`](../rust/core/src/network_pacing.rs)). AMF never encodes a frame twice on its own, so an inter frame that still comes out past that bound, a scene cut, goes back to AMF once as a keyframe from the same converted input, and only the keyframe is sent. A device that chooses its own error correction ([`adaptive_fec.rs`](../rust/core/src/adaptive_fec.rs)) moves the bound with its share.
- **Long-term reference recovery.** On H.264 and HEVC the host rewrites AMF's slice headers for a device that skipped lost frames, so a repair frame built on an older reference decodes on stock decoders ([`ltr.rs`](../rust/core/src/ltr.rs), [`bitstream`](../rust/core/src/bitstream.rs)); it sends a keyframe instead when that would be smaller.
- **Roaming.** A device whose address changes proves the new one with the stream key before the host moves control, video and audio there ([`roaming`](../rust/core/src/roaming.rs), [host side](../rust/host/src/stream/roaming.rs)); the new path's pacing and bitrate are learned from scratch.
- **Nucleus Co-op.** Background streams and co-op run each player's copy of the game as a Nucleus Co-op instance ([`nucleus`](../rust/core/src/nucleus.rs), [host](../rust/host/src/nucleus.rs)). The host captures that instance's window alone with WGC, plays only that instance's sound, and routes each device's controllers to its own instance; keyboard, mouse and clipboard stay with the PC user.
- **Clipboard, files and folders.** Clipboard sync ([`clipboard_sync.rs`](../rust/host/src/stream/clipboard_sync.rs)), file transfer and shared folders ([`files.rs`](../rust/windows/src/files.rs), [`shared_folder.rs`](../rust/windows/src/shared_folder.rs)) run on the streaming HTTPS port and the control channel, each behind its own per-device permission. Files are read and written as the signed-in Windows user.
- **USB forwarding.** The device serves a USB device over USB/IP inside a TLS tunnel on the stream's HTTPS port; the host relays the tunnel to usbip-win2 on 127.0.0.1 ([`usb_forward`](../rust/host/src/usb_forward.rs)), so no USB/IP port is open to the network.

## Recovery and updates

An encoder that returns no frame is recreated with the same encoder settings after 250 ms, then 500 ms, 1 s and 2 s; the session ends after 20 s without output. After a second encoder failure in one session, colour conversion moves to the graphics queue for the rest of it. A session that hears nothing from its client for the ping timeout (10 s by default) ends as a disconnect does.

A GPU device loss triggers a broader rebuild: capture consumers first release their encoders, images and filters, then capture and encoding restart together. This has a separate [30-second recovery deadline](../rust/host/src/stream/device_recovery.rs); repeated failures do not extend it. If the GPU stays unavailable, the stream ends while the host console remains available.

Display recovery journals host-owned changes. Disconnecting restores displays after three seconds by default, and exclusive layouts always release after the configured restore delay. An arrangement that disabled an original display restores the original layout even after a competing Win+P change; other arrangements preserve later user changes. Failed arrangement restores remain journaled, and a new stream adopts that original baseline. Virtual-display startup uses a short guard for unexpectedly reactivated displays, ending after startup or an observed competing layout change.

Updates notify first. The installed service verifies the official installer, waits for an idle host (or, on **Install now**, ends the stream) and upgrades with package recovery for copying or startup failures. See [configuration](configuration.md) for the update controls and [getting started](getting-started.md) for installation and migration.
