package com.limelight.binding.video;

/** Moonlight X's submission clock: round the accumulated time, never each frame. */
final class PyroWaveSubmissionTiming {
    private long remainderNs;

    // Lives for the stream, independently of the one-second VideoStats windows.
    long addDurationNs(long durationNs) {
        remainderNs += durationNs;
        long milliseconds = remainderNs / 1_000_000L;
        remainderNs -= milliseconds * 1_000_000L;
        return milliseconds;
    }
}
