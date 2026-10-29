package com.limelight.policy;

import com.limelight.utils.LateFrames;

/**
 * Drives LateFrames from the shared traces. They pass the RTP timestamp (90 kHz) as the desktop
 * decoder gets it; Android's decoder gets it in microseconds. Rust's twin is the subject in stats.rs.
 */
final class LateFramesTrace {
    private final LateFrames late = new LateFrames();

    void push(long rtp, long processingUs, long receiveUs, int fps) {
        late.push(rtp * 1000 / 90, processingUs, receiveUs, fps);
    }

    int tick() {
        return late.tick();
    }
}
