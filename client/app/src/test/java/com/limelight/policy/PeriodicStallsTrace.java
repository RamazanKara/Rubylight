package com.limelight.policy;

import com.limelight.utils.PeriodicStalls;

/** Drives PeriodicStalls from the shared traces, which pass stall times as one space-separated string. */
final class PeriodicStallsTrace {
    private PeriodicStallsTrace() { }

    static long periodMs(String times, int count) {
        String trimmed = times.trim();
        String[] parts = trimmed.isEmpty() ? new String[0] : trimmed.split("\\s+");
        long[] values = new long[parts.length];
        for (int i = 0; i < parts.length; i++) values[i] = Long.parseLong(parts[i]);
        if (count > values.length) throw new IllegalArgumentException("count " + count + " exceeds " + values.length + " times");
        return PeriodicStalls.periodMs(values, count);
    }
}
