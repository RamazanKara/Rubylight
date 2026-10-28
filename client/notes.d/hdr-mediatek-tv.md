## en
- Dolby Vision: frames reach the decoder without the stream's trailing padding, which MediaTek TVs such as TCL's rejected as broken Dolby Vision metadata.
- HDR on MediaTek TVs: HEVC frames reach the decoder without the HDR Vivid metadata, which made these TVs show the picture as SDR. HDR10 and HDR10+ metadata stay.
- Dolby Vision with the automatic codec: a stream that asks for Dolby Vision now uses HEVC instead of AV1, since Dolby Vision profile 8 is HEVC only. Before, it streamed HDR10 over AV1.

## de
- Dolby Vision: Frames erreichen den Decoder ohne das Auffüllen am Ende des Streams, das MediaTek-Fernseher wie die von TCL als fehlerhafte Dolby-Vision-Metadaten ablehnten.
- HDR auf MediaTek-Fernsehern: HEVC-Frames erreichen den Decoder ohne die HDR-Vivid-Metadaten, wegen derer diese Fernseher das Bild in SDR zeigten. HDR10- und HDR10+-Metadaten bleiben.
- Dolby Vision mit automatischem Codec: Ein Stream, der Dolby Vision anfordert, nutzt jetzt HEVC statt AV1, da Dolby Vision Profil 8 nur HEVC kennt. Bisher lief er als HDR10 über AV1.
