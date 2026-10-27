package com.limelight.utils;

import java.util.Arrays;

public final class LatencyBenchmarkStats {
    public static final int FRAME_INTERVAL = 0;
    public static final int CALLBACK_DELAY = 1;
    public static final int INPUT_AGE = 2;
    public static final int TIMER_DELAY = 3;
    public static final int WINDOW_SIZE = 2048;
    private final long[][] samples = new long[4][WINDOW_SIZE];
    private final int[] counts = new int[4];
    private final int[] positions = new int[4];
    private long lastFrameNs = -1;

    public void frame(long frameNs, long callbackNs) {
        if (frameNs < 0 || callbackNs < frameNs || frameNs <= lastFrameNs) return;
        if (lastFrameNs >= 0) add(FRAME_INTERVAL, frameNs - lastFrameNs);
        lastFrameNs = frameNs;
        add(CALLBACK_DELAY, callbackNs - frameNs);
    }

    public void add(int metric, long durationNs) {
        if (durationNs < 0) return;
        samples[metric][positions[metric]] = durationNs;
        positions[metric] = (positions[metric] + 1) % WINDOW_SIZE;
        counts[metric] = Math.min(WINDOW_SIZE, counts[metric] + 1);
    }

    public double[] summary(int metric) {
        long[] values = Arrays.copyOf(samples[metric], counts[metric]);
        if (values.length == 0) return new double[] {0, -1, -1, -1};
        Arrays.sort(values);
        double sum = 0;
        for (long value : values) sum += value;
        return new double[] {values.length, sum / values.length / 1_000_000,
                values[(int) Math.ceil(values.length * 0.95) - 1] / 1_000_000.0,
                values[(int) Math.ceil(values.length * 0.99) - 1] / 1_000_000.0};
    }
}
