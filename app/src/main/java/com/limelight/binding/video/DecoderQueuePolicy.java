package com.limelight.binding.video;

import java.util.Locale;

final class DecoderQueuePolicy {
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
