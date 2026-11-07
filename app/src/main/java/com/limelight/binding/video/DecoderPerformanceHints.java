package com.limelight.binding.video;

import android.content.Context;
import android.os.Build;
import android.os.PerformanceHintManager;
import android.os.Process;

import com.limelight.LimeLog;

import java.util.concurrent.atomic.AtomicLong;

final class DecoderPerformanceHints implements AutoCloseable {
    /** debug.nightjar.submit_adpf: the submit thread joins the renderer's session (off by default). */
    static final String SUBMIT_SWITCH = "submit_adpf";
    /** debug.nightjar.adpf_cpu_time: the renderer reports thread CPU time, not wall time (on by default). */
    static final String CPU_TIME_SWITCH = "adpf_cpu_time";

    private PerformanceHintManager manager;
    private PerformanceHintManager.Session session;
    private long targetDurationNs;
    private int ownThreadId;
    private int submitThreadId;

    DecoderPerformanceHints(Context context, boolean enabled, int frameRate) {
        if (enabled && Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            manager = context.getSystemService(PerformanceHintManager.class);
            if (manager != null) {
                targetDurationNs = 1000000000L / frameRate;
                ownThreadId = Process.myTid();
                session = manager.createHintSession(new int[] { ownThreadId }, targetDurationNs);
            }
        }
    }

    /**
     * Puts the thread that submits decode units in the session beside the creating thread, so the
     * governor boosts the whole path and not just the hand-off. A new submit thread replaces the
     * last one; the same one is a no-op.
     */
    void setSubmitThread(int threadId) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S || session == null || threadId == 0 ||
                threadId == submitThreadId) {
            return;
        }
        // Remembered even when refused, so a refusal is logged once and not retried every frame.
        submitThreadId = threadId;
        int[] threads = threadId == ownThreadId ? new int[] { ownThreadId } : new int[] { ownThreadId, threadId };
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            try {
                session.setThreads(threads);
            } catch (IllegalArgumentException | SecurityException e) {
                LimeLog.warning("Performance hint session refused submit thread " + threadId + ": " + e);
            }
        } else {
            PerformanceHintManager.Session replacement = manager.createHintSession(threads, targetDurationNs);
            if (replacement == null) {
                LimeLog.warning("Performance hint session could not add submit thread " + threadId);
            } else {
                session.close();
                session = replacement;
            }
        }
    }

    void reportWorkDuration(long durationNs, int frameRate) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S && session != null) {
            long frameIntervalNs = 1000000000L / frameRate;
            if (frameIntervalNs != targetDurationNs) {
                session.updateTargetWorkDuration(frameIntervalNs);
                targetDurationNs = frameIntervalNs;
            }
            session.reportActualWorkDuration(Math.max(1, durationNs));
        }
    }

    @Override
    public void close() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S && session != null) {
            session.close();
            session = null;
        }
    }

    /**
     * The submit thread's CPU time since the renderer last reported, so each frame's report covers
     * both threads. CPU time, not wall time: the submit thread blocks for input buffers and spacing,
     * and that waiting is no work for ADPF (wall time was why the first version was reverted).
     */
    static final class SubmitWork {
        private final AtomicLong unreportedNs = new AtomicLong();
        private volatile int threadId;

        /** On the submit thread, after a decode unit: its thread id and the CPU time it took. */
        void add(int submitThreadId, long cpuNs) {
            threadId = submitThreadId;
            if (cpuNs > 0) {
                unreportedNs.addAndGet(cpuNs);
            }
        }

        /** On the renderer thread, with its report: the submit time since the last take. */
        long take() {
            return unreportedNs.getAndSet(0);
        }

        int threadId() {
            return threadId;
        }

        /** A codec recovery drops the frames whose submissions are not reported yet. */
        void reset() {
            unreportedNs.set(0);
        }
    }
}
