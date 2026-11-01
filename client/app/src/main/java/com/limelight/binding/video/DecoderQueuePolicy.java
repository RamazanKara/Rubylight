package com.limelight.binding.video;

import java.util.Locale;

final class DecoderQueuePolicy {
    /**
     * Frames the decoder holds: submitted less returned since the last flush, whose frames are gone
     * (base is the difference at that moment). Never negative.
     */
    static int depth(int framesIn, int framesOut, int base) {
        return Math.max(0, framesIn - framesOut - base);
    }

    static int outputLimit(String decoderName, boolean advertisedLowLatency, boolean dropLateFrames,
                           boolean recovered) {
        if (dropLateFrames || recovered) return 1;
        String name = decoderName.toLowerCase(Locale.ROOT);
        // Old OMX families in decoder-errata.txt must return scarce output buffers promptly.
        // These are client-held buffers, not codec reference pictures or vendor queue knobs.
        if (name.startsWith("omx.mtk.") || name.startsWith("omx.exynos.") ||
                name.startsWith("omx.nvidia.") || name.startsWith("omx.brcm.") ||
                name.startsWith("omx.ti.") || name.startsWith("omx.allwinner.") ||
                name.startsWith("omx.amlogic.") || name.startsWith("omx.intel.")) return 1;
        // Modern SoC names alone are not evidence of low-latency support.
        return advertisedLowLatency ? 2 : 1;
    }

    /**
     * Residency governor. A frame's input-to-output time is compared with the smallest seen in the
     * last ten seconds; when the excess stays above half a content interval for [FRAMES] frames in
     * a row, the decoder is holding a surplus (a few ms, issue 7) that the age rule in
     * [FramePacingPolicy] cannot see, and the older output is dropped when a newer one is already
     * decoded. At most one drop in [PERIOD] outputs, and none when no output is behind it. Only
     * decoded outputs are dropped, never compressed reference frames.
     */
    static final class Residency {
        static final int FRAMES = 30;
        static final int PERIOD = 4;
        static final int WINDOWS = 10;
        static final long WINDOW_NS = 1_000_000_000L;

        private final long[] windowMinNs = new long[WINDOWS];
        private int windows;
        private int slot;
        private long windowStartNs;
        private long currentMinNs = Long.MAX_VALUE;
        private int streak;
        private int sinceDrop = PERIOD;

        /**
         * One decoder output: input-to-output time and the clock reading. Returns the excess over
         * the ten second minimum (zero for the first frame or an unknown time).
         */
        long onOutput(long inputToOutputNs, long nowNs) {
            if (inputToOutputNs <= 0) return 0;
            if (windowStartNs == 0) windowStartNs = nowNs;
            if (nowNs - windowStartNs >= WINDOW_NS) {
                windowMinNs[slot] = currentMinNs;
                slot = (slot + 1) % WINDOWS;
                windows = Math.min(WINDOWS, windows + 1);
                currentMinNs = Long.MAX_VALUE;
                windowStartNs = nowNs;
            }
            currentMinNs = Math.min(currentMinNs, inputToOutputNs);
            long baseline = currentMinNs;
            for (int i = 0; i < windows; i++) baseline = Math.min(baseline, windowMinNs[i]);
            return inputToOutputNs - baseline;
        }

        /** Counts the output for the streak and the drop period; call after [onOutput], once per output. */
        void track(long excessNs, long contentIntervalNs) {
            streak = excessNs > contentIntervalNs / 2 ? streak + 1 : 0;
            if (sinceDrop < PERIOD) sinceDrop++;
        }

        /** framesBehind: outputs still in the decoder behind this one. */
        boolean shouldDrop(int framesBehind) {
            return framesBehind >= 1 && streak >= FRAMES && sinceDrop >= PERIOD;
        }

        void onDropped() {
            sinceDrop = 0;
        }

        void reset() {
            windows = 0;
            slot = 0;
            windowStartNs = 0;
            currentMinNs = Long.MAX_VALUE;
            streak = 0;
            sinceDrop = PERIOD;
        }
    }

