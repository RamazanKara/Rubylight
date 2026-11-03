package com.limelight;

import org.junit.Test;

import java.util.concurrent.CountDownLatch;

import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

public class StreamStopReportTest {
    /** Nothing on the UI thread waits for a stream's stop (#69), so one stuck for good says where it is stuck. */
    @Test
    public void aStopStillRunningSaysWhereItIsStuck() throws Exception {
        CountDownLatch release = new CountDownLatch(1);
        Thread stopping = new Thread(() -> {
            try {
                release.await();
            } catch (InterruptedException ignored) {
                // The test is done.
            }
        }, "Stop stream");
        stopping.setDaemon(true);
        stopping.start();
        long deadline = System.nanoTime() + 10_000_000_000L;
        while (stopping.getState() != Thread.State.WAITING && System.nanoTime() < deadline) Thread.sleep(1);
        String report = GameConnection.stuckStopReport(stopping);
        release.countDown();
        stopping.join(5000);

        assertNotNull(report);
        assertTrue(report, report.startsWith("Stream stop still running after 10 s:"));
        assertTrue(report, report.contains("CountDownLatch.await"));
        assertNull("an ended stop is not reported", GameConnection.stuckStopReport(stopping));
    }
}
