package com.limelight.binding.video;

import android.content.Context;
import android.os.Build;
import android.os.PerformanceHintManager;
import android.os.Process;

import com.limelight.LimeLog;

import java.util.Arrays;

final class DecoderPerformanceHints implements AutoCloseable {
    private PerformanceHintManager manager;
    private PerformanceHintManager.Session session;
    private long targetDurationNs;
    private int[] threadIds;

    DecoderPerformanceHints(Context context, boolean enabled, int frameRate) {
        if (enabled && Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            manager = context.getSystemService(PerformanceHintManager.class);
            if (manager != null) {
                targetDurationNs = 1000000000L / frameRate;
                threadIds = new int[] { Process.myTid() };
                session = manager.createHintSession(threadIds, targetDurationNs);
            }
        }
    }

    /**
     * Adds a thread that shares each frame's work with the creating thread, such as the one that
     * submits decode units, so the governor boosts the whole path and not just the hand-off.
     */
    void addThread(int threadId) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S || session == null || threadId == 0) {
            return;
        }
        for (int id : threadIds) {
            if (id == threadId) return;
        }
        int[] next = Arrays.copyOf(threadIds, threadIds.length + 1);
        next[threadIds.length] = threadId;
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            try {
                session.setThreads(next);
            } catch (IllegalArgumentException | SecurityException e) {
                LimeLog.warning("Performance hint session refused thread " + threadId + ": " + e);
                // Remember it anyway so the refusal is not retried for every frame.
            }
        } else {
            PerformanceHintManager.Session replacement = manager.createHintSession(next, targetDurationNs);
            if (replacement == null) {
                LimeLog.warning("Performance hint session could not add thread " + threadId);
            } else {
                session.close();
                session = replacement;
            }
        }
        threadIds = next;
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
}
