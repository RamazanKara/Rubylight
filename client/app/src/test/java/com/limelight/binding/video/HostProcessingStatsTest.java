package com.limelight.binding.video;

import org.junit.Test;

import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Paths;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

/**
 * The statistics overlay's Host processing is the video frame header's per-frame value, counted for
 * every received frame and never touched by host timing reports.
 */
public class HostProcessingStatsTest {
    private static VideoStats window(char... headerValues) {
        VideoStats stats = new VideoStats();
        for (char value : headerValues) {
            MediaCodecDecoderRenderer.recordHostProcessing(stats, value);
            stats.totalFramesReceived++;
        }
        return stats;
    }

    @Test
    public void headerValuesAverageOverTheFramesThatCarriedOne() {
        // 0.5, 0.6 and 0.4 ms; a frame without a value neither counts nor drags the average down.
        VideoStats stats = window((char) 5, (char) 6, (char) 0, (char) 4);
        assertEquals(0.5f, stats.getHostProcessingMs(), 0.00001f);
        assertEquals(4, stats.minHostProcessingLatency);
        assertEquals(6, stats.maxHostProcessingLatency);
        assertEquals(3, stats.framesWithHostProcessingLatency);
        assertEquals(-1, window((char) 0, (char) 0).getHostProcessingMs(), 0);
    }

    @Test
    public void hostTimingReportsLeaveTheHeaderAverageAlone() {
        FrameLatencyStats latency = new FrameLatencyStats();
        VideoStats stats = new VideoStats();
        for (int i = 1; i <= 3; i++) {
            long ns = i * 8_000_000L;
            MediaCodecDecoderRenderer.recordHostProcessing(stats, (char) 5);
            latency.onDecoderInput(i, i, ns, ns, ns, (char) 5);
        }
        float before = stats.getHostProcessingMs();
        // Reports say 4.9 ms present to sent; the header said 0.5 ms.
        latency.onHostTiming(PerformanceOverlayTest.hostReport(4900, 100, 600, 4200), 40_000_000L);
        assertEquals(0.5f, before, 0.00001f);
        assertEquals(before, stats.getHostProcessingMs(), 0);
        assertEquals(3, stats.framesWithHostProcessingLatency);
    }

    @Test
    public void everyPathCountsIntoTheWindowTheFrameBelongsTo() {
        // The window flips between frames: the older frames sit in the last window, and the overlay
        // averages the last window plus the active one, like Artemis.
        VideoStats last = window((char) 5, (char) 5);
        VideoStats active = window((char) 10, (char) 0, (char) 5);
        VideoStats lastTwo = new VideoStats();
        lastTwo.add(last);
        lastTwo.add(active);
        assertEquals(25f / 4 / 10, lastTwo.getHostProcessingMs(), 0.00001f);
    }

    // submitDecodeUnit hands PyroWave frames to submitPyroWaveFrame, which returns twice, and the
    // native renderer cannot run in a JVM test, so this pins the wiring: the count comes first.
    @Test
    public void pyroWaveFramesAreCountedBeforeTheBranchCanReturn() throws Exception {
        String source = new String(Files.readAllBytes(
                Paths.get("src/main/java/com/limelight/binding/video/MediaCodecDecoderRenderer.java")), StandardCharsets.UTF_8);
        int method = source.indexOf("public int submitDecodeUnit(");
        assertTrue(method > 0);
        int branch = source.indexOf("if ((videoFormat & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) != 0) {", method);
        assertTrue(branch > method);
        int delegated = source.indexOf("return submitPyroWaveFrame(", branch);
        int branchEnd = source.indexOf("return MoonBridge.", branch);
        assertTrue("the PyroWave branch hands the frame to submitPyroWaveFrame", delegated > branch && delegated < branchEnd);
        int mediaCodecCount = source.indexOf("recordHostProcessing(activeWindowVideoStats, frameHostProcessingLatency)", branch);
        int end = source.indexOf("\n    }\n", method);
        assertTrue("the MediaCodec path counts its own frames", mediaCodecCount > branchEnd && mediaCodecCount < end);

        int pyroWave = source.indexOf("private int submitPyroWaveFrame(");
        assertTrue(pyroWave > 0);
        int firstReturn = source.indexOf("return MoonBridge.", pyroWave);
        int count = source.indexOf("recordHostProcessing(activeWindowVideoStats, frameHostProcessingLatency)", pyroWave);
        assertTrue("PyroWave frames must be counted before the first return", count > pyroWave && count < firstReturn);
    }
}
