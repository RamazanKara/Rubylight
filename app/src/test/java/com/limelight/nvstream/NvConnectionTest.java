package com.limelight.nvstream;

import org.junit.Test;

import static com.limelight.nvstream.jni.MoonBridge.*;
import static org.junit.Assert.*;

public class NvConnectionTest {
    private static final int CLIENT_HDR = VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265 |
            VIDEO_FORMAT_H265_MAIN10 | VIDEO_FORMAT_AV1_MAIN8 | VIDEO_FORMAT_AV1_MAIN10;
    private static final int PYROWAVE_CLIENT_FORMATS = 0x0F0000;
    private static final int PYROWAVE_HOST_FORMATS = 0x07800000;

    private NvConnection connection() {
        return new NvConnection(null,
                new com.limelight.nvstream.http.ComputerDetails.AddressTuple("127.0.0.1", 47989),
                47984, "1234567890abcdef", new StreamConfiguration.Builder()
                .setApp(new com.limelight.nvstream.http.NvApp("Desktop", 1, false)).build(), null, null);
    }

    private java.util.concurrent.Semaphore connectionSemaphore() throws Exception {
        java.lang.reflect.Field field = NvConnection.class.getDeclaredField("connectionAllowed");
        field.setAccessible(true);
        return (java.util.concurrent.Semaphore) field.get(null);
    }

    @Test
    public void stoppedConnectionCannotSendInputIntoTheNextNativeSession() {
        NvConnection connection = connection();
        assertTrue(connection.canSendInput());
        assertTrue(connection.canSendControllerInput());
        connection.stop();
        assertFalse(connection.canSendInput());
        assertFalse(connection.canSendControllerInput());
    }

    @Test
    public void otherInputPermissionsCannotReachTheControllerNativeBridge() throws Exception {
        NvConnection connection = connection();
        com.limelight.nvstream.http.ComputerDetails details = new com.limelight.nvstream.http.ComputerDetails();
        java.lang.reflect.Field field = NvConnection.class.getDeclaredField("hostDetails");
        field.setAccessible(true);
        field.set(connection, details);
        // Touch, pen, mouse, and keyboard permissions each permit general input,
        // but none can create a controller or send its state and capabilities.
        for (int bit = 9; bit <= 12; bit++) {
            details.permission = 1 << bit;
            assertTrue(connection.canSendInput());
            assertFalse(connection.canSendControllerInput());
            connection.sendControllerInput((short) 0, (short) 1, 0, (byte) 0, (byte) 0,
                    (short) 0, (short) 0, (short) 0, (short) 0);
            connection.sendControllerBatteryEvent((byte) 0, (byte) 0, (byte) 0);
            assertEquals(LI_ERR_UNSUPPORTED, connection.sendControllerArrivalEvent(
                    (byte) 0, (short) 1, (byte) 0, 0, (short) 0));
            assertEquals(LI_ERR_UNSUPPORTED, connection.sendControllerTouchEvent(
                    (byte) 0, (byte) 0, 0, 0, 0, 0));
            assertEquals(LI_ERR_UNSUPPORTED, connection.sendControllerMotionEvent(
                    (byte) 0, (byte) 0, 0, 0, 0));
        }
        details.permission = com.limelight.nvstream.http.ComputerDetails.PERMISSION_CONTROLLER;
        assertTrue(connection.canSendInput());
        assertTrue(connection.canSendControllerInput());
    }

    @Test
    public void inputWaitingForABatchCannotSendAfterStop() throws Exception {
        java.util.List<java.util.function.Consumer<NvConnection>> events = java.util.List.of(
                c -> c.sendMouseButtonDown((byte) 1),
                c -> c.sendMouseButtonUp((byte) 1),
                c -> c.sendKeyboardInput((short) 65, (byte) 3, (byte) 0, (byte) 0),
                c -> c.sendMouseScroll((byte) 1),
                c -> c.sendMouseHScroll((byte) 1),
                c -> c.sendMouseHighResScroll((short) 120),
                c -> c.sendMouseHighResHScroll((short) 120));
        for (java.util.function.Consumer<NvConnection> event : events) {
            NvConnection connection = connection();
            java.lang.reflect.Field field = NvConnection.class.getDeclaredField("inputBatcher");
            field.setAccessible(true);
            Object batcher = field.get(connection);
            java.util.concurrent.atomic.AtomicReference<Throwable> failure = new java.util.concurrent.atomic.AtomicReference<>();
            Thread input = new Thread(() -> {
                try {
                    event.accept(connection);
                } catch (Throwable e) {
                    failure.set(e);
                }
            });
            synchronized (batcher) {
                input.start();
                long deadline = System.nanoTime() + java.util.concurrent.TimeUnit.SECONDS.toNanos(2);
                while (input.getState() != Thread.State.BLOCKED && input.isAlive() && System.nanoTime() < deadline) {
                    Thread.sleep(1);
                }
                assertEquals(Thread.State.BLOCKED, input.getState());
                connection.stop();
            }
            input.join(2000);
            assertFalse(input.isAlive());
            assertNull("Stopped input reached JNI: " + failure.get(), failure.get());
        }
    }

