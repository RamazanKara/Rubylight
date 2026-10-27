package com.limelight.binding.video;

import com.limelight.nvstream.av.video.VideoDecoderRenderer;
import com.limelight.nvstream.jni.MoonBridge;

/**
 * Takes the frames of a small-stream Input-only session and drops them unseen: no decoder, no
 * surface. Each frame counts as handled, so the core keeps acknowledging the stream and asks the
 * host for nothing extra. Frames arrive on the receive thread.
 */
public class DiscardingDecoderRenderer extends VideoDecoderRenderer {
    @Override
    public int setup(int format, int width, int height, int redrawRate) {
        return 0;
    }

    @Override
    public void start() {
    }

    @Override
    public void stop() {
    }

    @Override
    public int submitDecodeUnit(byte[] decodeUnitData, int decodeUnitLength, int decodeUnitType,
                                int frameNumber, int frameType, char frameHostProcessingLatency,
                                long receiveTimeUs, long enqueueTimeUs, long receiveTimeNs,
                                long presentationTimeUs) {
        return MoonBridge.DR_OK;
    }

    @Override
    public void cleanup() {
    }

    @Override
    public int getCapabilities() {
        return MoonBridge.CAPABILITY_DIRECT_SUBMIT;
    }

    @Override
    public void setHdrMode(boolean enabled, byte[] hdrMetadata) {
    }
}
