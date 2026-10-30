## en
- Windows: with HDR on, Nightjar asks a Rubylight host for Dolby Vision (profile 8.1 on HEVC, 10.1 on AV1), and the HDR tone map follows each frame's Dolby Vision brightness metadata, ahead of HDR10+. Other hosts get the same request as before.
- Windows: HLG streams are drawn with the HLG transfer for the display's brightness instead of being read as PQ, on HDR displays and on SDR ones.
- Linux: HLG pictures from hardware decoders (VAAPI, NVDEC, V4L2) are drawn as HLG too, instead of as PQ.