    @Test
    public void stoppingBeforeStartIsIdempotentAndDoesNotReleaseAnotherConnectionsPermit() throws Exception {
        java.util.concurrent.Semaphore semaphore = connectionSemaphore();
        assertTrue(semaphore.tryAcquire());
        try {
            NvConnection connection = connection();
            connection.stop();
            connection.stop();
            connection.start(null, null, null);
            assertEquals(0, semaphore.availablePermits());
            assertThrows(java.io.IOException.class, connection::refreshHostDetails);
        } finally {
            semaphore.release();
        }
    }

    @Test
    public void cancellingAQueuedStartCleansRendererWithoutStealingNativeConnection() throws Exception {
        java.util.concurrent.Semaphore semaphore = connectionSemaphore();
        java.util.concurrent.CountDownLatch starting = new java.util.concurrent.CountDownLatch(1);
        java.util.concurrent.CountDownLatch cleaned = new java.util.concurrent.CountDownLatch(1);
        java.util.concurrent.atomic.AtomicInteger failures = new java.util.concurrent.atomic.AtomicInteger();
        NvConnectionListener listener = (NvConnectionListener) java.lang.reflect.Proxy.newProxyInstance(
                NvConnectionListener.class.getClassLoader(), new Class<?>[] {NvConnectionListener.class},
                (proxy, method, args) -> {
                    if (method.getName().equals("stageStarting")) {
                        starting.countDown();
                    } else if (method.getName().equals("stageFailed")) {
                        failures.incrementAndGet();
                    }
                    return null;
                });
        com.limelight.nvstream.av.video.VideoDecoderRenderer renderer =
                new com.limelight.nvstream.av.video.VideoDecoderRenderer() {
                    @Override public int setup(int format, int width, int height, int rate) {
                        throw new AssertionError("Cancelled connection reached native setup");
                    }
                    @Override public void start() { }
                    @Override public void stop() { }
                    @Override public int getCapabilities() { return 0; }
                    @Override public void setHdrMode(boolean enabled, byte[] metadata) { }
                    @Override public void cleanup() { cleaned.countDown(); }
                    @Override public int submitDecodeUnit(byte[] data, int length, int type, int frame,
                            int frameType, char hostLatency, long received, long enqueued, long receivedNs,
                            long presentationUs) {
                        throw new AssertionError("Cancelled connection received video");
                    }
                };
        assertTrue(semaphore.tryAcquire());
        NvConnection connection = connection();
        try {
            connection.start(null, renderer, listener);
            assertTrue(starting.await(2, java.util.concurrent.TimeUnit.SECONDS));
            connection.stop();
            assertTrue(cleaned.await(2, java.util.concurrent.TimeUnit.SECONDS));
            connection.stop();
            assertEquals(0, semaphore.availablePermits());
            assertEquals(0, failures.get());
        } finally {
            connection.stop();
            semaphore.release();
        }
    }

    @Test
    public void unknownCodecFamiliesNeverReachTheNativeHandshake() {
        int host = PYROWAVE_HOST_FORMATS | 0x30301;
        assertEquals(CLIENT_HDR | VIDEO_FORMAT_PYROWAVE_MAIN10 | VIDEO_FORMAT_PYROWAVE_MAIN10_444,
                NvConnection.negotiateVideoFormats(CLIENT_HDR | PYROWAVE_CLIENT_FORMATS | 0x40000000, host));
        assertEquals(PYROWAVE_CLIENT_FORMATS, NvConnection.negotiateVideoFormats(PYROWAVE_CLIENT_FORMATS, host));
    }

