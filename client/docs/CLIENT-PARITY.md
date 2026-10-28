# Android and desktop

[Documentation](index.md) · [Host feature support](PARITY-MATRIX.md)

What the desktop client (Windows and Linux, [protocol/desktop](../protocol/desktop/README.md)) does compared with Nightjar Android. **Same** means the same behaviour, usually through the shared `nightjar-policy` crate; **Partial** says what is missing; **Missing** is not there yet. Host support is the same for both clients: see [host feature support](PARITY-MATRIX.md).

| Feature | Desktop | Notes |
| --- | --- | --- |
| Discovery, manual addresses, Wake-on-LAN | Same | |
| PIN and one-time-PIN pairing | Same | |
| Library, artwork, Home and the game page | Same | |
| Launch, resume, quit, quit and start another | Same | |
| Quick resume | Same | Off by default (Settings › App). |
| Quit on leave per game | Same | On the game page. Leaving shows "Quitting … on the PC…" like Android; a failed quit shows a message. |
| Remote Monitor and Input-only tiles | Same | |
| Input-only on any PC | Same | Off by default (Settings › App); a 640x360, 30 fps, 500 kbps stream that is neither shown nor played. |
| Settings per PC and per game, profiles by network | Same | |
| H.264, HEVC, AV1 hardware decoding | Same | D3D11VA on Windows, VAAPI on Linux. Linux copies pictures through the CPU (no dmabuf import yet). |
| HDR10 | Partial | Windows. Linux is Experimental and not checked on an HDR display yet. |
| YUV 4:4:4 | Same | Experimental, off by default (Settings › Video): H.264, HEVC and AV1 4:4:4 where the decoder check passes and the PC encodes it; otherwise 4:2:0 with the reason in a toast. Software decoding draws it; hardware decoders that only give packed 4:4:4 surfaces are refused by name. Linux needs a restart after turning it on. |
| 10-bit SDR | Same | Experimental, off by default (Settings › Video): HEVC or AV1 Main10 without HDR, from the hardware or the software decoder; 4:4:4 takes priority when the PC takes both. A refusal or an 8-bit answer is told in a toast. Linux needs a restart after turning it on. |
| PyroWave | Partial | Decoding, bandwidth test and pace calibration. No PyroWave automatic bitrate. |
| Automatic bitrate, automatic resolution and frame rate | Same | |
| Frame pacing modes (Balanced, Smoothest, drop late) | Missing | V-Sync and VRR host-time pacing only. |
| Phase-lock frame timing | Partial | Windows; Linux on X11 with OpenGL only. |
| Performance overlay | Same | Compact, Statistics and Debug in any corner, with the host's CPU and GPU readings. |
| Stream health report and bitrate trial | Same | |
| Connection test | Partial | No network path and remote access check, and no delay test. |
| Reconnect and network drop tolerance | Same | |
| Audio, microphone | Same | Opus. |
| Keyboard, mouse, touch and pen | Same | |
| Touch trackpad mode and on-screen controls | Missing | |
| Controllers | Partial | Rumble, motion, touchpad, LEDs, battery and DualSense features are wired but not tried with physical controllers yet. |
| Clipboard, host commands, cursor shapes | Same | Each only when the host reports it. |
| Saved shortcuts | Same | Same import and export format. |
| Starting from a game frontend | Same | `nightjar --host … [--app …]`. |
| Host family and version in the PC's details, host web UI button | Missing | |
| Problem reports and crash records | Missing | The logs folder is there. |
| Screen reader, settings search, presets, PIN lock | Missing | |
| Languages | Partial | English and German. |
| Updater | Missing | Windows. |
