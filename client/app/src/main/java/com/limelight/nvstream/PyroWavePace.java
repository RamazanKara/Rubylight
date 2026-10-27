package com.limelight.nvstream;

import java.util.ArrayList;
import java.util.List;
import java.util.function.BooleanSupplier;

/**
 * Frame-pace calibration for PyroWave hosts with frame-shaped UDP probes (Vibeshine's
 * PyroWaveUdpProbeBurstVersion 1). The host sends a probe's packets as video frames, each frame's
 * packets back-to-back in 1 ms groups at a given pace. The fastest pace this device receives
 * without losing frame tails is sent to the host as x-ss-video[0].pyrowavePaceMbps, and the host
 * paces its frame bursts at it instead of at the raw link speed.
 *
 * The limits and the search follow Nonary's Moonlight (pyrowavelinkpolicy.h) so both clients
 * calibrate the same way.
 */
public final class PyroWavePace {
    public static final int MIN_PROBE_KBPS = 5000;
    public static final int MAX_PROBE_KBPS = 3000000;
    public static final int MAX_PACE_KBPS = 9999999;
    public static final int DURATION_MS = 2000;
    static final int WINDOW_MS = 100;
    static final int WINDOW_COUNT = DURATION_MS / WINDOW_MS;
    /** Packet, RTP, encryption, UDP, IPv6 and Ethernet bytes the host charges per probe packet. */
    public static final int WIRE_OVERHEAD = 134;
    /** Probe packets carry the token, the sequence number and padding beyond the stream packet size. */
    public static final int PROBE_EXTRA_BYTES = 48;

    static final double LOSS_LIMIT_PERCENT = 2.0;
    static final double WINDOW_LOSS_LIMIT_PERCENT = 5.0;
    static final double MINIMUM_SENT_SHARE = 0.98;
    static final double FRAME_LOSS_LIMIT_PERCENT = 10.0;
    static final int PACE_STEP_KBPS = 50000;
    static final double PACE_TAIL_FRAME_LIMIT_PERCENT = 2.0;

    // Audio, control and their FEC alongside the video (8 high-quality channels), as the host counts it.
    static final double AUDIO_CONTROL_KBPS = 3572;
    static final int FRAME_HEADER = 8;

    private PyroWavePace() {
    }

    /** One probe's outcome. */
    public static final class Result {
        public long expected;
        public long sent;
        public long received;
        public double senderMs;
        public double lossPercent = 100;
        public double worstWindowLossPercent = 100;
        public long frames;
        public long damagedFrames;
        public long excessiveLossFrames;
        public long missingTailFrames;
        public double worstFrameLossPercent = 100;

        public double missingTailFramePercent() {
            return frames != 0 ? 100.0 * missingTailFrames / frames : 100;
        }

        public double excessiveLossFramePercent() {
            return frames != 0 ? 100.0 * excessiveLossFrames / frames : 100;
        }

        boolean hostSentEnough() {
            return expected > 0 && sent <= expected && sent >= expected * MINIMUM_SENT_SHARE;
        }

        @Override
        public String toString() {
            return String.format(java.util.Locale.ROOT,
                    "sent %d/%d, received %d, loss %.3f%%, worst 100 ms %.3f%%, damaged frames %d/%d, " +
                            "frames losing over 10%% %.3f%%, missing tails %.3f%%, worst frame %.3f%%, host %.1f ms",
                    sent, expected, received, lossPercent, worstWindowLossPercent, damagedFrames, frames,
                    excessiveLossFramePercent(), missingTailFramePercent(), worstFrameLossPercent, senderMs);
        }
    }

    /** Packets the host sends in a probe of this rate. */
    public static long expectedPackets(int kbps, int packetSize) {
        return (long) kbps * DURATION_MS / (8L * (packetSize + WIRE_OVERHEAD));
    }

    /** Packets per 1 ms send group at this pace, with the host's UDP, IP and Ethernet charge. */
    public static long burstGroupPackets(int paceKbps, int packetSize, boolean ipv6) {
        int wireBytes = packetSize + PROBE_EXTRA_BYTES + 8 + (ipv6 ? 40 : 20) + 38;
        return Math.max(1, paceKbps / 8 / wireBytes);
    }

