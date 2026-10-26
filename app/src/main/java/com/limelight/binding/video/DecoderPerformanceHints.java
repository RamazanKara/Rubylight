package com.limelight.binding.video;

import android.content.Context;
import android.os.Build;
import android.os.PerformanceHintManager;
import android.os.Process;

final class DecoderPerformanceHints implements AutoCloseable {
    private PerformanceHintManager.Session session;
    private long targetDurationNs;

    DecoderPerformanceHints(Context context, boolean enabled, int frameRate) {
        if (enabled && Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            PerformanceHintManager manager = context.getSystemService(PerformanceHintManager.class);
            if (manager != null) {
                targetDurationNs = 1000000000L / frameRate;
                session = manager.createHintSession(new int[] { Process.myTid() }, targetDurationNs);
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
}
