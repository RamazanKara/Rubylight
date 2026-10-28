## en
- Dolby Vision on MediaTek TVs such as TCL's: the TV now shows the stream as Dolby Vision. Frames reach the decoder without the stream's trailing padding, which these TVs rejected as broken Dolby Vision metadata, falling back to HDR10.
- Dolby Vision with the automatic codec from a PC that offers only Dolby Vision profile 8: the stream now uses HEVC instead of AV1, since profile 8 is HEVC only. Before, it streamed HDR10 over AV1.
- HDR on MediaTek TVs: HEVC streams with HDR10+ now show as HDR10+ instead of SDR. Frames reach the decoder without the HDR Vivid metadata, which these TVs took over HDR10+ and then showed as SDR.
- HDR streams no longer freeze on MediaTek TVs shortly after they start. The PC's HDR metadata now reaches the running decoder instead of restarting it, a restart after which these decoders took no more input.

## de
- Dolby Vision auf MediaTek-Fernsehern wie denen von TCL: Der Fernseher zeigt den Stream jetzt in Dolby Vision. Frames erreichen den Decoder ohne das Auffüllen am Ende des Streams, das diese Fernseher als fehlerhafte Dolby-Vision-Metadaten ablehnten und auf HDR10 zurückfielen.
- Dolby Vision mit automatischem Codec von einem PC, der nur Dolby Vision Profil 8 anbietet: Der Stream nutzt jetzt HEVC statt AV1, da Profil 8 nur HEVC kennt. Bisher lief er als HDR10 über AV1.
- HDR auf MediaTek-Fernsehern: HEVC-Streams mit HDR10+ erscheinen jetzt als HDR10+ statt SDR. Frames erreichen den Decoder ohne die HDR-Vivid-Metadaten, die diese Fernseher HDR10+ vorzogen und dann in SDR zeigten.
- HDR-Streams frieren auf MediaTek-Fernsehern nicht mehr kurz nach dem Start ein. Die HDR-Metadaten des PCs erreichen jetzt den laufenden Decoder, statt ihn neu zu starten; nach diesem Neustart nahmen diese Decoder keine Eingabe mehr an.
