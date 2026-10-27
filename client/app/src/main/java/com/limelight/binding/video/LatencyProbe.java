package com.limelight.binding.video;

import java.util.Arrays;

/**
 * Measures input to screen on a still host screen: the host only sends a frame when the picture
 * changes, so the first frame received after a small mouse nudge is the host's answer to it.
 * The time runs from sending the nudge to Android reporting that frame on the display.
 */
public final class LatencyProbe {
    // A frame within this long before the nudge means the screen is still changing by itself.
    public static final long IDLE_NS = 150_000_000L;

    private long lastReceiveNs;
    private long sentNs;
    private long resultNs = -1;

    synchronized void onFrameReceived(long receiveNs) {
        lastReceiveNs = Math.max(lastReceiveNs, receiveNs);
    }

    synchronized void onFrameRendered(long receiveNs, long renderNs) {
        if (sentNs != 0 && resultNs < 0 && receiveNs >= sentNs && renderNs >= receiveNs) {
            resultNs = renderNs - sentNs;
            notifyAll();
        }
    }

    /** Whether no frame has arrived for IDLE_NS. */
    public synchronized boolean isIdle(long nowNs) {
        return nowNs - lastReceiveNs >= IDLE_NS;
    }

    /** Call right before sending the nudge. */
    public synchronized void arm(long nowNs) {
        sentNs = nowNs;
        resultNs = -1;
    }

    /** Input-to-screen time in nanoseconds for the armed nudge, or -1 if no frame showed up in time. */
    public synchronized long await(long timeoutMs) throws InterruptedException {
        long deadline = System.nanoTime() + timeoutMs * 1_000_000L;
        long left;
        while (resultNs < 0 && (left = deadline - System.nanoTime()) > 0) {
            wait(Math.max(1, left / 1_000_000L));
        }
        long result = resultNs;
        sentNs = 0;
        resultNs = -1;
        return result;
    }

    /** {best, median, p90} in milliseconds, or null without samples. */
    public static float[] summarize(long[] samplesNs, int count) {
        if (count == 0) return null;
        long[] sorted = Arrays.copyOf(samplesNs, count);
        Arrays.sort(sorted);
        return new float[] {sorted[0] / 1e6f, sorted[count / 2] / 1e6f,
                sorted[Math.min(count - 1, (int) Math.ceil(count * 0.9) - 1)] / 1e6f};
    }
}