    @Test
    public void pyrowaveProfilesRequireTheirOwnHostBits() {
        int[] client = { VIDEO_FORMAT_PYROWAVE, VIDEO_FORMAT_PYROWAVE_444,
                VIDEO_FORMAT_PYROWAVE_MAIN10, VIDEO_FORMAT_PYROWAVE_MAIN10_444 };
        int[] host = { 0x00800000, 0x01000000, 0x02000000, 0x04000000 };
        for (int i = 0; i < client.length; i++) {
            assertEquals(client[i], NvConnection.negotiateVideoFormats(PYROWAVE_CLIENT_FORMATS, host[i]));
        }
        assertEquals(0, NvConnection.negotiateVideoFormats(PYROWAVE_CLIENT_FORMATS, 0x30301));
    }

    @Test
    public void surfaceFailureRetainsConventionalFallback() {
        int common = NvConnection.negotiateVideoFormats(CLIENT_HDR | PYROWAVE_CLIENT_FORMATS, 0x07830301);
        assertEquals(CLIENT_HDR, NvConnection.negotiateVideoFormats(common & ~VIDEO_FORMAT_MASK_PYROWAVE, 0x07830301));
        // PyroWave HDR must not erase ordinary SDR codecs needed on a bitstream mismatch.
        assertEquals(VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265 | VIDEO_FORMAT_AV1_MAIN8 | VIDEO_FORMAT_PYROWAVE_MAIN10,
                NvConnection.negotiateVideoFormats(CLIENT_HDR | VIDEO_FORMAT_PYROWAVE_MAIN10, 0x02010101));
    }

    @Test
    public void conventionalHdrWinsOverPyrowaveSdr() {
        assertEquals(CLIENT_HDR, NvConnection.negotiateVideoFormats(CLIENT_HDR |
                VIDEO_FORMAT_PYROWAVE | VIDEO_FORMAT_PYROWAVE_444, 0x01830301));
    }

    @Test
    public void pyrowaveHostStillUsesAv1OrHevcHdrWithOrdinaryClients() {
        for (int extension : new int[] {0x00800000, 0x01000000, 0x02000000, 0x04000000,
                PYROWAVE_HOST_FORMATS}) {
            assertEquals(VIDEO_FORMAT_H264 | VIDEO_FORMAT_AV1_MAIN8 | VIDEO_FORMAT_AV1_MAIN10,
                    NvConnection.negotiateVideoFormats(CLIENT_HDR, extension | 0x30101));
            assertEquals(VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265 | VIDEO_FORMAT_H265_MAIN10,
                    NvConnection.negotiateVideoFormats(CLIENT_HDR, extension | 0x10301));
        }
    }

    private static final int RUBYLIGHT_3_HOST = 0x47830301;

    /** The connection's own sequence: offer, intersect, set up the surface, intersect again. */
    private static int[] connect(int offered, int host, java.util.function.IntPredicate surface) {
        int common = NvConnection.negotiateVideoFormats(offered, host);
        int prepared = common;
        int pyroWave = com.limelight.binding.video.MediaCodecDecoderRenderer.firstPyroWaveFormat(common, surface);
        if ((common & VIDEO_FORMAT_MASK_PYROWAVE) != 0) {
            prepared = (common & ~VIDEO_FORMAT_MASK_PYROWAVE) | pyroWave;
        }
        return new int[] {common, NvConnection.negotiateVideoFormats(prepared, host)};
    }

    @Test
    public void forcedPyrowaveWithHdrKeepsPyrowaveMain10OnARubylight3Host() {
        int offered = VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265 | VIDEO_FORMAT_H265_MAIN10 | VIDEO_FORMAT_AV1_MAIN8 |
                VIDEO_FORMAT_AV1_MAIN10 | VIDEO_FORMAT_PYROWAVE | VIDEO_FORMAT_PYROWAVE_MAIN10;
        int[] flow = connect(offered, RUBYLIGHT_3_HOST, format -> true);
        assertEquals(VIDEO_FORMAT_PYROWAVE_MAIN10, flow[1] & VIDEO_FORMAT_MASK_PYROWAVE);
        assertFalse(NvConnection.pyroWaveHdrLost(flow[0], flow[1]));

        // With 4:4:4 on, the 10-bit 4:4:4 profile leads.
        flow = connect(offered | VIDEO_FORMAT_PYROWAVE_444 | VIDEO_FORMAT_PYROWAVE_MAIN10_444,
                RUBYLIGHT_3_HOST, format -> true);
        assertEquals(VIDEO_FORMAT_PYROWAVE_MAIN10_444, flow[1] & VIDEO_FORMAT_MASK_PYROWAVE);
    }

