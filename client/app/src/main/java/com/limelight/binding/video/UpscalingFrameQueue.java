package com.limelight.binding.video;

final class UpscalingFrameQueue {
    private final long[] timestamps = new long[64];
    private final long[] targets = new long[64];
    // The decoder's presentation time of each frame (microseconds), which Frame Warp finds its frame number by; -1 unknown.
    private final long[] pts = new long[64];
    private int head, size, overflow;
    private long lastTimestamp, consumedTimestamp;

    synchronized long release(long nowNs, long targetNs) {
        return release(nowNs, targetNs, -1);
    }

    synchronized long release(long nowNs, long targetNs, long ptsUs) {
        if (size == timestamps.length) {
            head = (head + 1) % timestamps.length;
            size--;
            overflow++;
        }
        int tail = (head + size++) % timestamps.length;
        lastTimestamp = Math.max(nowNs, lastTimestamp + 1);
        timestamps[tail] = lastTimestamp;
        targets[tail] = targetNs;
        pts[tail] = ptsUs;
        return lastTimestamp;
    }

    // SurfaceTexture may coalesce callbacks and latch the newest of several released buffers.
    // frame gets the release token, the target, the frames skipped and, given a fourth slot, the pts.
    synchronized boolean consume(long timestampNs, long[] frame) {
        if (timestampNs <= consumedTimestamp) return false;
        for (int skipped = 0; skipped < size; skipped++) {
            int index = (head + skipped) % timestamps.length;
            if (timestamps[index] == timestampNs) {
                frame[0] = timestamps[index];
                frame[1] = targets[index];
                frame[2] = skipped + overflow;
                if (frame.length > 3) frame[3] = pts[index];
                head = (index + 1) % timestamps.length;
                size -= skipped + 1;
                overflow = 0;
                consumedTimestamp = timestampNs;
                return true;
            }
        }
        return false;
    }

    synchronized long oldestReleaseNs() {
        return size == 0 ? 0 : timestamps[head];
    }
}