    /**
     * Decoder debt in Smoothest and the frame rate limit, which show every frame (issue 7). After a
     * start or a stall the decoder can keep one or two frames waiting inside it for a minute:
     * input-to-output stays a frame or more above its floor while the frame behind is not decoded
     * yet, so the age rule in [FramePacingPolicy] has no newer output to swap in.
     *
     * The floor starts as the smallest input-to-output time since the last flush. An output is
     * over when it is at least three quarters of a content interval above the floor. Once
     * [SHARE_PERCENT] of the outputs of the last [WINDOW_NS] were over, the held output gives way to
     * a newer one: while a newer frame is in the decoder behind it, the renderer waits up to
     * [WAIT_US] for that frame to come out and drops the held output only if it does. One drop at
     * a time: the next [SETTLE] outputs are measured after each, and recovery stops once their
     * median is under the threshold again. Sub-frame drift between the display's clock and the
     * host's stays under the threshold, and a decoder whose floor is a frame long has no excess:
     * both are left alone.
     *
     * A drop that did not take half a frame off, or no newer output in [MAX_WAITS] waits or in a
     * whole window, mean the excess is the decoder's own pipelining for this content (deeper in
     * motion than in a still scene, say) or frames it will never output. Then the floor rises to
     * the level measured and recovery pauses until a whole window is healthy, so the next scene
     * change does not start it again; real debt still shrinks when a frame is dropped. [MAX_DROPS]
     * drops pause it without moving the floor.
     */
    static final class Debt {
        static final long WINDOW_NS = 1_500_000_000L;
        static final int SHARE_PERCENT = 80;
        static final int MIN_OUTPUTS = 20;
        static final int SETTLE = 3;
        static final int MAX_DROPS = 8;
        static final long WAIT_US = 2_000;
        static final int MAX_WAITS = 60;
        private static final int CAPACITY = 1024;
        private static final int WATCH = 0;
        private static final int RECOVER = 1;
        private static final int PAUSED = 2;

        // The window: outputs of the last WINDOW_NS, oldest at head.
        private final long[] sampleNs = new long[CAPACITY];
        private final boolean[] sampleOver = new boolean[CAPACITY];
        private int head;
        private int count;
        private int overCount;
        private long windowStartNs;

        // The floor: the smallest input-to-output time, plus what recovery learned is the decoder's.
        private long minNs = Long.MAX_VALUE;
        private long learnedNs;
        private long intervalNs;
        private long excessNs;
        private boolean over;
        private final long[] recentNs = new long[3];
        private int state = WATCH;
        private int drops;
        private int waits;
        private int settle;
        private final long[] settleNs = new long[SETTLE];
        private long droppedLevelNs;
        private long startLevelNs;
        private long readySinceNs;
        private int under;
        private String event;

        /**
         * One decoder output: its input-to-output time, the clock reading and the content interval.
         * Returns its excess over the floor (zero for an unknown time).
         */
        long onOutput(long inputToOutputNs, long nowNs, long contentIntervalNs) {
            if (inputToOutputNs <= 0 || contentIntervalNs <= 0) {
                over = false;
                return 0;
            }
            intervalNs = contentIntervalNs;
            minNs = Math.min(minNs, inputToOutputNs);
            excessNs = Math.max(0, inputToOutputNs - floorNs());
            over = isOver(excessNs);
            recentNs[0] = recentNs[1];
            recentNs[1] = recentNs[2];
            recentNs[2] = excessNs;
            if (state == WATCH) {
                add(nowNs, over);
                if (windowShare(nowNs, true)) {
                    state = RECOVER;
                    drops = 0;
                    waits = 0;
                    settle = 0;
                    under = 0;
                    readySinceNs = nowNs;
                    startLevelNs = median(recentNs[0], recentNs[1], recentNs[2]);
                }
            } else if (state == RECOVER) {
                recover(nowNs);
            } else {
                add(nowNs, over);
                if (windowShare(nowNs, false)) {
                    state = WATCH;
                    clear();
                }
            }
            return excessNs;
        }

        /**
         * Whether the held output should give way to a newer one, if one comes out within
         * [WAIT_US]. framesBehind: frames in the decoder behind it. Call after [onOutput].
         */
        boolean shouldDrop(int framesBehind) {
            return state == RECOVER && settle == 0 && over && framesBehind >= 1 && drops < MAX_DROPS;
        }

        /** A newer output came out and replaced the held one. */
        void onDropped() {
            drops++;
            settle = SETTLE;
            droppedLevelNs = median(recentNs[0], recentNs[1], recentNs[2]);
        }

        /** No newer output came out within [WAIT_US] after [shouldDrop]; the held one was shown. */
        void onNoNewer() {
            if (state == RECOVER && ++waits >= MAX_WAITS) learn("paused, no newer output came out");
        }

        /** The one-line summary of a recovery that ended since the last call, or null. */
        String takeEvent() {
            String taken = event;
            event = null;
            return taken;
        }

        void reset() {
            clear();
            minNs = Long.MAX_VALUE;
            learnedNs = 0;
            excessNs = 0;
            over = false;
            java.util.Arrays.fill(recentNs, 0);
            state = WATCH;
            drops = 0;
            waits = 0;
            settle = 0;
            under = 0;
            event = null;
        }

