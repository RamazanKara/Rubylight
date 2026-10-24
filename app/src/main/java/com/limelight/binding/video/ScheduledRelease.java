package com.limelight.binding.video;

/**
 * Releases one decoded output with host frame times: the lateness check, the replacement of a
 * late or surplus frame by a newer output, and the plan onto the vsync grid. It talks to the codec
 * and the display only through {@link Port}, so the rules can be tested without either.
 *
 * Nothing here waits: a frame that cannot be replaced is released at once, and the plan puts it on
 * the vsync the clock allows (at most one past its first possible one).
 */
final class ScheduledRelease {
    /** The display timeline as the renderer last saw it. */
    static final class Timing {
        boolean grid;
        long vsyncNs;
        long intervalNs;
        long deadlineNs;
    }

    interface Port {
        long nanoTime();

        /** Fills out with the current display timeline (one consistent snapshot). */
        void timing(Timing out);

        /** The codec timestamp of the output being held. */
        long heldPtsUs();

        /**
         * Drops the held output for a newer one that is already decoded, which becomes the held
         * output. Returns false, changing nothing, when there is none.
         */
        boolean replaceHeld(String reason, long basisNs);

        /** The clock's time for a frame when it cannot be planned onto the grid. */
        long fallbackPresentNs(long codecPtsUs, long basisNs);

        /** Hands the held output to the codec, due at releaseNs; dueNs is the vsync it should be shown at. */
        void release(long releaseNs, long dueNs, boolean planned, long holdNs, boolean late, int inFlightCount);
    }

    private final PresentScheduler scheduler;
    private final DecoderQueuePolicy.InFlight inFlight;
    private final Port port;
    private final PresentScheduler.Plan plan = new PresentScheduler.Plan();
    private final Timing timing = new Timing();

    ScheduledRelease(PresentScheduler scheduler, DecoderQueuePolicy.InFlight inFlight, Port port) {
        this.scheduler = scheduler;
        this.inFlight = inFlight;
        this.port = port;
    }

    /**
     * Releases the held output. A frame behind the game's cadence, or one that would be the third
     * in front of the compositor (capActive: render callbacks tell when frames are shown), is
     * replaced by a newer output when one is already decoded, unless the frame before it was
     * dropped (droppedBefore): never two in a row, and never the last frame. Returns the codec
     * timestamp of the frame that was released.
     */
    long run(boolean droppedBefore, long contentIntervalNs, boolean capActive) {
        boolean dropped = droppedBefore;
        boolean late;
        long basisNs;
        for (;;) {
            long readyNs = port.nanoTime();
            port.timing(timing);
            // The clock learns lateness against this time; it must be the same for every frame.
            basisNs = timing.grid ? readyNs + Math.max(0, timing.deadlineNs) : readyNs;
            late = scheduler.isLate(port.heldPtsUs(), basisNs, contentIntervalNs);
            if (PresentScheduler.dropLate(late, true, dropped) && replace("late", basisNs)) {
                dropped = true;
                continue;
            }
            if (capActive && inFlight.full(port.nanoTime())) {
                scheduler.onCapHeld();
                if (!dropped && replace("in_flight", basisNs)) {
                    dropped = true;
                    continue;
                }
            }
            break;
        }

        // Plan from the time and the vsync the frame is released at, not from what was read above:
        // a replacement attempt takes time, and a stale vsync could be in the past or taken.
        long readyNs = port.nanoTime();
        port.timing(timing);
        long ptsUs = port.heldPtsUs();
        long releaseNs, dueNs, holdNs;
        boolean planned = timing.grid && scheduler.plan(ptsUs, readyNs, timing.vsyncNs, timing.intervalNs,
                timing.deadlineNs, plan);
        if (planned) {
            releaseNs = plan.releaseNs;
            dueNs = plan.vsyncNs;
            holdNs = plan.holdNs;
        } else {
            releaseNs = dueNs = port.fallbackPresentNs(ptsUs, timing.grid ? basisNs : readyNs);
            holdNs = releaseNs - readyNs;
        }
        long nowNs = port.nanoTime();
        int inFlightCount = inFlight.count(nowNs);
        inFlight.onReleased(ptsUs, dueNs);
        port.release(releaseNs, dueNs, planned, holdNs, late, inFlightCount);
        return ptsUs;
    }

    private boolean replace(String reason, long basisNs) {
        long oldPtsUs = port.heldPtsUs();
        if (!port.replaceHeld(reason, basisNs)) return false;
        scheduler.discard(oldPtsUs, basisNs);
        if ("late".equals(reason)) scheduler.onLateDropped();
        else scheduler.onCapDropped();
        return true;
    }
}
