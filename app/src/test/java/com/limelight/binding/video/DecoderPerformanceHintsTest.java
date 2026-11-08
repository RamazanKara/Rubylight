package com.limelight.binding.video;

import org.junit.Test;

import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Paths;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

/** What the decoder threads tell ADPF is work: CPU time, never the time they wait. */
public class DecoderPerformanceHintsTest {
    private static String renderer() throws Exception {
        return new String(Files.readAllBytes(
                Paths.get("src/main/java/com/limelight/binding/video/MediaCodecDecoderRenderer.java")), StandardCharsets.UTF_8);
    }

    private static String rendererLoop(String source) {
        int thread = source.indexOf("private void startRendererThread()");
        assertTrue(thread > 0);
        return source.substring(thread, source.indexOf("rendererThread.start();", thread));
    }

    // The renderer loop needs a running codec and a display, so this pins its wiring: the debt wait
    // (up to DecoderQueuePolicy.Debt.WAIT_US) and blocking releases sit inside the reported window,
    // so the window must be measured in thread CPU time, not wall time.
    @Test
    public void theRendererThreadReportsItsCpuTimeNotTheWaitsInItsHandOff() throws Exception {
        String loop = rendererLoop(renderer());
        int report = loop.indexOf("performanceHints.reportWorkDuration(");
        assertTrue(report > 0);
        assertEquals("performanceHints.reportWorkDuration(workNs, refreshRate)",
                loop.substring(report, loop.indexOf(';', report)));
        assertTrue("reports thread CPU time unless the switch restores wall time", loop.contains(
                "long workNs = cpuTime ? Debug.threadCpuTimeNanos() - workCpuStartNs :\n" +
                "                                        System.nanoTime() - workStartNs;"));
        assertTrue("CPU time by default", loop.contains(
                "MediaCodecHelper.debugSwitch(DecoderPerformanceHints.CPU_TIME_SWITCH, true)"));
        assertEquals("adpf_cpu_time", DecoderPerformanceHints.CPU_TIME_SWITCH);
        assertTrue("off, no thread clock is read", loop.contains("if (cpuTime) workCpuStartNs = Debug.threadCpuTimeNanos();"));
        int start = loop.indexOf("workCpuStartNs = Debug.threadCpuTimeNanos();");
        int dequeue = loop.indexOf("videoDecoder.dequeueOutputBuffer(info, 50000)");
        assertTrue("the window starts after the idle dequeue", start > dequeue && dequeue > 0);
    }

    @Test
    public void submitWorkHandsEachSubmissionToTheNextReportOnce() {
        DecoderPerformanceHints.SubmitWork work = new DecoderPerformanceHints.SubmitWork();
        assertEquals(0, work.take());
        assertEquals(0, work.threadId());
        work.add(42, 300_000);
        work.add(42, 200_000);
        assertEquals(42, work.threadId());
        assertEquals(500_000, work.take());
        // Reported once: the next frame starts from nothing.
        assertEquals(0, work.take());
        // A clock that steps back is no negative work.
        work.add(43, -5);
        assertEquals(0, work.take());
        assertEquals("a new submit thread is the one put in the session", 43, work.threadId());
        // A recovery drops what its flushed frames took.
        work.add(43, 1_000);
        work.reset();
        assertEquals(0, work.take());
    }

    // The submit side must measure CPU time and only when the switch is on: off, a frame costs
    // no clock reads (no added latency by default).
    @Test
    public void submitThreadTimingIsCpuTimeAndBehindItsSwitch() throws Exception {
        String source = renderer();
        int method = source.indexOf("public int submitDecodeUnit(");
        assertTrue(method > 0);
        String wrapper = source.substring(method, source.indexOf("private int queueDecodeUnit(", method));
        int off = wrapper.indexOf("if (!submitHints) {");
        int firstClock = wrapper.indexOf("Debug.threadCpuTimeNanos()");
        assertTrue("off returns before any clock read", off > 0 && firstClock > off &&
                wrapper.indexOf("return queueDecodeUnit(", off) < firstClock);
        assertTrue(wrapper.contains("submitWork.add(Process.myTid(), Debug.threadCpuTimeNanos() - cpuStartNs);"));
        assertFalse(wrapper.contains("System.nanoTime()"));
        assertTrue("the setting (off by default, SettingsKeysTest), which the property overrides",
                source.contains("MediaCodecHelper.debugSwitch(DecoderPerformanceHints.SUBMIT_SWITCH, prefs.submitHint)"));
        assertEquals("submit_adpf", DecoderPerformanceHints.SUBMIT_SWITCH);
    }
}
