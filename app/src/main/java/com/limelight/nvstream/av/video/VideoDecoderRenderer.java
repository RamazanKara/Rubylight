package com.limelight.nvstream.av.video;

public abstract class VideoDecoderRenderer {
    // The protocol setup callback carries whole FPS. Preserve a fractional launch rate separately
    // when the freshly discovered host supports it; renderers without pacing may ignore it.
    public void setStreamFrameRateMilliHz(int frameRateMilliHz) {}

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
