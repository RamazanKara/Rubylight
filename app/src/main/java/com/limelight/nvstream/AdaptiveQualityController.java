package com.limelight.nvstream;

import java.util.ArrayList;
import java.util.List;

/** Bounded, opt-in resolution/FPS adaptation. Decisions do not perform I/O. */
public final class AdaptiveQualityController {
    public enum Mode {
        OFF, MOTION, DETAIL, BATTERY;
        public static Mode parse(String value) {
            if ("motion".equals(value)) return MOTION;
            if ("detail".equals(value)) return DETAIL;
            if ("battery".equals(value)) return BATTERY;
            return OFF;
        }
        public String preference() { return name().toLowerCase(java.util.Locale.ROOT); }
    }

    public static final class Target {
        public final int width, height, fps;
        private Target(int width, int height, int fps) {
            this.width = width; this.height = height; this.fps = fps;
        }
        public int[] request() { return new int[] {width, height, fps * 1000}; }
        public boolean matches(int width, int height, int fps) {
            return this.width == width && this.height == height && this.fps == fps;
        }
    }

    public static final long DOWN_INTERVAL_MS = 15000;
    public static final long RECOVER_MS = 30000;
    private final Mode mode;
    private final int minShortEdge, minFps, alignWidth, alignHeight;
    private final List<Target> levels = new ArrayList<>();
    private int level, batteryCeiling;
    private long lastChange, healthySince = -1;
    private int devicePressureSamples, networkPressureSamples;
    private int tailPressureSamples;
    private long timingGeneration = Long.MIN_VALUE, timingWindowEndNs, timingTickMs = -1;

    public AdaptiveQualityController(Mode mode, int width, int height, int fps, int minShortEdge,
                                     int minFps, int alignWidth, int alignHeight, long now) {
        this.mode = mode;
        this.minShortEdge = Math.max(240, minShortEdge);
        this.minFps = Math.max(15, minFps);
        this.alignWidth = Math.max(2, alignWidth);
        this.alignHeight = Math.max(2, alignHeight);
        rebuild(width, height, fps);
        lastChange = now;
    }

    private void rebuild(int width, int height, int fps) {
        if (width <= 0 || height <= 0 || fps <= 0) throw new IllegalArgumentException("Invalid stream size or FPS");
        levels.clear();
        int shortEdge = Math.min(width, height);
        int floor = Math.min(shortEdge, minShortEdge);
        List<int[]> sizes = new ArrayList<>();
        sizes.add(new int[] {width, height});
        for (int next = shortEdge; next > floor;) {
            next = Math.max(floor, next * 4 / 5);
            double scale = (double) next / shortEdge;
            int w = Math.min(width, Math.max(alignWidth, (int) Math.ceil(width * scale / alignWidth) * alignWidth));
            int h = Math.min(height, Math.max(alignHeight, (int) Math.ceil(height * scale / alignHeight) * alignHeight));
            int[] previous = sizes.get(sizes.size() - 1);
            if (w != previous[0] || h != previous[1]) sizes.add(new int[] {w, h});
        }
        List<Integer> rates = new ArrayList<>();
        rates.add(fps);
        int floorFps = Math.min(fps, minFps);
        for (int candidate : new int[] {120, 90, 60, 45, 30}) {
            if (candidate < fps && candidate > floorFps) rates.add(candidate);
        }
        if (rates.get(rates.size() - 1) != floorFps) rates.add(floorFps);
        int size = 0, rate = 0;
        levels.add(new Target(width, height, fps));
        while (size + 1 < sizes.size() || rate + 1 < rates.size()) {
            boolean lowerSize = mode == Mode.MOTION ? size + 1 < sizes.size() :
                    mode == Mode.DETAIL ? rate + 1 >= rates.size() :
                            size + 1 < sizes.size() && (rate + 1 >= rates.size() || (levels.size() % 2 == 0));
            if (lowerSize) size++;
            else rate++;
            levels.add(new Target(sizes.get(size)[0], sizes.get(size)[1], rates.get(rate)));
        }
        batteryCeiling = 0;
        if (mode == Mode.BATTERY) {
            // Battery mode deliberately caps work even on a healthy connection, within the user's floors.
            while (batteryCeiling + 1 < levels.size()) {
                Target target = levels.get(batteryCeiling);
                if (target.fps <= Math.max(60, floorFps) &&
                        Math.min(target.width, target.height) <= Math.max(1080, floor)) break;
                batteryCeiling++;
            }
        }
        level = Math.min(level, levels.size() - 1);
    }

    public Target ceiling() { return levels.get(0); }
    public Target current() { return levels.get(level); }

    /** A fold/window change keeps the current degradation level and the chosen user floors. */
    public Target resizeCeiling(int width, int height, int fps, long now) {
        if (ceiling().matches(width, height, fps)) return null;
        rebuild(width, height, fps);
        suspend();
        lastChange = now;
        return current();
    }

    public Target sample(long now, boolean recentVideo, boolean poorConnection, float loss,
                         float decodeMs, float jitterMs, float queueMs, int thermalLevel,
                         boolean bitrateHasRoom) {
        return sample(now, recentVideo, poorConnection, loss, decodeMs, jitterMs, queueMs,
                thermalLevel, bitrateHasRoom, null, false);
    }

