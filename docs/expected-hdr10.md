# HDR10 on an HDR phone (Expected, untested on hardware)

**Status: Expected (untested on hardware).** Nightjar's own devices are a Retroid Pocket 6 (SDR panel), an emulator and a Linux NUC, so no HDR10 panel has been in front of this code. The decisions below are covered by unit tests with made-up display capabilities; the picture itself has not been looked at on an HDR screen.

## What the app does

1. **Ask for HDR only when it can show.** HDR is requested when the HDR setting is on, the Android version is 7 or newer, the screen reports **HDR10** (HLG or Dolby Vision alone do not count: the PC encodes HDR10 PQ). On Android 14 and newer the screen's capabilities follow the mode it is in, so a mode without HDR10 is refused; if any other mode of the screen has HDR10 the refusal says it is the mode, and HDR is still not requested. The decoder must also offer HEVC Main10 HDR10, AV1 Main10 or PyroWave. The same test (`HdrPolicy`) decides the request, the settings toggle, the packed 4:4:4 path and the HDR flag sent to the host in the display report. Before this change the report also accepted HLG.
2. **Configure the decoder the way the platform describes HDR10.** The output is BT.2020 with the ST 2084 (PQ) transfer. The host's mastering data is passed as `KEY_HDR_STATIC_INFO`: a type byte of 0 followed by the first 24 bytes of the host's report (CTA-861.3 layout; the host's 26-byte form adds a full-frame luminance that Android does not accept). A change of metadata restarts the codec once.
3. **Say it when HDR is not what the user asked for.** Besides the once-per-PC message, the Statistics and debug overlays carry a line for as long as the condition lasts, and the log gets one `HDR requested but ...` line:

| Shown as | Meaning |
| --- | --- |
| This screen can't show HDR | No HDR10 in the screen's capabilities (or none reported at all, as on some old devices). |
| This screen mode can't show HDR | Android 14+: the current mode has no HDR10, but another mode of the screen lists it. |
| HDR needs Android 7 / This device can't play HDR video | The Android version or the decoders rule it out. |
| The PC sent no HDR video | HDR was asked for and the negotiated format is 8-bit. |
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

- Plan, B3: "HDR10 end to end on an HDR phone: Expected (untested on hardware). Nightjar asks for HDR only when the screen reports HDR10 in its current mode and names the reason on the overlay when HDR is asked for and not delivered."
- Hardware matrix: "Nightjar Android, HDR on an HDR phone | Expected | None | Decisions and reasons are unit-tested with fake capabilities; no HDR panel has been used."
- Release notes: "Expected, not tested on hardware: HDR10 is requested only when the current display mode has it, and the Statistics overlay says why HDR is off or not working."