    @Test
    public void pyrowaveHdrSurfaceFailureFallsBackToHdrAndIsReported() {
        int offered = VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265 | VIDEO_FORMAT_H265_MAIN10 | VIDEO_FORMAT_AV1_MAIN8 |
                VIDEO_FORMAT_AV1_MAIN10 | VIDEO_FORMAT_PYROWAVE | VIDEO_FORMAT_PYROWAVE_MAIN10;
        // Only the SDR surface can be created: HDR stays on AV1, and the loss is detected.
        int[] flow = connect(offered, RUBYLIGHT_3_HOST, format -> (format & VIDEO_FORMAT_MASK_10BIT) == 0);
        assertEquals(0, flow[1] & VIDEO_FORMAT_MASK_PYROWAVE);
        assertTrue((flow[1] & VIDEO_FORMAT_AV1_MAIN10) != 0);
        assertTrue(NvConnection.pyroWaveHdrLost(flow[0], flow[1]));
        // A host without PyroWave 10-bit is an ordinary HDR stream, not a loss.
        flow = connect(offered, 0x01830301, format -> true);
        assertFalse(NvConnection.pyroWaveHdrLost(flow[0], flow[1]));
    }

    @Test
    public void pyrowaveHdrAnd444DoNotImplyConventionalHdrOr444() {
        int client = VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265 | VIDEO_FORMAT_H265_MAIN10 |
                VIDEO_FORMAT_H264_HIGH8_444 | VIDEO_FORMAT_H265_REXT8_444;
        assertEquals(VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265,
                NvConnection.negotiateVideoFormats(client, PYROWAVE_HOST_FORMATS | 0x101));
        assertEquals(VIDEO_FORMAT_H264,
                NvConnection.negotiateVideoFormats(VIDEO_FORMAT_H264, PYROWAVE_HOST_FORMATS | 1));
    }

    @Test
    public void prefersHevcHdrOverAv1Sdr() {
        int formats = NvConnection.negotiateVideoFormats(CLIENT_HDR, 0x10301);
        assertEquals(0, formats & VIDEO_FORMAT_MASK_AV1);
        assertEquals(VIDEO_FORMAT_H265_MAIN10, formats & VIDEO_FORMAT_MASK_10BIT);
    }

    @Test
    public void av1HdrUsesTheHostBitNotTheClientBit() {
        int formats = NvConnection.negotiateVideoFormats(CLIENT_HDR, 0x30101);
        assertEquals(VIDEO_FORMAT_AV1_MAIN10, formats & VIDEO_FORMAT_MASK_10BIT);
        assertEquals(0, formats & VIDEO_FORMAT_MASK_H265);
    }

    @Test
    public void mismatchedHdrCodecsFallBackToSdr() {
        int formats = NvConnection.negotiateVideoFormats(VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265 |
                VIDEO_FORMAT_H265_MAIN10, 0x30101);
        assertEquals(VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265, formats);
    }

    @Test
    public void missingHostCapabilitiesDoNotInventHdrOr444() {
        int formats = NvConnection.negotiateVideoFormats(CLIENT_HDR | VIDEO_FORMAT_H264_HIGH8_444 |
                VIDEO_FORMAT_H265_REXT8_444, 0);
        assertEquals(VIDEO_FORMAT_H264 | VIDEO_FORMAT_H265 | VIDEO_FORMAT_AV1_MAIN8, formats);
    }

    @Test
    public void prefersSupported444OverSdr420() {
        int formats = NvConnection.negotiateVideoFormats(CLIENT_HDR | VIDEO_FORMAT_H264_HIGH8_444, 0x50101);
        assertEquals(VIDEO_FORMAT_H264 | VIDEO_FORMAT_H264_HIGH8_444, formats);
    }

    @Test
    public void hdrTakesPriorityOverEightBit444() {
        int formats = NvConnection.negotiateVideoFormats(CLIENT_HDR | VIDEO_FORMAT_H265_REXT8_444, 0xB0101);
        assertEquals(VIDEO_FORMAT_AV1_MAIN10, formats & VIDEO_FORMAT_MASK_10BIT);
        assertEquals(0, formats & VIDEO_FORMAT_MASK_H265);
    }
}