    /**
     * Grades a probe from which sequence numbers arrived. Frames are the host's sequence partitions,
     * so reordering never moves a packet to another frame, and an absent frame counts in full.
     */
    public static void summarize(Result result, boolean[] arrived, int fps, long groupPackets) {
        result.frames = result.damagedFrames = result.excessiveLossFrames = result.missingTailFrames = 0;
        result.worstFrameLossPercent = 100;
        long expected = result.expected;
        if (fps > 0 && expected > 0) {
            long frames = Math.max(1, (long) fps * DURATION_MS / 1000);
            long group = Math.max(1, groupPackets);
            result.worstFrameLossPercent = 0;
            for (long frame = 0; frame < frames; frame++) {
                long begin = expected * frame / frames;
                long end = expected * (frame + 1) / frames;
                if (begin == end) continue;
                // The tail is the frame's final 1 ms send group.
                long tailBegin = begin + (end - begin - 1) / group * group;
                long missing = 0;
                boolean missingTail = false;
                for (long seq = begin; seq < end; seq++) {
                    if (seq >= arrived.length || !arrived[(int) seq]) {
                        missing++;
                        missingTail |= seq >= tailBegin;
                    }
                }
                double loss = 100.0 * missing / (end - begin);
                result.frames++;
                if (missing != 0) result.damagedFrames++;
                if (loss > FRAME_LOSS_LIMIT_PERCENT) result.excessiveLossFrames++;
                if (missingTail) result.missingTailFrames++;
                result.worstFrameLossPercent = Math.max(result.worstFrameLossPercent, loss);
            }
        }
        result.received = 0;
        result.worstWindowLossPercent = 0;
        for (int window = 0; window < WINDOW_COUNT; window++) {
            long begin = expected * window / WINDOW_COUNT;
            long end = expected * (window + 1) / WINDOW_COUNT;
            long count = 0;
            for (long seq = begin; seq < end && seq < arrived.length; seq++) {
                if (arrived[(int) seq]) count++;
            }
            result.received += count;
            if (end > begin) {
                result.worstWindowLossPercent = Math.max(result.worstWindowLossPercent,
                        100.0 * (end - begin - count) / (end - begin));
            }
        }
        result.lossPercent = expected != 0 ? 100.0 * (expected - result.received) / expected : 100;
    }

    /** A complete frame-shaped measurement. Only loss and the host's send duration grade it. */
    public static boolean measured(Result r) {
        return r.hostSentEnough() && r.received > 0 && r.received <= r.sent &&
                Double.isFinite(r.senderMs) && Double.isFinite(r.lossPercent) &&
                Double.isFinite(r.worstWindowLossPercent) &&
                r.senderMs >= DURATION_MS * 0.98 && r.senderMs <= DURATION_MS * 1.02 &&
                r.frames > 0 && Double.isFinite(r.worstFrameLossPercent);
    }

    public static boolean qualified(Result r) {
        return measured(r) && r.lossPercent < LOSS_LIMIT_PERCENT &&
                r.worstWindowLossPercent < WINDOW_LOSS_LIMIT_PERCENT &&
                r.excessiveLossFrames == 0 && r.worstFrameLossPercent <= FRAME_LOSS_LIMIT_PERCENT &&
                r.missingTailFramePercent() < PACE_TAIL_FRAME_LIMIT_PERCENT;
    }

    private static int violations(Result r) {
        return (r.lossPercent >= LOSS_LIMIT_PERCENT ? 1 : 0) +
                (r.worstWindowLossPercent >= WINDOW_LOSS_LIMIT_PERCENT ? 1 : 0) +
                (r.excessiveLossFrames != 0 || r.worstFrameLossPercent > FRAME_LOSS_LIMIT_PERCENT ? 1 : 0) +
                (r.missingTailFramePercent() >= PACE_TAIL_FRAME_LIMIT_PERCENT ? 1 : 0);
    }

