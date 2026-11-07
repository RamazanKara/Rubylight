package com.limelight.nvstream.av.video;

import com.limelight.nvstream.jni.MoonBridge;

import org.junit.Test;

import java.nio.ByteBuffer;
import java.util.Arrays;

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertEquals;

public class VideoDecoderRendererDirectTest {
    /** A renderer that only takes arrays, as every renderer did before direct hand-off. */
    private static final class ArrayRenderer extends VideoDecoderRenderer {
        byte[] data;
        int length, type, frameNumber, frameType, calls;
        char hostProcessing;
        long receiveTimeUs, enqueueTimeUs, receiveTimeNs, presentationTimeUs;

        @Override
        public int submitDecodeUnit(byte[] decodeUnitData, int decodeUnitLength, int decodeUnitType,
                                    int frameNumber, int frameType, char frameHostProcessingLatency,
                                    long receiveTimeUs, long enqueueTimeUs, long receiveTimeNs,
                                    long presentationTimeUs) {
            calls++;
            data = Arrays.copyOf(decodeUnitData, decodeUnitLength);
            length = decodeUnitLength;
            type = decodeUnitType;
            this.frameNumber = frameNumber;
            this.frameType = frameType;
            hostProcessing = frameHostProcessingLatency;
            this.receiveTimeUs = receiveTimeUs;
            this.enqueueTimeUs = enqueueTimeUs;
            this.receiveTimeNs = receiveTimeNs;
            this.presentationTimeUs = presentationTimeUs;
            return MoonBridge.DR_NEED_IDR;
        }

        @Override public int setup(int format, int width, int height, int redrawRate) { return 0; }
        @Override public void start() {}
        @Override public void stop() {}
        @Override public void cleanup() {}
        @Override public int getCapabilities() { return 0; }
        @Override public void setHdrMode(boolean enabled, byte[] hdrMetadata) {}
    }

    private static ByteBuffer direct(byte[] bytes) {
        ByteBuffer buffer = ByteBuffer.allocateDirect(bytes.length);
        buffer.put(bytes);
        buffer.flip();
        return buffer;
    }

    @Test
    public void rendererWithoutDirectSupportGetsTheFrameAsAnArray() {
        ArrayRenderer renderer = new ArrayRenderer();
        byte[] frame = {0, 0, 0, 1, 0x26, 1, 2, 3, 4, 5};
        ByteBuffer buffer = direct(frame);
        int result = renderer.submitDirectDecodeUnit(buffer, frame.length, MoonBridge.BUFFER_TYPE_PICDATA, 42,
                MoonBridge.FRAME_TYPE_IDR, (char) 17, 1000, 2000, 3000, 4000);
        // The renderer's own answer comes back, with every field as given.
        assertEquals(MoonBridge.DR_NEED_IDR, result);
        assertEquals(1, renderer.calls);
        assertArrayEquals(frame, renderer.data);
        assertEquals(frame.length, renderer.length);
        assertEquals(MoonBridge.BUFFER_TYPE_PICDATA, renderer.type);
        assertEquals(42, renderer.frameNumber);
        assertEquals(MoonBridge.FRAME_TYPE_IDR, renderer.frameType);
        assertEquals(17, renderer.hostProcessing);
        assertEquals(1000, renderer.receiveTimeUs);
        assertEquals(2000, renderer.enqueueTimeUs);
        assertEquals(3000, renderer.receiveTimeNs);
        assertEquals(4000, renderer.presentationTimeUs);
        // The caller's buffer is left as it was.
        assertEquals(0, buffer.position());
        assertEquals(frame.length, buffer.limit());
    }

    @Test
    public void copiesOnlyTheFrameAndReadsFromTheBufferStart() {
        ArrayRenderer renderer = new ArrayRenderer();
        byte[] big = new byte[64];
        Arrays.fill(big, (byte) 7);
        renderer.submitDirectDecodeUnit(direct(big), big.length, MoonBridge.BUFFER_TYPE_PICDATA, 1,
                MoonBridge.FRAME_TYPE_PFRAME, (char) 0, 0, 0, 0, 0);
        // A smaller frame after a larger one: the reused array holds more than the frame, the
        // renderer is told the frame's own length. A moved position does not shift the read.
        byte[] small = {9, 8, 7};
        ByteBuffer buffer = direct(small);
        buffer.position(2);
        renderer.submitDirectDecodeUnit(buffer, small.length, MoonBridge.BUFFER_TYPE_PICDATA, 2,
                MoonBridge.FRAME_TYPE_PFRAME, (char) 0, 0, 0, 0, 0);
        assertEquals(small.length, renderer.length);
        assertArrayEquals(small, renderer.data);
        assertEquals(2, buffer.position());
    }
}
