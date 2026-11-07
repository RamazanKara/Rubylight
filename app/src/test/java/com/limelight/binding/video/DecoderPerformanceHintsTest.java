package com.limelight.binding.video;

import org.junit.Test;

import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Paths;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

/** What the decoder threads tell ADPF is work: CPU time, never the time they wait. */
public class DecoderPerformanceHintsTest {
    private static String renderer() throws Exception {
        return new String(Files.readAllBytes(
                Paths.get("src/main/java/com/limelight/binding/video/MediaCodecDecoderRenderer.java")), StandardCharsets.UTF_8);
    }

    // The renderer loop needs a running codec and a display, so this pins its wiring: the debt wait
    // (up to DecoderQueuePolicy.Debt.WAIT_US) and blocking releases sit inside the reported window,
    // so the window must be measured in thread CPU time, not wall time.
    @Test
    public void theRendererThreadReportsItsCpuTimeNotTheWaitsInItsHandOff() throws Exception {
        String source = renderer();
        int thread = source.indexOf("private void startRendererThread()");
        assertTrue(thread > 0);
        int end = source.indexOf("rendererThread.start();", thread);
        String loop = source.substring(thread, end);
        int report = loop.indexOf("performanceHints.reportWorkDuration(");
        assertTrue(report > 0);
        String argument = loop.substring(report, loop.indexOf(';', report));
        assertTrue("reports thread CPU time", argument.contains("Debug.threadCpuTimeNanos() - workCpuStartNs"));
        assertFalse("never wall time", argument.contains("System.nanoTime()"));
        int start = loop.indexOf("workCpuStartNs = Debug.threadCpuTimeNanos();");
        int dequeue = loop.indexOf("videoDecoder.dequeueOutputBuffer(info, 50000)");
        assertTrue("the window starts after the idle dequeue", start > dequeue && dequeue > 0);
    }
}
