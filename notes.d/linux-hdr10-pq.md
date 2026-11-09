## desktop
- Linux: HDR streams on a desktop whose compositor offers HDR10 present as HDR10 PQ (BT.2020) instead of scRGB, like moonlight-qt. The launcher and SDR streams keep their previous output. Where the HDR10 output cannot be made, Nightjar falls back to scRGB for the rest of the run and says so in the log.
- Linux: the HDR10 output passes the host's mastering metadata to the compositor. Metadata whose peak luminance is not above its minimum (such as all zeros) is not passed on.