    /** A missing timing observation is unknown, never evidence that the decoder recovered. */
    public Target sample(long now, boolean recentVideo, boolean poorConnection, float loss,
                         float decodeMs, float jitterMs, float queueMs, int thermalLevel,
                         boolean bitrateHasRoom, DecodeTiming timing) {
        return sample(now, recentVideo, poorConnection, loss, decodeMs, jitterMs, queueMs,
                thermalLevel, bitrateHasRoom, timing, true);
    }

    private Target sample(long now, boolean recentVideo, boolean poorConnection, float loss,
                          float decodeMs, float jitterMs, float queueMs, int thermalLevel,
                          boolean bitrateHasRoom, DecodeTiming timing, boolean observeTail) {
        if (mode == Mode.OFF || !recentVideo) { suspend(); return null; }
        float frameMs = 1000f / current().fps;
        double decodeBudgetMs = DecodeTiming.decodeBudgetMs(current().fps);
        boolean validTiming = observeTail && acceptTiming(now, timing);
        // A single keyframe cannot satisfy both the count and ratio. Four fresh intervals
        // must agree, so even a whole brief burst cannot be reused to force a reduction.
        boolean tailPressure = validTiming && timing.lateFrames >= 3 &&
                timing.lateFrames * 10L >= timing.samples;
        tailPressureSamples = tailPressure ? tailPressureSamples + 1 : 0;
        boolean devicePressure = thermalLevel >= 2 || decodeMs >= decodeBudgetMs;
        boolean networkPressure = poorConnection || loss >= 3 || queueMs >= Math.max(20, frameMs * 2);
        devicePressureSamples = devicePressure ? devicePressureSamples + 1 : 0;
        networkPressureSamples = networkPressure && !bitrateHasRoom ? networkPressureSamples + 1 : 0;
        boolean batteryStep = level < batteryCeiling;
        if (devicePressure || tailPressure || networkPressure || batteryStep) {
            healthySince = -1;
            if (now - lastChange >= DOWN_INTERVAL_MS && level + 1 < levels.size() &&
                    (devicePressureSamples >= 4 || tailPressureSamples >= 4 ||
                            networkPressureSamples >= 8 || batteryStep)) {
                return levels.get(level + 1);
            }
            return null;
        }
        boolean healthy = (!observeTail || (validTiming && timing.lateFrames * 50L <= timing.samples)) &&
                thermalLevel == 0 && loss >= 0 && loss < 0.5f &&
                decodeMs >= 0 && decodeMs < decodeBudgetMs * 0.75 && jitterMs >= 0 &&
                jitterMs < Math.max(5, frameMs * 0.25f) && (queueMs < 0 || queueMs < Math.max(4, frameMs / 2));
        if (!healthy) healthySince = -1;
        else if (healthySince < 0) healthySince = now;
        else if (level > batteryCeiling && now - healthySince >= RECOVER_MS && now - lastChange >= RECOVER_MS) {
            return levels.get(level - 1);
        }
        return null;
    }

    private boolean acceptTiming(long now, DecodeTiming timing) {
        if (timingTickMs >= 0 && (now < timingTickMs || now - timingTickMs > 2000)) {
            tailPressureSamples = devicePressureSamples = networkPressureSamples = 0;
            healthySince = -1;
        }
        timingTickMs = now;
        if (timing == null) return false;
        if (timing.generation != timingGeneration) {
            timingGeneration = timing.generation;
            timingWindowEndNs = 0;
            tailPressureSamples = devicePressureSamples = networkPressureSamples = 0;
            healthySince = -1;
        }
        boolean newWindow = timing.windowEndNs > timingWindowEndNs &&
                timing.windowStartNs >= timingWindowEndNs;
        timingWindowEndNs = Math.max(timingWindowEndNs, timing.windowEndNs);
        // A quiet host may send fewer frames than the negotiated rate. Require enough fresh
        // outputs spread over half a second, so pauses and short bursts still cannot recover.
        int minimumSamples = 12;
        return newWindow && timing.fps == current().fps && timing.windowStartNs >= 0 &&
                timing.windowEndNs - timing.windowStartNs <= 2_000_000_000L &&
                timing.samples >= minimumSamples && timing.lateFrames >= 0 &&
                timing.lateFrames <= timing.samples && timing.firstSampleNs > timing.windowStartNs &&
                timing.lastSampleNs <= timing.windowEndNs &&
                timing.lastSampleNs - timing.firstSampleNs >= 500_000_000L &&
                timing.windowEndNs - timing.lastSampleNs <= 500_000_000L;
    }

    /** Called only after the request was sent successfully; absent requests cannot advance the ladder. */
    public void applied(Target target, long now) {
        int index = levels.indexOf(target);
        if (index < 0) throw new IllegalArgumentException("Target belongs to a different ceiling");
        level = index;
        lastChange = now;
        suspend();
    }

    public void suspend() {
        healthySince = -1;
        devicePressureSamples = networkPressureSamples = tailPressureSamples = 0;
        timingTickMs = -1;
    }
}
