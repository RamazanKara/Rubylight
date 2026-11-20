package com.limelight.utils;

import java.util.ArrayDeque;
import java.util.Iterator;

/**
 * Frames that arrived at least {@link #thresholdUs} later than the host's timestamps put them after
 * the frame before, for the stream doctor's BURSTS vote (nightjar-policy's stats::LateFrames): a
 * network, or this device's Wi-Fi, holding frames back and handing them over in a burst. A still
 * desktop, a game that slows down or a capture stall, where the timestamps step as far as the
 * arrivals, is never late.
 * <p>
 * The timestamps are taken at capture, so a frame the host itself sent later after its capture than
 * the frame before (a slower encode, a wait for its turn, paced sending after a loss) also arrives
 * later, and that change comes off. The host's timing report for the frame tells it: present to last
 * packet sent less the send stage, i.e. present to the first packet sent, which is when the measured
 * first packet left. Frames and reports are matched by frame number. Without a report for the frame
 * or the one before, the change of the frame header's processing time comes off instead, which
 * covers only packetizing.
 * <p>
 * Once the host has sent a report, a frame waits for its own (reports follow the frame's last
 * packet) until a frame {@link #REPORT_WAIT_US} newer arrives, then counts without it. A late frame
 * counts in the tick it is judged in. Fed by the decoder and control threads, read by the health
 * sample.
 */
public final class LateFrames {
    /** Timestamp steps longer than this start over (a new stream or a long pause). */
    private static final long MAX_HOST_STEP_US = 1_000_000;
    /** A frame waits for its timing report until a frame this much newer arrives. */
    public static final long REPORT_WAIT_US = 250_000;
    /** Frames waiting for reports past this count without them: a quarter second at 2000 fps. */
    private static final int MAX_PENDING = 512;
    /** Reports kept for frames that have not arrived yet. */
    private static final int MAX_EARLY = 64;

    private static final class Arrival {
        final int frame;
        final long ptsUs, processingUs, receiveUs;
        final int fps;
        /** The timing report's present to first packet sent, -1 until it comes. */
        long firstSentUs;

        Arrival(int frame, long ptsUs, long processingUs, long receiveUs, int fps, long firstSentUs) {
            this.frame = frame;
            this.ptsUs = ptsUs;
            this.processingUs = processingUs;
            this.receiveUs = receiveUs;
            this.fps = fps;
            this.firstSentUs = firstSentUs;
        }
    }

    /** The last frame judged, null before the first. */
    private Arrival last;
    /** Frames waiting for their timing report, oldest first. */
    private final ArrayDeque<Arrival> pending = new ArrayDeque<>();
    /** Reports that came before their frame: frame number and present to first packet sent. */
    private final ArrayDeque<long[]> early = new ArrayDeque<>();
    /** The host sends timing reports: frames wait for theirs. */
    private boolean reports;
    private int count;

    /**
     * How late a frame must be to count: three quarters of a frame interval, at least 8 ms. At that
     * delay the frame misses its refresh and the one before is shown twice.
     */
    public static long thresholdUs(int fps) {
        return fps <= 0 ? Long.MAX_VALUE : Math.max(750_000L / fps, 8_000L);
    }

    /** {@link #push(int, long, long, long, int)} without a frame number, as from a host that sends no timing reports. */
    public void push(long ptsUs, long processingUs, long receiveUs, int fps) {
        push(0, ptsUs, processingUs, receiveUs, fps);
    }

    /**
     * Frame {@code frame} arrived (its first packet at {@code receiveUs}) with the host timestamp
     * {@code ptsUs} after {@code processingUs} of host processing as its header says (0 when the host
     * does not report it).
     */
    public synchronized void push(int frame, long ptsUs, long processingUs, long receiveUs, int fps) {
        long firstSentUs = -1;
        for (Iterator<long[]> it = early.iterator(); it.hasNext(); ) {
            long[] report = it.next();
            if ((int) report[0] == frame) {
                firstSentUs = report[1];
                it.remove();
                break;
            }
        }
        pending.addLast(new Arrival(frame, ptsUs, processingUs, receiveUs, fps, firstSentUs));
        settle(receiveUs);
    }

    /**
     * The host's timing report for frame {@code frame}: present to last packet sent ({@code hostUs})
     * and its send stage, first to last packet ({@code sendUs}).
     */
    public synchronized void report(int frame, long hostUs, long sendUs) {
        reports = true;
        long firstSentUs = Math.max(0, hostUs - sendUs);
        Arrival waiting = null;
        for (Arrival arrival : pending) {
            if (arrival.frame == frame) {
                waiting = arrival;
                break;
            }
        }
        if (waiting != null) {
            waiting.firstSentUs = firstSentUs;
        } else if (last == null || frame - last.frame > 0) {
            if (early.size() == MAX_EARLY) early.removeFirst();
            early.addLast(new long[] {frame, firstSentUs});
        }
        settle(-1);
    }

    /**
     * Judges the waiting frames, oldest first, that have their report, have waited until the arrival
     * at {@code nowUs} (-1 for none), or need none because the host sends no reports.
     */
    private void settle(long nowUs) {
        while (!pending.isEmpty()) {
            Arrival next = pending.getFirst();
            boolean ready = !reports || next.firstSentUs >= 0 ||
                    (nowUs >= 0 && nowUs - next.receiveUs >= REPORT_WAIT_US) || pending.size() > MAX_PENDING;
            if (!ready) break;
            pending.removeFirst();
            judge(next);
        }
    }

    private void judge(Arrival frame) {
        if (last != null) {
            long hostUs = frame.ptsUs - last.ptsUs;
            if (hostUs > 0 && hostUs <= MAX_HOST_STEP_US && frame.receiveUs >= last.receiveUs) {
                // Only a change of the host's own time moves the arrival, and only when both are known:
                // from the reports, else from the headers.
                long hostStepUs;
                if (frame.firstSentUs >= 0 && last.firstSentUs >= 0) {
                    hostStepUs = frame.firstSentUs - last.firstSentUs;
                } else if (frame.processingUs > 0 && last.processingUs > 0) {
                    hostStepUs = frame.processingUs - last.processingUs;
                } else {
                    hostStepUs = 0;
                }
                if (frame.receiveUs - last.receiveUs - hostUs - hostStepUs >= thresholdUs(frame.fps)) {
                    count++;
                }
            }
        }
        last = frame;
    }

    /** The late frames judged since the last tick, then starts the next one. */
    public synchronized int tick() {
        int late = count;
        count = 0;
        return late;
    }
}
