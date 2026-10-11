package com.limelight.binding.video;

import com.limelight.preferences.PreferenceConfiguration;

final class FramePacingPolicy {
    static boolean dropQueuedFrames(int mode) {
        return mode != PreferenceConfiguration.FRAME_PACING_MAX_SMOOTHNESS &&
                mode != PreferenceConfiguration.FRAME_PACING_CAP_FPS;
    }

    static boolean isFrameDue(long dueNs, long vsyncNs, long vsyncIntervalNs) {
        return vsyncNs + vsyncIntervalNs / 2 >= dueNs;
    }

    static long nextFrameTimeNs(long dueNs, long vsyncNs, int fps) {
        long intervalNs = 1_000_000_000L / fps;
        if (dueNs == 0) return vsyncNs + intervalNs;
        return dueNs + ((vsyncNs - dueNs) / intervalNs + 1) * intervalNs;
    }
}
