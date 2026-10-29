package com.limelight.nvstream.av.video;

public abstract class VideoDecoderRenderer {
    // The protocol setup callback carries whole FPS. Preserve a fractional launch rate separately
    // when the freshly discovered host supports it; renderers without pacing may ignore it.
    public void setStreamFrameRateMilliHz(int frameRateMilliHz) {}

    /** The PC's codec mode support (serverinfo's ServerCodecModeSupport), set before prepareVideoFormats. */
    public void setHostCodecModeSupport(int serverCodecModeSupport) {}

    public int prepareVideoFormats(int formats, int width, int height, int redrawRate) {
        return formats;
    }

    /** String resource saying why prepareVideoFormats could not set up PyroWave HDR, or 0. */
    public int pyroWaveHdrFailureReason() { return 0; }

    /** Why prepareVideoFormats left HDR out of packed full chroma, or null when it did not. */
    public String packedHdrDropReason() { return null; }

    public int preparePacked444Modes(int formats, int width, int height, int fps) { return 0; }

    /**
     * The packed full-chroma modes the PC offers (serverinfo, else its last DESCRIBE, later this
     * stream's DESCRIBE), or -1 when unknown. Set before prepareVideoFormats and again before setup.
     */
    public void setHostPacked444Modes(int modes) {}

    /**
     * The one HdrFormatPolicy bit an HDR stream asks for among the PC's HdrFormats (0 when it
     * lists none), or 0 for stock HDR10. Called after prepareVideoFormats with the negotiated
     * video formats.
     */
    public int pickHdrFormat(int hostHdrFormats, int videoFormats, boolean packed444) { return 0; }

    /** The video formats to negotiate once pickHdrFormat chose {@code hdrFormat}: the same unless it needs another codec. */
    public int videoFormatsForHdrFormat(int hdrFormat, int videoFormats) { return videoFormats; }

    /** What this stream's ANNOUNCE asked for (0 for stock HDR10), set right before setup. */
    public void setAnnouncedHdrFormat(int format) {}

    public int setChromaPacking(int version, int width, int height, int colorSpace, int colorRange) {
        return version == 0 ? 0 : -1;
    }

    public abstract int setup(int format, int width, int height, int redrawRate);

    public abstract void start();

    public abstract void stop();

    // This is called once for each frame-start NALU. This means it will be called several times
    // for an IDR frame which contains several parameter sets and the I-frame data.
    public abstract int submitDecodeUnit(byte[] decodeUnitData, int decodeUnitLength, int decodeUnitType,
                                         int frameNumber, int frameType, char frameHostProcessingLatency,
                                         long receiveTimeUs, long enqueueTimeUs, long receiveTimeNs,
                                         long presentationTimeUs);
    
    public abstract void cleanup();

    public abstract int getCapabilities();

    // The capabilities once the video format is negotiated: a renderer whose decoder depends on the
    // format reports those of the decoder that will run it.
    public int getCapabilities(int negotiatedFormat) {
        return getCapabilities();
    }

    public abstract void setHdrMode(boolean enabled, byte[] hdrMetadata);

    // A host timing report (control message 0x5536). Called on the native control thread, only
    // for hosts that announced the message and were asked for it.
    public void onHostTiming(HostTimingReport report) {}
}
