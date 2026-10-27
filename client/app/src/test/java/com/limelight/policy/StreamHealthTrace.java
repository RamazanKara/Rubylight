package com.limelight.policy;

import com.limelight.utils.StreamHealth;

/**
 * Drives StreamHealth from the shared traces, whose arguments are plain values: a sample's fields
 * as arguments and one report field per call. Rust's twin is the subject in health.rs.
 */
final class StreamHealthTrace {
    private final StreamHealth health = new StreamHealth();

    void add(long atMs, boolean fresh, int fps, float lossPercent, float jitterMs, float decodeMs,
             float hostP95Ms, float displayP95Ms, float receivedFps, float releasedFps, float shownFps,
             int thermal, boolean hostIncludesSend) {
        health.add(new StreamHealth.Sample(atMs, fresh, fps, lossPercent, jitterMs, decodeMs, hostP95Ms,
                displayP95Ms, receivedFps, releasedFps, shownFps, thermal, hostIncludesSend));
    }

    /** The same sample {@code count} times, {@code stepMs} apart, as steady stats ticks. */
    void addSeries(long firstMs, long count, long stepMs, boolean fresh, int fps, float lossPercent,
                   float jitterMs, float decodeMs, float hostP95Ms, float displayP95Ms, float receivedFps,
                   float releasedFps, float shownFps, int thermal, boolean hostIncludesSend) {
        for (long i = 0; i < count; i++) {
            add(firstMs + i * stepMs, fresh, fps, lossPercent, jitterMs, decodeMs, hostP95Ms, displayP95Ms,
                    receivedFps, releasedFps, shownFps, thermal, hostIncludesSend);
        }
    }

    long stallPeriodMs() { return health.stallPeriodMs(); }

    String reportFindings(long nowMs) {
        StringBuilder names = new StringBuilder();
        for (StreamHealth.Finding finding : health.report(nowMs).findings) {
            if (names.length() > 0) names.append(',');
            names.append(finding.name());
        }
        return names.toString();
    }

    int reportSamples(long nowMs) { return health.report(nowMs).samples; }
    boolean reportRecent(long nowMs) { return health.report(nowMs).recent; }
    float reportWorstLossPercent(long nowMs) { return health.report(nowMs).worstLossPercent; }
    float reportWorstDecodeMs(long nowMs) { return health.report(nowMs).worstDecodeMs; }
    float reportWorstHostP95Ms(long nowMs) { return health.report(nowMs).worstHostP95Ms; }
    float reportWorstDisplayP95Ms(long nowMs) { return health.report(nowMs).worstDisplayP95Ms; }
    int reportStalls(long nowMs) { return health.report(nowMs).stalls; }
    long reportStallPeriodMs(long nowMs) { return health.report(nowMs).stallPeriodMs; }
    boolean reportHostIncludesSend(long nowMs) { return health.report(nowMs).hostIncludesSend; }
    boolean reportCanSuggestBitrate(long nowMs) { return health.report(nowMs).canSuggestBitrate(); }
}
