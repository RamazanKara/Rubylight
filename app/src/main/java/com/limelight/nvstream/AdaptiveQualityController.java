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
        if (mode == Mode.OFF || !recentVideo) { suspend(); return null; }
        float frameMs = 1000f / current().fps;
        boolean devicePressure = thermalLevel >= 2 || decodeMs >= frameMs;
        boolean networkPressure = poorConnection || loss >= 3 || queueMs >= Math.max(20, frameMs * 2);
        devicePressureSamples = devicePressure ? devicePressureSamples + 1 : 0;
        networkPressureSamples = networkPressure && !bitrateHasRoom ? networkPressureSamples + 1 : 0;
        boolean batteryStep = level < batteryCeiling;
        if (devicePressure || networkPressure || batteryStep) {
            healthySince = -1;
            if (now - lastChange >= DOWN_INTERVAL_MS && level + 1 < levels.size() &&
                    (devicePressureSamples >= 4 || networkPressureSamples >= 8 || batteryStep)) {
                return levels.get(level + 1);
            }
            return null;
        }
        boolean healthy = thermalLevel == 0 && loss >= 0 && loss < 0.5f &&
                decodeMs >= 0 && decodeMs < frameMs * 0.65f && jitterMs >= 0 &&
                jitterMs < Math.max(5, frameMs * 0.25f) && (queueMs < 0 || queueMs < Math.max(4, frameMs / 2));
        if (!healthy) healthySince = -1;
        else if (healthySince < 0) healthySince = now;
        else if (level > batteryCeiling && now - healthySince >= RECOVER_MS && now - lastChange >= RECOVER_MS) {
            return levels.get(level - 1);
        }
        return null;
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
        devicePressureSamples = networkPressureSamples = 0;
    }
}
