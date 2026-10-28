## en
- Dolby Vision on MediaTek TVs such as TCL's: the TV now shows the stream as Dolby Vision. Frames reach the decoder without the stream's trailing padding, which these TVs rejected as broken Dolby Vision metadata, falling back to HDR10.
- Dolby Vision with the automatic codec from a PC that offers only Dolby Vision profile 8: the stream now uses HEVC instead of AV1, since profile 8 is HEVC only. Before, it streamed HDR10 over AV1.
- HDR on MediaTek TVs: HEVC streams with HDR10+ now show as HDR10+ instead of SDR. Frames reach the decoder without the HDR Vivid metadata, which these TVs took over HDR10+ and then showed as SDR.
- HDR streams no longer freeze on MediaTek TVs shortly after they start. The PC's HDR metadata now reaches the running decoder instead of restarting it, a restart after which these decoders took no more input.
- The same now holds on every device from Android 10: the PC's HDR metadata reaches the running decoder instead of restarting it. A decoder whose output still lacks it 60 frames later restarts with it, as before.