        private void recover(long nowNs) {
            if (settle > 0) {
                settleNs[SETTLE - settle] = excessNs;
                if (--settle > 0) return;
                long level = median(settleNs[0], settleNs[1], settleNs[2]);
                if (!isOver(level)) {
                    finish("recovered", level, WATCH, false);
                } else if (level > droppedLevelNs - intervalNs / 2) {
                    learn("paused, a drop did not shorten the queue");
                } else if (drops >= MAX_DROPS) {
                    finish("paused at the drop limit", level, PAUSED, false);
                } else {
                    readySinceNs = nowNs;
                    waits = 0;
                    under = 0;
                }
                return;
            }
            if (!over) {
                if (++under >= 2) finish("recovered", excessNs, WATCH, false);
                return;
            }
            under = 0;
            if (nowNs - readySinceNs < WINDOW_NS) return;
            // Over for a whole window without a drop. Newer frames that never came out: learn
            // the level. Nothing behind to drop: watch again from scratch.
            if (waits > 0) learn("paused, no newer output came out");
            else finish("ended, nothing behind to drop", excessNs, WATCH, false);
        }

        /** The excess is the decoder's own: raise the floor to the level measured and pause. */
        private void learn(String result) {
            long level = median(recentNs[0], recentNs[1], recentNs[2]);
            learnedNs += level;
            finish(result, level, PAUSED, true);
        }

        private void finish(String result, long levelNs, int next, boolean learned) {
            if (drops > 0 || next == PAUSED) {
                event = String.format(java.util.Locale.ROOT,
                        "Decoder debt %s: %d stale output%s dropped, excess %.1f -> %.1f ms over a %.1f ms floor%s",
                        result, drops, drops == 1 ? "" : "s", startLevelNs / 1e6, levelNs / 1e6,
                        (floorNs() - (learned ? levelNs : 0)) / 1e6,
                        learned ? String.format(java.util.Locale.ROOT, ", floor raised to %.1f ms", floorNs() / 1e6) : "");
            }
            state = next;
            clear();
        }

        private long floorNs() {
            return minNs + learnedNs;
        }

        private boolean isOver(long excess) {
            return excess * 4 >= intervalNs * 3;
        }

        private void add(long nowNs, boolean isOver) {
            while (count > 0 && (count == CAPACITY || sampleNs[head] < nowNs - WINDOW_NS)) {
                if (sampleOver[head]) overCount--;
                head = (head + 1) % CAPACITY;
                count--;
            }
            if (count == 0) windowStartNs = nowNs;
            int tail = (head + count) % CAPACITY;
            sampleNs[tail] = nowNs;
            sampleOver[tail] = isOver;
            count++;
            if (isOver) overCount++;
        }

        /** Whether the window spans [WINDOW_NS] and at least [SHARE_PERCENT] of it is over (or under). */
        private boolean windowShare(long nowNs, boolean ofOver) {
            if (count < MIN_OUTPUTS || nowNs - windowStartNs < WINDOW_NS) return false;
            int matching = ofOver ? overCount : count - overCount;
            return matching * 100L >= count * (long) SHARE_PERCENT;
        }

        private void clear() {
            head = 0;
            count = 0;
            overCount = 0;
        }

        private static long median(long a, long b, long c) {
            return Math.max(Math.min(a, b), Math.min(Math.max(a, b), c));
        }
    }

    /**
     * Frames released to the compositor and not yet shown. A frame counts from its release until
     * it is rendered, or until the vsync it was planned for has passed (the render callback can
     * be late, and a frame the compositor replaced is never reported).
     */
    static final class InFlight {
        static final int MAX = 2;
        private static final int SLOTS = 8;

        private final long[] ptsUs = new long[SLOTS];
        private final long[] dueNs = new long[SLOTS];
        private int next;
        private long renderedPtsUs = Long.MIN_VALUE;

        synchronized void onReleased(long codecPtsUs, long dueAtNs) {
            ptsUs[next] = codecPtsUs;
            dueNs[next] = dueAtNs;
            next = (next + 1) % SLOTS;
        }

        synchronized void onRendered(long codecPtsUs) {
            renderedPtsUs = Math.max(renderedPtsUs, codecPtsUs);
        }

        synchronized int count(long nowNs) {
            int count = 0;
            for (int i = 0; i < SLOTS; i++) {
                if (dueNs[i] > nowNs && ptsUs[i] > renderedPtsUs) count++;
            }
            return count;
        }

        /** Another release would put more than [MAX] frames in front of the compositor. */
        synchronized boolean full(long nowNs) {
            return count(nowNs) >= MAX;
        }

        synchronized void reset() {
            java.util.Arrays.fill(dueNs, 0);
            java.util.Arrays.fill(ptsUs, 0);
            renderedPtsUs = Long.MIN_VALUE;
        }
    }
}