    /** Fewer violated limits, then less tail loss, severe frame damage, worst frame damage and total loss. */
    static boolean better(Result a, Result b) {
        if (violations(a) != violations(b)) return violations(a) < violations(b);
        if (a.missingTailFramePercent() != b.missingTailFramePercent())
            return a.missingTailFramePercent() < b.missingTailFramePercent();
        if (a.excessiveLossFramePercent() != b.excessiveLossFramePercent())
            return a.excessiveLossFramePercent() < b.excessiveLossFramePercent();
        if (a.worstFrameLossPercent != b.worstFrameLossPercent)
            return a.worstFrameLossPercent < b.worstFrameLossPercent;
        if (a.lossPercent != b.lossPercent) return a.lossPercent < b.lossPercent;
        return a.worstWindowLossPercent < b.worstWindowLossPercent;
    }

    public interface Probe {
        Result probe(int paceKbps) throws Exception;
    }

    public static final class Outcome {
        /** Zero: not determined, the host keeps its own default. */
        public final int paceKbps;
        /** Both fresh confirmations passed every loss limit. */
        public final boolean qualified;
        public final Result probe;

        Outcome(int paceKbps, boolean qualified, Result probe) {
            this.paceKbps = paceKbps;
            this.qualified = qualified;
            this.probe = probe;
        }

        static final Outcome NONE = new Outcome(0, false, null);
    }

    private static final class Measurement {
        final int paceKbps;
        Result probe;

        Measurement(int paceKbps, Result probe) {
            this.paceKbps = paceKbps;
            this.probe = probe;
        }
    }

    private static int stepDown(double kbps) {
        return (int) (kbps / PACE_STEP_KBPS) * PACE_STEP_KBPS;
    }

    /**
     * Highest pace between floorKbps (a frame must still fit its interval) and the link. The link is
     * tried first. Below it, a bisected pass leaves a 5% margin that must pass twice afresh, stepping
     * down on failure; then every remaining 50 Mbps step, since loss need not be monotonic. Only then
     * the best unqualified measurement, measured twice more at that pace.
     */
    public static Outcome search(int linkKbps, int floorKbps, Probe probe, BooleanSupplier cancelled) throws Exception {
        int cap = stepDown(linkKbps);
        if (cap < PACE_STEP_KBPS) return Outcome.NONE;
        int floor = Math.max(PACE_STEP_KBPS, Math.min(cap,
                (int) Math.ceil((double) floorKbps / PACE_STEP_KBPS) * PACE_STEP_KBPS));
        List<Measurement> measurements = new ArrayList<>();
        Searcher s = new Searcher(probe, measurements, cancelled);

        if (s.passes(cap)) {
            if (cancelled.getAsBoolean()) return Outcome.NONE;
            Outcome atLink = s.confirmed(cap);
            if (atLink.qualified) return atLink;
        }
        if (cancelled.getAsBoolean()) return Outcome.NONE;
        int low = 0, high = cap;
        if (floor < cap) {
            if (s.passes(floor)) low = floor;
            for (int attempt = 0; low != 0 && attempt < 16 && high - low > PACE_STEP_KBPS &&
                    !cancelled.getAsBoolean(); attempt++) {
                int next = Math.max(low + PACE_STEP_KBPS, stepDown((low + high) / 2.0));
                if (next >= high) break;
                if (s.passes(next)) low = next;
                else high = next;
            }
        }
        if (cancelled.getAsBoolean()) return Outcome.NONE;
        for (int pace = Math.max(floor, stepDown(low * 0.95)); low != 0 && !cancelled.getAsBoolean();
             pace = Math.max(floor, stepDown(pace * 0.9))) {
            Outcome result = s.confirmed(pace);
            if (result.qualified) return result;
            if (pace == floor) break;
        }
        // A failed floor says nothing about the speeds between it and the link.
        for (int pace = cap - PACE_STEP_KBPS; pace >= floor && !cancelled.getAsBoolean(); pace -= PACE_STEP_KBPS) {
            Measurement previous = s.find(pace);
            if (previous != null && !qualified(previous.probe)) continue;
            if (previous != null || s.passes(pace)) {
                Outcome result = s.confirmed(pace);
                if (result.qualified) return result;
            }
        }
        if (cancelled.getAsBoolean()) return Outcome.NONE;
        Measurement best = null;
        for (Measurement m : measurements) {
            if (measured(m.probe) && (best == null || better(m.probe, best.probe) ||
                    (!better(best.probe, m.probe) && m.paceKbps > best.paceKbps))) {
                best = m;
            }
        }
        if (best == null) return Outcome.NONE;
        Result bestProbe = best.probe;
        boolean passed = true;
        for (int run = 0; run < 2; run++) {
            if (cancelled.getAsBoolean()) return Outcome.NONE;
            Result result = s.measure(best.paceKbps);
            // Never publish a fallback whose final measurements are invalid.
            if (!measured(result)) return Outcome.NONE;
            passed &= qualified(result);
            if (run == 0 || better(bestProbe, result)) bestProbe = result;
        }
        return cancelled.getAsBoolean() ? Outcome.NONE : new Outcome(best.paceKbps, passed, bestProbe);
    }

