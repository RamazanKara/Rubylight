package com.limelight.nvstream.av.video;

public abstract class VideoDecoderRenderer {
    // The protocol setup callback carries whole FPS. Preserve a fractional launch rate separately
    // when the freshly discovered host supports it; renderers without pacing may ignore it.
    public void setStreamFrameRateMilliHz(int frameRateMilliHz) {}

    public int prepareVideoFormats(int formats, int width, int height, int redrawRate) {
        return formats;
    }

    public int preparePacked444Modes(int formats, int width, int height, int fps) { return 0; }

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

    public abstract void setHdrMode(boolean enabled, byte[] hdrMetadata);

    // A host timing report (control message 0x5536). Called on the native control thread, only
    // for hosts that announced the message and were asked for it.
    public void onHostTiming(HostTimingReport report) {}
}
