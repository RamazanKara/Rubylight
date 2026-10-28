# HDR10 on an HDR phone (Experimental)

**Status: Experimental.** Nightjar's own devices are a Retroid Pocket 6 (SDR panel), an emulator and a Linux NUC, so no HDR10 panel has been in front of this code. The decisions below are covered by unit tests with made-up display capabilities; the picture itself has not been looked at on an HDR screen.

## What the app does

1. **Ask for HDR only when it can show.** HDR is requested when the HDR setting is on, the Android version is 7 or newer, the screen reports **HDR10**, or **HLG** while the PC offers HLG (Rubylight 3.0 hosts list it in `HdrFormats`; every other PC encodes HDR10 PQ only, so HLG or Dolby Vision alone do not count there) and the decoder or "Render through Vulkan" puts the picture on the screen. The Vulkan renderer passes HDR10 and HLG through: in the swapchain's HDR colour space where the Android loader lists it (Android 15 and newer), else with the window dataspace set to BT.2020 PQ or HLG (Android 10 to 14). Only where its Vulkan instance lacks `VK_EXT_swapchain_colorspace` is its surface PQ only, and an HLG-only screen is then refused with its own reason. Where it draws HLG, it asks for HLG only if this display's window takes the BT.2020 HLG dataspace (the probe below, which the loader's HLG colour space needs as well), else the stream is HDR10 and the status line says the screen shows HDR only as HLG. It never gets Dolby Vision. PyroWave passes HLG through the same two routes when the PC offers HLG on PyroWave (the `HdrFormats` offer flag 128, Rubylight 3.0 hosts with that update) and this display's window takes the BT.2020 HLG dataspace, else it draws PQ only; it never gets Dolby Vision either. Whether the window takes it is asked on each device, once per display and session, before HLG is asked of the PC (`WindowDataspace.probeHlg`): `ANativeWindow_setBuffersDataSpace` first, then, where that refuses it, the window's own `perform` call with `NATIVE_WINDOW_SET_BUFFERS_DATASPACE`, which the renderers then use too; the window's dataspace is put back afterwards, and the log names the call that answered. On our Android 13 Retroid Pocket 6 the public call refuses HLG (-22) and `perform` takes it: SurfaceFlinger shows the stream's layer as BT2020_HLG and the stream runs. If the window took HLG but no longer holds it after the first frame, or PyroWave stops producing frames, the status line (or the message at the end of the stream) says so. The hidden `debug_hdr_hlg_only` preference (debug builds only, no UI) makes an HDR10 screen count as HLG-only, for HLG tests. On Android 14 and newer the screen's capabilities follow the mode it is in, so a mode without HDR10 is refused; if any other mode of the screen has HDR10 the refusal says it is the mode, and HDR is still not requested. The decoder must also offer HEVC Main10 HDR10, AV1 Main10 or PyroWave. The same test (`HdrPolicy`) decides the request, the settings toggle and the packed 4:4:4 path. The HDR flag sent to the host in the display report is set for HDR10, and for an HLG-only screen only while the stream is HLG, never for a PQ stream it cannot show.
2. **Configure the decoder the way the platform describes HDR10.** The output is BT.2020 with the ST 2084 (PQ) transfer, or the HLG transfer for an HLG or Dolby Vision 8.4 stream. The host's mastering data is passed as `KEY_HDR_STATIC_INFO`: a type byte of 0 followed by the first 24 bytes of the host's report (CTA-861.3 layout; the host's 26-byte form adds a full-frame luminance that Android does not accept). A change of metadata restarts the codec once. MediaTek's Codec2 decoders (`c2.mtk.*`) get new metadata on a running HDR stream with `setParameters` instead, from Android 10, and restart only if their output format still lacks it 60 frames later; with their vendor low-latency mode, every restart releases the codec and creates a new one, because `stop()` and `configure()` left them without input buffers.
3. **Say it when HDR is not what the user asked for.** Besides the once-per-PC message, the Statistics and debug overlays carry a line for as long as the condition lasts, and the log gets one `HDR requested but ...` line:

| Shown as | Meaning |
| --- | --- |
| This screen can't show HDR | No HDR10 in the screen's capabilities (or none reported at all, as on some old devices). |
| This screen shows HDR only as HLG, which "Render through Vulkan" can't draw | The screen has HLG and no HDR10, the PC offers HLG, and the Vulkan renderer would present the stream on a PQ-only surface (no `VK_EXT_swapchain_colorspace`); the stream is SDR. |
| HDR10 (or HLG) shown as SDR: the display refused HDR from "Render through Vulkan" | The PC sends HDR, but the window refused the HDR dataspace or the surface has no ten-bit format, so the Vulkan renderer tone maps it to SDR. The renderer line in the overlay says "(tone mapped)". |
| Dolby Vision needs "Render through Vulkan" off | Once per PC: the screen and a decoder have it, and the Vulkan renderer cannot carry it; the stream is HDR10. |
| HDR10+ is not passed on with "Render through Vulkan" on this Android version | The screen has HDR10+, and the Vulkan renderer hands each frame's HDR10+ metadata to its window through Android's internal call for it (the one its video decoders use), but this Android does not have that call or refused it. Said once per stream on the status line and once per PC as a message; the picture stays HDR10 with its static metadata. The overlay says "HDR10+" while it reaches the screen. |
| This screen mode can't show HDR | Android 14+: the current mode has no HDR10, but another mode of the screen lists it. |
| HDR needs Android 7 / This device can't play HDR video | The Android version or the decoders rule it out. |
| The PC sent no HDR video | HDR was asked for and the negotiated format is 8-bit. |
| This screen shows HDR only as HLG, but the PC sends HDR10 | HDR was asked for because the screen shows HLG, but the stream is PQ: packed 4:4:4 draws PQ only, PyroWave draws PQ only from a PC that offers no HLG on PyroWave (the text then says that updating the PC’s host adds it) or on a display whose window refused HLG (the text then says so), or the PC left HLG out for this device or codec. |
| This screen took HLG from PyroWave before the stream, then didn’t show the stream as HLG | The window took the HLG dataspace before the stream, but no longer holds it after the first frame, or PyroWave stopped producing frames (then it is the message at the end of the stream). |
| The PC reports SDR | The host sent "HDR off", usually because Windows HDR is off for the streamed display. |
| The PC sent no mastering data | HDR is on, but the metadata is missing, shorter than 24 bytes, or has no primaries or a maximum luminance of 0. |
| The decoder reports a non-HDR output | The decoder's output transfer is present and is not PQ, so the compositor would treat the picture as SDR. Not judged on the PyroWave and packed paths, whose output is not the codec's. |

The overlay's colour word (HDR, 10-bit, SDR) was already there.

## Researched behaviour

- Android 7 defined HDR playback for tunneled decoders; HDR on a `SurfaceView` with opaque buffers is optional and not standardised, and "only tunneled decoders are guaranteed to play back HDR content". There is no standard API to check non-tunneled HDR playback, which is why a decoder check by profile (Main10 HDR10) and a check of the output format are used. [HDR video playback](https://source.android.com/docs/core/display/hdr)
- `Display.getHdrCapabilities()` reflects the EDID static metadata block (CTA-861.3); HDR10 maps to the ST 2084 EOTF. From Android 9, `MediaCodec` reports HDR10 static metadata in the output format as `KEY_HDR_STATIC_INFO` (HDR10+ dynamic metadata is `KEY_HDR10_PLUS_INFO`). Same page.
- Non-tunneled content shown on a panel that cannot display it may be "flattened into an SDR color volume". Nightjar therefore never asks for HDR on a screen without HDR10. Same page.
- A video `SurfaceView` does not need `Window.setColorMode(COLOR_MODE_HDR)`; that is for HDR UI content drawn by the app. Nightjar does not call it. (From the platform documentation as remembered; not re-checked in this pass.)
- Per-mode HDR types, `Display.Mode.getSupportedHdrTypes()`, exist from Android 14 (API 34). The keys `KEY_HDR_STATIC_INFO`, `KEY_COLOR_STANDARD` and `KEY_COLOR_TRANSFER` exist from Android 7 (API 24). (From the API reference as remembered: the reference pages could not be fetched as text in this pass.)
- Samsung HDR10+ phones: if the host sends only static metadata, a decoder that exposes the `HDR10Plus` profile may be picked. Nightjar's decoder choice already prefers a Main10 HDR10 profile and falls back to `HDR10Plus`; whether the phone then applies its own dynamic tone mapping is unverified.
- Sources on the host side: Sunshine's HDR metadata message is 26 bytes (12 for the primaries, 4 for the white point, then maximum and minimum luminance, MaxCLL, MaxFALL and the full-frame luminance). This is from reading the existing Nightjar code and protocol notes, not from the Sunshine source in this pass.

## What a tester should look for

On a phone whose settings show an HDR10 panel, with HDR on in Nightjar and Windows HDR on for the streamed display:

1. The overlay's colour word says HDR and no HDR line appears.
2. A bright HDR scene is brighter than the same scene in SDR, and not washed out.
3. Turning Windows HDR off mid-stream shows "The PC reports SDR" and the picture returns to SDR without a restart.
4. On a phone with several refresh rates, change the refresh setting and check HDR is still on or, if it is refused, that the overlay says the mode is the problem.

Report the phone, Android version, the codec in use and the `HDR requested but` log line if any.

## Rubylight wording

- Plan, B3: "HDR10 end to end on an HDR phone: Experimental. Nightjar asks for HDR only when the screen reports HDR10 in its current mode and names the reason on the overlay when HDR is asked for and not delivered."
- Hardware matrix: "Nightjar Android, HDR on an HDR phone | Experimental | None | Decisions and reasons are unit-tested with fake capabilities; no HDR panel has been used."
- Release notes: "Experimental: HDR10 is requested only when the current display mode has it, and the Statistics overlay says why HDR is off or not working."