    private static final class Searcher {
        private final Probe probe;
        private final List<Measurement> measurements;
        private final BooleanSupplier cancelled;

        Searcher(Probe probe, List<Measurement> measurements, BooleanSupplier cancelled) {
            this.probe = probe;
            this.measurements = measurements;
            this.cancelled = cancelled;
        }

        Measurement find(int pace) {
            for (Measurement m : measurements) {
                if (m.paceKbps == pace) return m;
            }
            return null;
        }

        Result measure(int pace) throws Exception {
            Result result = probe.probe(pace);
            Measurement previous = find(pace);
            // Keep the worse observation at each pace so a lucky first run never wins the fallback.
            if (previous == null) {
                measurements.add(new Measurement(pace, result));
            } else if (!measured(result) || (measured(previous.probe) && better(previous.probe, result))) {
                previous.probe = result;
            }
            return result;
        }

        boolean passes(int pace) throws Exception {
            return qualified(measure(pace));
        }

        Outcome confirmed(int pace) throws Exception {
            Result last = null;
            for (int run = 0; run < 2; run++) {
                if (cancelled.getAsBoolean()) return Outcome.NONE;
                last = measure(pace);
                if (!qualified(last)) return Outcome.NONE;
            }
            return new Outcome(pace, true, last);
        }
    }

    // The host's wire budget (pyrowave_bandwidth.h), to size probes like the configured stream.

    private static long maxCriticalParity(int criticalFecPercent, int minParityShards, long packets) {
        long result = 0;
        if (criticalFecPercent <= 0) return 0;
        for (long critical = 1; critical <= Math.min(packets, 255); critical++) {
            long parity = Math.max((critical * criticalFecPercent + 99) / 100, minParityShards);
            if (critical + parity <= 255) result = Math.max(result, parity);
        }
        return result;
    }

    /** Wire kbps a stream of this image bitrate occupies, with FEC, headers, audio and control. */
    public static int wireKbps(int imageKbps, int fps, int packetSize, int criticalFecPercent, int minParityShards) {
        if (fps <= 0 || imageKbps <= 0 || packetSize <= 16) return 0;
        long imageBytes = (long) Math.ceil(imageKbps * 125.0 / fps);
        long payload = packetSize - 16;
        long data = (imageBytes + FRAME_HEADER + payload - 1) / payload;
        long wireBytes = (data + maxCriticalParity(criticalFecPercent, minParityShards, 255)) * (packetSize + WIRE_OVERHEAD);
        return (int) Math.ceil(wireBytes * fps / 125.0 + AUDIO_CONTROL_KBPS);
    }

    /** The probe rate for a stream: its wire rate, inside what the host accepts. */
    public static int probeKbps(int imageKbps, int fps, int packetSize, int criticalFecPercent, int minParityShards) {
        int wire = wireKbps(imageKbps, fps, packetSize, criticalFecPercent, minParityShards);
        return Math.max(MIN_PROBE_KBPS, Math.min(MAX_PROBE_KBPS, wire));
    }

    /** Nonary's floor: frames must still leave at 125% of the average rate they carry. */
    public static int floorKbps(int probeKbps) {
        return probeKbps / 4 * 5;
    }
}
