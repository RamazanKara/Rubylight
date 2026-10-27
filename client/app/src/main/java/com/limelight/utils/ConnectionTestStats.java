package com.limelight.utils;

public final class ConnectionTestStats {
    private int attempts, received, differences;
    private double totalMs, totalDifferenceMs;
    private double previousMs = Double.NaN;

    public void add(double milliseconds) {
        attempts++;
        if (!Double.isFinite(milliseconds) || milliseconds < 0) {
            previousMs = Double.NaN;
            return;
        }
        received++;
        totalMs += milliseconds;
        if (!Double.isNaN(previousMs)) {
            totalDifferenceMs += Math.abs(milliseconds - previousMs);
            differences++;
        }
        previousMs = milliseconds;
    }

    public double latencyMs() {
        return received == 0 ? Double.NaN : totalMs / received;
    }

    public double jitterMs() {
        return differences == 0 ? Double.NaN : totalDifferenceMs / differences;
    }

    public double lossPercent() {
        return attempts == 0 ? Double.NaN : (attempts - received) * 100.0 / attempts;
    }
}
