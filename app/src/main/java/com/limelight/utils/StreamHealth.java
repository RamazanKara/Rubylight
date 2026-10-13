package com.limelight.utils;

import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;

/** Bounded, identifier-free observations. Findings describe evidence, not proven root causes. */
public final class StreamHealth {
    public static final long HISTORY_MS = 30_000;
    private static final int MAX_SAMPLES = 30;
    public enum Finding { NETWORK, DECODER, HOST, PRESENTATION, HEAT }

    public static final class Sample {
        public final long atMs;
        public final boolean fresh;
        public final float budgetMs, lossPercent, jitterMs, decodeMs, hostP95Ms, displayP95Ms;
        public final float receivedFps, releasedFps, shownFps;
        public final int thermal;
        public Sample(long atMs, boolean fresh, int fps, float lossPercent, float jitterMs,
                      float decodeMs, float hostP95Ms, float displayP95Ms,
                      float receivedFps, float releasedFps, float shownFps, int thermal) {
            this.atMs = atMs;
            this.fresh = fresh;
            this.budgetMs = fps > 0 ? 1000f / fps : Float.NaN;
            this.lossPercent = lossPercent;
            this.jitterMs = jitterMs;
            this.decodeMs = decodeMs;
            this.hostP95Ms = hostP95Ms;
            this.displayP95Ms = displayP95Ms;
            this.receivedFps = receivedFps;
            this.releasedFps = releasedFps;
            this.shownFps = shownFps;
            this.thermal = thermal;
        }
    }

    public static final class Report {
        public final List<Finding> findings;
        public final int samples;
        public final boolean recent;
        public final float worstLossPercent, worstDecodeMs, worstHostP95Ms, worstDisplayP95Ms;
        private Report(List<Finding> findings, int samples, boolean recent, float loss,
                       float decode, float host, float display) {
            this.findings = Collections.unmodifiableList(findings);
            this.samples = samples;
            this.recent = recent;
            worstLossPercent = loss; worstDecodeMs = decode;
            worstHostP95Ms = host; worstDisplayP95Ms = display;
        }
        public boolean canSuggestBitrate() {
            return recent && findings.contains(Finding.NETWORK);
        }
    }

    private final ArrayDeque<Sample> samples = new ArrayDeque<>();

    public synchronized void add(Sample sample) {
        if (!samples.isEmpty() && sample.atMs - samples.getLast().atMs < 900) return;
        expire(sample.atMs);
        while (samples.size() >= MAX_SAMPLES) samples.removeFirst();
        samples.addLast(sample);
    }

    private void expire(long nowMs) {
        while (!samples.isEmpty() && nowMs - samples.getFirst().atMs > HISTORY_MS) samples.removeFirst();
    }

    public synchronized Report report(long nowMs) {
        expire(nowMs);
        int[] votes = new int[Finding.values().length];
        int valid = 0;
        float loss = -1, decode = -1, host = -1, display = -1;
        for (Sample s : samples) {
            if (!s.fresh || !Float.isFinite(s.budgetMs)) continue;
            valid++;
            if (valid(s.lossPercent)) loss = Math.max(loss, s.lossPercent);
            if (valid(s.decodeMs)) decode = Math.max(decode, s.decodeMs);
            if (valid(s.hostP95Ms)) host = Math.max(host, s.hostP95Ms);
            if (valid(s.displayP95Ms)) display = Math.max(display, s.displayP95Ms);
            if ((valid(s.lossPercent) && s.lossPercent >= 2) ||
                    (valid(s.jitterMs) && s.jitterMs >= Math.max(8, s.budgetMs))) votes[Finding.NETWORK.ordinal()]++;
            if (valid(s.decodeMs) && s.decodeMs >= s.budgetMs) votes[Finding.DECODER.ordinal()]++;
            if (valid(s.hostP95Ms) && s.hostP95Ms >= s.budgetMs * 1.25f) votes[Finding.HOST.ordinal()]++;
            // Compare observed stages, not requested FPS: a static/VRR desktop is not a dropped-frame fault.
            if (valid(s.shownFps) && valid(s.releasedFps) && valid(s.displayP95Ms) && s.releasedFps >= 10 &&
                    s.shownFps < s.releasedFps * .85f && s.displayP95Ms > s.budgetMs * 2) {
                votes[Finding.PRESENTATION.ordinal()]++;
            }
            if (s.thermal >= 2) votes[Finding.HEAT.ordinal()]++;
        }
        List<Finding> findings = new ArrayList<>();
        if (valid >= 3) for (Finding finding : Finding.values()) {
            if (votes[finding.ordinal()] >= 2) findings.add(finding);
        }
        boolean recent = !samples.isEmpty() && samples.getLast().fresh && nowMs - samples.getLast().atMs <= 2500;
        return new Report(findings, valid, recent, loss, decode, host, display);
    }

    private static boolean valid(float value) { return Float.isFinite(value) && value >= 0; }
}
