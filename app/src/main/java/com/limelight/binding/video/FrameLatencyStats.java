package com.limelight.binding.video;

import com.limelight.nvstream.av.video.HostTimingReport;
import com.limelight.nvstream.DecodeTiming;

import java.io.IOException;
import java.io.Writer;
import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.Iterator;
import java.util.Arrays;
import java.util.HashMap;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;

class FrameLatencyStats {
    static final int WINDOW_SIZE = 600;
    // Cover the five-second callback timeout at the maximum 1000 FPS stream rate.
    private static final int MAX_PENDING = 8192;
    private static final int MAX_COMPLETED = 4096;
    private static final long CALLBACK_TIMEOUT_NS = 5000000000L;
    private static final long FPS_WINDOW_NS = 2000000000L;
    // Sample stages: 0 queue wait, 1 decode, 2 display wait, 3 client latency, 4 host processing
    // (the video frame header's value, the one Artemis and Moonlight show), 5 to 7 the host's queue,
    // encode and send split and 8 its present to last packet sent (only from host timing reports).
    private static final int STAGES = 9;
    private static final int HOST_STAGE = 4;
    static final int HOST_QUEUE_STAGE = 5;
    static final int HOST_SEND_STAGE = 7;
    static final int HOST_SENT_STAGE = 8;
    // Reports that arrive before their frame, and frames finished before their report, are matched
    // within these bounds; a report older than that is dropped.
    private static final int MAX_EARLY_REPORTS = 256;
    private static final int MAX_COMPLETED_SCAN = 512;
    // Queued frames Artemis never counts (replaced before display) are forgotten past this many.
    private static final int MAX_ARTEMIS_QUEUED = 1024;
    static final String CSV_HEADER = "frame_number,pts_us,receive_ns,decoder_input_ns,decoder_output_ns," +
            "render_ns,release_ns,status,receive_to_input_ms,input_to_output_ms,output_to_render_ms," +
            "receive_to_render_ms,csv_rows_lost,host_processing_ms," +
            "decode_to_present_ms,receive_to_present_ms," +
            "decode_to_release_ms,decode_interval_ms,release_interval_ms," +
            "fps_sample_ns,received_fps,released_fps,shown_fps,present_drops,enqueue_ns,queue_wait_ms," +
            "host_present_to_sent_ms,host_queue_ms,host_encode_ms,host_send_ms," +
            "callback_received_ns,callback_delay_ms," +
            // Nine columns for joining the row with the host's record (Rubylight's
            // docs/timing-record.md): host microseconds, empty where unknown.
            "rtp_host_us,host_present_us,host_acquired_us,host_submitted_us,host_encoded_us," +
            "host_first_sent_us,host_last_sent_us,clock_offset_us,clock_rtt_us," +
            // The present scheduler and the decoder queue (empty where the frame was not planned):
            // the vsync it was planned for, the wait before its release, whether it was behind the
            // game's cadence, frames behind it in the decoder, its input-to-output time above the
            // ten second minimum, frames in front of the compositor, and why it was dropped.
            "sched_vsync_ns,sched_hold_ms,sched_late,queue_depth,residency_excess_ms,in_flight," +
            "drop_reason\n";

    /** Reads the host-to-client clock offset and round trip (microseconds) into out[0..1]; false before an exchange. */
    interface ClockOffsetSource {
        boolean read(long[] out);
    }

    private final LinkedHashMap<Long, Frame> pending = new LinkedHashMap<>();
    private final HashMap<Integer, Frame> outputs = new HashMap<>();
    private final ArrayDeque<Frame> completed = new ArrayDeque<>();
    private final long[][] samples = new long[STAGES][WINDOW_SIZE];
    private final long[][] sampleTimes = new long[STAGES][WINDOW_SIZE];
    private final int[] counts = new int[STAGES];
    private final int[] positions = new int[STAGES];
    private final LinkedHashMap<Integer, HostTimingReport.Record> earlyReports = new LinkedHashMap<>();
    private long csvRowsLost;
    private final boolean vrr;
    private long lastOutputNs;
    private long lastReleaseNs;
    private long decodeTimeTotalNs;
    private int decodeTimeSamples;
    private long totalDecodeTimeNs;
    private long totalDecodedFrames;
    private long decodeTimingConsumedFrames, decodeTimingPollNs, decodeTimingGeneration;
    private long totalClientTimeNs;
    private long totalClientTimeSamples;
    private final long[][] frameTimes = new long[3][2048];
    private final int[] frameTimePositions = new int[3];
    private long fpsStartNs;
    private boolean renderCallbacksAvailable;
    private long presentDrops;
    // The decode time the app shows is counted the way Artemis counts it, so the two apps show the
    // same figure for the same decoder work: each frame Artemis would count is timed from
    // queueInputBuffer() returning to the decoder returning it, rounded down to whole milliseconds, and the
    // sum is later divided by every frame received. The precise timings above stay as they are
    // for adaptive bitrate and quality, the stream doctor, the stage breakdown and the CSV.
    private final LinkedHashMap<Long, Long> artemisQueuedNs = new LinkedHashMap<Long, Long>() {
        @Override
        protected boolean removeEldestEntry(Map.Entry<Long, Long> eldest) {
            return size() > MAX_ARTEMIS_QUEUED;
        }
    };
    // When the decoder returned each of those frames from dequeueOutputBuffer(). Kept apart from
    // Frame because the render callback can finish a frame before the renderer thread counts it.
    private final LinkedHashMap<Long, Long> artemisOutputNs = new LinkedHashMap<Long, Long>() {
        @Override
        protected boolean removeEldestEntry(Map.Entry<Long, Long> eldest) {
            return size() > MAX_ARTEMIS_QUEUED;
        }
    };
    private long artemisDecodeMs;

    FrameLatencyStats() {
        this(false);
    }

    FrameLatencyStats(boolean vrr) {
        this.vrr = vrr;
    }

    private static class Frame {
        int number;
        int outputIndex = -1;
        long ptsUs;
        long receiveNs;
        long enqueueNs;
        long inputNs;
        long outputNs;
        long renderNs;
        long callbackReceivedNs;
        long releaseNs;
        long hostProcessingNs;
        // The frame's presentationTimeUs, the host's RTP timestamp in microseconds; -1 when unknown.
        long hostRtpUs = -1;
        HostTimingReport.Record hostTiming;
        long previousOutputNs;
        long previousReleaseNs;
        boolean awaitingRender;
        String status;
        long schedVsyncNs;
        long schedHoldNs = -1;
        boolean schedLate;
        int queueDepth = -1;
        long residencyExcessNs = -1;
        int inFlight = -1;
        String dropReason;
    }

    final LatencyProbe latencyProbe = new LatencyProbe();

    synchronized void onFrameReceived(long receiveNs) {
        if (fpsStartNs == 0) fpsStartNs = receiveNs;
        recordFrameTime(0, receiveNs);
        latencyProbe.onFrameReceived(receiveNs);
    }

    synchronized void onDecoderInput(int frameNumber, long ptsUs, long receiveNs, long enqueueNs,
                                     long inputNs, char hostProcessingLatency) {
        onDecoderInput(frameNumber, ptsUs, receiveNs, enqueueNs, inputNs, hostProcessingLatency, -1);
    }

    // hostRtpUs is the frame's RTP timestamp in host microseconds (moonlight's presentationTimeUs).
    synchronized void onDecoderInput(int frameNumber, long ptsUs, long receiveNs, long enqueueNs,
                                     long inputNs, char hostProcessingLatency, long hostRtpUs) {
        if (pending.size() == MAX_PENDING) {
            finish(pending.values().iterator().next(), "tracking_overflow");
        }
        Frame frame = new Frame();
        frame.number = frameNumber;
        frame.ptsUs = ptsUs;
        frame.hostRtpUs = hostRtpUs;
        frame.receiveNs = receiveNs;
        frame.enqueueNs = enqueueNs;
        frame.inputNs = inputNs;
        // The unsigned wire value is in 100 us units; zero means unavailable.
        frame.hostProcessingNs = hostProcessingLatency * 100000L;
        pending.put(ptsUs, frame);
        frame.hostTiming = earlyReports.remove(frameNumber);
        // Stage 4 is only ever the header's value. Never feed a host timing report into it:
        // present to last packet sent includes the network send and reads several times higher.
        if (frame.hostProcessingNs != 0) {
            addDuration(HOST_STAGE, frame.hostProcessingNs, inputNs);
        }
    }

    // A host timing report (0x5536), which arrives on the native control thread. Its present to
    // last-packet-sent time and the queue, encode and send split get their own stages; they never
    // touch the host processing stage, which stays the frame header's value.
    synchronized void onHostTiming(HostTimingReport report, long nowNs) {
        for (int i = 0; i < report.size(); i++) {
            HostTimingReport.Record record = report.get(i);
            addDuration(HOST_SENT_STAGE, record.hostUs * 1000L, nowNs);
            addDuration(HOST_QUEUE_STAGE, record.queueUs * 1000L, nowNs);
            addDuration(HOST_QUEUE_STAGE + 1, record.encodeUs * 1000L, nowNs);
            addDuration(HOST_SEND_STAGE, record.sendUs * 1000L, nowNs);
            Frame frame = findFrame(record.frame);
            if (frame != null) {
                frame.hostTiming = record;
            } else {
                if (earlyReports.size() == MAX_EARLY_REPORTS) {
                    Iterator<Integer> oldest = earlyReports.keySet().iterator();
                    oldest.next();
                    oldest.remove();
                }
                earlyReports.put(record.frame, record);
            }
        }
    }

    private Frame findFrame(int frameNumber) {
        for (Frame frame : pending.values()) {
            if (frame.number == frameNumber) {
                return frame;
            }
        }
        int scanned = 0;
        for (Iterator<Frame> newest = completed.descendingIterator(); newest.hasNext() && scanned < MAX_COMPLETED_SCAN; scanned++) {
            Frame frame = newest.next();
            if (frame.number == frameNumber) {
                return frame;
            }
        }
        return null;
    }

    // queueInputBuffer() has returned: decode time starts here, like other Moonlight clients count
    // it, and the queue call itself belongs to the queue wait. A frame whose output already came
    // (the decoder beat the bookkeeping) keeps its earlier start.
    synchronized void onDecoderQueued(long ptsUs, long queuedNs) {
        artemisQueuedNs.put(ptsUs, queuedNs);
        Frame early = pending.get(ptsUs);
        if (early != null && early.outputNs != 0) {
            artemisOutputNs.put(ptsUs, early.outputNs);
        }
        recordDecoderQueued(ptsUs, queuedNs);
    }

    // PyroWave measures CPU submission separately and never consumes the MediaCodec hand-off
    // counter. Keep its precise queue/completion samples without retaining unused boxed entries.
    synchronized void onPyroWaveDecoderQueued(long ptsUs, long queuedNs) {
        recordDecoderQueued(ptsUs, queuedNs);
    }

    private void recordDecoderQueued(long ptsUs, long queuedNs) {
        Frame frame = pending.get(ptsUs);
        if (frame == null) {
            return;
        }
        if (frame.outputNs == 0) {
            frame.inputNs = queuedNs;
        }
        addSample(0, frame.enqueueNs, frame.inputNs);
    }

    synchronized void onDecoderOutput(int index, long ptsUs, long outputNs) {
        if (artemisQueuedNs.containsKey(ptsUs)) {
            // Decode time ends here, when the decoder hands the frame back.
            artemisOutputNs.put(ptsUs, outputNs);
        }
        Frame frame = pending.get(ptsUs);
        if (frame != null) {
            frame.outputIndex = index;
            frame.outputNs = outputNs;
            if (vrr) {
                frame.previousOutputNs = lastOutputNs;
                lastOutputNs = outputNs;
            }
            outputs.put(index, frame);
            if (frame.inputNs > 0 && outputNs >= frame.inputNs) {
                addSample(1, frame.inputNs, outputNs);
                decodeTimeTotalNs += outputNs - frame.inputNs;
                decodeTimeSamples++;
                totalDecodeTimeNs += outputNs - frame.inputNs;
                totalDecodedFrames++;
                if (frame.receiveNs > 0 && frame.inputNs >= frame.receiveNs) {
                    totalClientTimeNs += outputNs - frame.receiveNs;
                    totalClientTimeSamples++;
                }
            }
        }
    }

    synchronized void onOutputReleased(int index, long releaseNs, boolean render, boolean hasRenderCallback) {
        Frame frame = outputs.remove(index);
        if (frame != null) {
            frame.outputIndex = -1;
            frame.releaseNs = releaseNs;
            frame.awaitingRender = render && hasRenderCallback;
            if (render) {
                renderCallbacksAvailable = hasRenderCallback;
                recordFrameTime(1, releaseNs);
            }
            if (vrr && render && frame.outputNs > 0 && releaseNs >= frame.outputNs) {
                frame.previousReleaseNs = lastReleaseNs;
                lastReleaseNs = releaseNs;
            }
            if (!render) {
                finish(frame, "dropped");
            } else if (!hasRenderCallback) {
                // Without render callbacks the release is the closest moment to the screen.
                latencyProbe.onFrameRendered(frame.receiveNs, releaseNs);
                finish(frame, "render_unavailable");
            }
        }
    }

    /** Input to output time of a still-owned output buffer, or zero when untracked. */
    synchronized long getInputToOutputNs(int outputIndex) {
        Frame frame = outputs.get(outputIndex);
        return frame == null || frame.inputNs <= 0 || frame.outputNs < frame.inputNs ? 0 :
                frame.outputNs - frame.inputNs;
    }

    /** Notes where the scheduler planned a still-owned output; call before it is released. */
    synchronized void onScheduled(int outputIndex, long vsyncNs, long holdNs, boolean late, int inFlight) {
        Frame frame = outputs.get(outputIndex);
        if (frame != null) {
            frame.schedVsyncNs = vsyncNs;
            frame.schedHoldNs = Math.max(0, holdNs);
            frame.schedLate = late;
            frame.inFlight = inFlight;
        }
    }

    /** Notes the decoder queue seen by a still-owned output: frames behind it, and its residency excess. */
    synchronized void onQueueState(int outputIndex, int depth, long excessNs) {
        Frame frame = outputs.get(outputIndex);
        if (frame != null) {
            frame.queueDepth = Math.max(0, depth);
            frame.residencyExcessNs = Math.max(0, excessNs);
        }
    }

    /** Notes why a still-owned output is about to be dropped; call before the release. */
    synchronized void onDropReason(int outputIndex, String reason) {
        Frame frame = outputs.get(outputIndex);
        if (frame != null) frame.dropReason = reason;
    }

    /** The local queue timestamp for a still-owned output buffer, or zero when untracked. */
    synchronized long getEnqueueTimeNsForOutput(int outputIndex) {
        Frame frame = outputs.get(outputIndex);
        return frame == null || frame.enqueueNs <= 0 || frame.enqueueNs > frame.outputNs ? 0 : frame.enqueueNs;
    }

    void onFrameRendered(long ptsUs, long renderNs) {
        onFrameRendered(ptsUs, renderNs, 0);
    }

    synchronized void onFrameRendered(long ptsUs, long renderNs, long callbackReceivedNs) {
        Frame frame = pending.get(ptsUs);
        if (frame != null) {
            frame.renderNs = renderNs;
            // Delivery is diagnostic only. Render latency and FPS use the platform timestamp.
            frame.callbackReceivedNs = callbackReceivedNs;
            boolean valid = frame.inputNs > 0 && frame.outputNs >= frame.inputNs && renderNs >= frame.outputNs;
            if (valid) {
                recordFrameTime(2, renderNs);
                addSample(2, frame.outputNs, renderNs);
                addSample(3, frame.receiveNs, renderNs);
                latencyProbe.onFrameRendered(frame.receiveNs, renderNs);
            }
            finish(frame, valid ? "rendered" : "invalid_render_time");
        }
    }

    synchronized void discard(long ptsUs, String reason) {
        Frame frame = pending.get(ptsUs);
        if (frame != null) {
            // Vulkan discovers missing display timing when polling after the first release.
            if ("render_unavailable".equals(reason)) renderCallbacksAvailable = false;
            finish(frame, reason);
        }
    }

    synchronized void discardPending(String reason) {
        for (Frame frame : new ArrayList<>(pending.values())) {
            finish(frame, reason);
        }
        lastOutputNs = lastReleaseNs = 0;
        decodeTimeTotalNs = 0;
        decodeTimeSamples = 0;
        resetDecodeTiming();
        artemisQueuedNs.clear();
        artemisOutputNs.clear();
        fpsStartNs = 0;
        for (long[] times : frameTimes) Arrays.fill(times, 0);
    }

    synchronized float takeDecodeTimeMs() {
        float result = decodeTimeSamples == 0 ? -1 : decodeTimeTotalNs / (decodeTimeSamples * 1000000.0f);
        decodeTimeTotalNs = 0;
        decodeTimeSamples = 0;
        return result;
    }

    /**
     * Only new outputs since the last quality tick count. The existing ring bounds this scan;
     * observing it neither consumes overlay/CSV data nor allocates anything per decoded frame.
     */
    synchronized DecodeTiming takeDecodeTiming(long nowNs, int fps) {
        long startNs = Math.max(decodeTimingPollNs, Math.max(0, nowNs - FPS_WINDOW_NS));
        int available = (int) Math.min(counts[1], totalDecodedFrames - decodeTimingConsumedFrames);
        int sampled = 0, late = 0;
        long first = 0, last = 0;
        if (nowNs > 0 && nowNs >= decodeTimingPollNs && fps > 0) {
            for (int offset = 1; offset <= available; offset++) {
                int index = (positions[1] - offset + WINDOW_SIZE) % WINDOW_SIZE;
                long observed = sampleTimes[1][index];
                if (observed <= startNs || observed > nowNs) continue;
                sampled++;
                if (samples[1][index] > DecodeTiming.decodeBudgetMs(fps) * 1_000_000d) late++;
                first = first == 0 ? observed : Math.min(first, observed);
                last = Math.max(last, observed);
            }
        }
        decodeTimingConsumedFrames = totalDecodedFrames;
        decodeTimingPollNs = Math.max(0, nowNs);
        return new DecodeTiming(decodeTimingGeneration, startNs, nowNs, first, last, fps, sampled, late);
    }

    /** A reset/reconfigure must not carry evidence from the previous decoder workload. */
    synchronized void resetDecodeTiming() {
        decodeTimingGeneration++;
        decodeTimingConsumedFrames = totalDecodedFrames;
        decodeTimingPollNs = 0;
    }

    // A frame Artemis counts: the one it presents, or one it puts in the pacing queue without
    // dropping an older one. Its decode time runs from queueInputBuffer() returning to the
    // decoder returning the frame; release and pacing waits after that are not decode time. Like
    // Artemis, a frame is rounded down to whole milliseconds and a second or more is left out.
    synchronized void onArtemisDecodeCounted(long ptsUs) {
        Long queuedNs = artemisQueuedNs.remove(ptsUs);
        Long outputNs = artemisOutputNs.remove(ptsUs);
        if (queuedNs != null && outputNs != null && outputNs >= queuedNs) {
            long ms = (outputNs - queuedNs) / 1000000L;
            if (ms < 1000) {
                artemisDecodeMs += ms;
            }
        }
    }

    synchronized long getArtemisDecodeMs() {
        return artemisDecodeMs;
    }

    // Whole milliseconds counted since the last call; the caller divides by frames received.
    synchronized long takeArtemisDecodeMs() {
        long result = artemisDecodeMs;
        artemisDecodeMs = 0;
        return result;
    }

    synchronized int getAverageDecoderLatency() {
        return totalDecodedFrames == 0 ? 0 : (int) (totalDecodeTimeNs / totalDecodedFrames / 1000000);
    }

    synchronized int getAverageEndToEndLatency() {
        return totalClientTimeSamples == 0 ? 0 : (int) (totalClientTimeNs / totalClientTimeSamples / 1000000);
    }

    private void recordFrameTime(int stage, long timeNs) {
        frameTimes[stage][frameTimePositions[stage]] = timeNs;
        frameTimePositions[stage] = (frameTimePositions[stage] + 1) % frameTimes[stage].length;
    }

    synchronized float[] getFrameRates(long nowNs) {
        float[] rates = new float[3];
        long startNs = Math.max(fpsStartNs, nowNs - FPS_WINDOW_NS);
        if (fpsStartNs != 0 && nowNs > startNs) {
            for (int stage = 0; stage < rates.length; stage++) {
                int count = 0;
                for (long timestamp : frameTimes[stage]) {
                    if (timestamp > startNs && timestamp <= nowNs) count++;
                }
                rates[stage] = (float) (count * 1000000000.0 / (nowNs - startNs));
            }
        }
        if (!renderCallbacksAvailable) rates[2] = -1;
        return rates;
    }

    synchronized long getPresentDrops() {
        return presentDrops;
    }

    synchronized void expire(long nowNs) {
        for (Frame frame : new ArrayList<>(pending.values())) {
            if (nowNs - (frame.awaitingRender ? frame.releaseNs : frame.inputNs) >= CALLBACK_TIMEOUT_NS) {
                // MediaCodec exposes no drop callback. Allow delayed render callbacks before estimating a drop.
                if (frame.awaitingRender) presentDrops++;
                finish(frame, frame.releaseNs != 0 ? "render_unobserved" : "output_unobserved");
            }
        }
    }

    private void finish(Frame frame, String status) {
        pending.remove(frame.ptsUs);
        if (frame.outputIndex != -1) {
            outputs.remove(frame.outputIndex);
        }
        frame.status = status;
        if (completed.size() == MAX_COMPLETED) {
            completed.removeFirst();
            csvRowsLost++;
        }
        completed.addLast(frame);
    }

    private void addSample(int stage, long startNs, long endNs) {
        if (startNs == 0 || endNs < startNs) {
            return;
        }
        addDuration(stage, endNs - startNs, endNs);
    }

    private void addDuration(int stage, long durationNs, long observedAtNs) {
        samples[stage][positions[stage]] = durationNs;
        sampleTimes[stage][positions[stage]] = observedAtNs;
        positions[stage] = (positions[stage] + 1) % WINDOW_SIZE;
        counts[stage] = Math.min(counts[stage] + 1, WINDOW_SIZE);
    }

    double[][] summarize() {
        return summarize(0, 0, false);
    }

    // Unlike the overlay's last-N-frame summary, a diagnostic tick must not revive a stage
    // value from minutes ago simply because new frames arrived at a different stage.
    double[][] summarizeRecent(long nowNs, long windowNs) {
        return summarize(nowNs, Math.max(0, windowNs), true);
    }

    private double[][] summarize(long nowNs, long windowNs, boolean recentOnly) {
        long[][] snapshot = new long[samples.length][];
        synchronized (this) {
            for (int stage = 0; stage < snapshot.length; stage++) {
                long[] values = new long[counts[stage]];
                int count = 0;
                for (int i = 0; i < counts[stage]; i++) {
                    long observedAt = sampleTimes[stage][i];
                    if (!recentOnly || (observedAt > 0 && observedAt <= nowNs && nowNs - observedAt <= windowNs)) {
                        values[count++] = samples[stage][i];
                    }
                }
                snapshot[stage] = count == values.length ? values : Arrays.copyOf(values, count);
            }
        }
        double[][] summary = new double[samples.length][4];
        for (int stage = 0; stage < snapshot.length; stage++) {
            long[] values = snapshot[stage];
            summary[stage][0] = values.length;
            if (values.length == 0) {
                continue;
            }
            Arrays.sort(values);
            double sum = 0;
            for (long value : values) {
                sum += value;
            }
            summary[stage][1] = sum / values.length / 1000000;
            summary[stage][2] = values[(int) Math.ceil(values.length * 0.95) - 1] / 1000000.0;
            summary[stage][3] = values[(int) Math.ceil(values.length * 0.99) - 1] / 1000000.0;
        }
        return summary;
    }

    synchronized long getCsvRowsLost() {
        return csvRowsLost;
    }

    synchronized boolean hasCompletedFrames() {
        return !completed.isEmpty();
    }

    void writeCsv(Writer writer) throws IOException {
        writeCsv(writer, null);
    }

    // The clock offset is read once per flush, before the lock: it calls into native code that
    // takes its own lock, and one value is plenty for the few hundred ms a flush covers.
    void writeCsv(Writer writer, ClockOffsetSource clockOffset) throws IOException {
        long[] clock = new long[2];
        String clockColumns = clockOffset != null && clockOffset.read(clock) ?
                clock[0] + "," + clock[1] : ",";
        List<Frame> frames;
        long lost;
        long dropped;
        long sampleNs = System.nanoTime();
        float[] rates;
        synchronized (this) {
            frames = new ArrayList<>(completed);
            completed.clear();
            lost = csvRowsLost;
            dropped = presentDrops;
            rates = getFrameRates(sampleNs);
        }
        // Disk I/O and formatting must never hold the lock used by the decoder threads.
        String frameRates = String.format(Locale.ROOT, "%.2f,%.2f,", rates[0], rates[1]) +
                (rates[2] < 0 ? "" : String.format(Locale.ROOT, "%.2f", rates[2]));
        for (Frame frame : frames) {
            String decodeToPresent = "rendered".equals(frame.status) ? duration(frame.outputNs, frame.renderNs) : "";
            String receiveToPresent = "rendered".equals(frame.status) ? duration(frame.receiveNs, frame.renderNs) : "";
            writer.write(frame.number + "," + frame.ptsUs + "," + timestamp(frame.receiveNs) + "," +
                    timestamp(frame.inputNs) + "," + timestamp(frame.outputNs) + "," + timestamp(frame.renderNs) + "," +
                    timestamp(frame.releaseNs) + "," + frame.status + "," +
                    duration(frame.receiveNs, frame.inputNs) + "," + duration(frame.inputNs, frame.outputNs) + "," +
                    decodeToPresent + "," + receiveToPresent + "," + lost + "," +
                    (frame.hostProcessingNs == 0 ? "" : milliseconds(frame.hostProcessingNs)) + "," +
                    decodeToPresent + "," + receiveToPresent + "," +
                    (vrr ? duration(frame.outputNs, frame.releaseNs) : "") + "," +
                    duration(frame.previousOutputNs, frame.outputNs) + "," +
                    duration(frame.previousReleaseNs, frame.releaseNs) + "," + sampleNs + "," +
                    frameRates + "," + dropped + "," +
                    timestamp(frame.enqueueNs) + "," + duration(frame.enqueueNs, frame.inputNs) +
                    hostTimingColumns(frame.hostTiming) + "," + timestamp(frame.callbackReceivedNs) + "," +
                    duration(frame.renderNs, frame.callbackReceivedNs) +
                    "," + (frame.hostRtpUs < 0 ? "" : Long.toString(frame.hostRtpUs)) +
                    hostStampColumns(frame.hostTiming) + "," + clockColumns + schedulerColumns(frame) + "\n");
        }
    }

    private static String schedulerColumns(Frame frame) {
        return "," + timestamp(frame.schedVsyncNs) +
                "," + (frame.schedHoldNs < 0 ? "" : milliseconds(frame.schedHoldNs)) +
                "," + (frame.schedHoldNs < 0 ? "" : frame.schedLate ? "1" : "0") +
                "," + (frame.queueDepth < 0 ? "" : Integer.toString(frame.queueDepth)) +
                "," + (frame.residencyExcessNs < 0 ? "" : milliseconds(frame.residencyExcessNs)) +
                "," + (frame.inFlight < 0 ? "" : Integer.toString(frame.inFlight)) +
                "," + (frame.dropReason == null ? "" : frame.dropReason);
    }

    private static String hostTimingColumns(HostTimingReport.Record record) {
        return record == null ? ",,,," : "," + milliseconds(record.hostUs * 1000L) +
                "," + milliseconds(record.queueUs * 1000L) + "," + milliseconds(record.encodeUs * 1000L) +
                "," + milliseconds(record.sendUs * 1000L);
    }

    // Host present, acquired, submitted, encoded, first and last packet sent, on the host's video
    // clock (microseconds), each with its leading comma; empty without an extended report.
    private static String hostStampColumns(HostTimingReport.Record record) {
        if (record == null || !record.hasExtra) return ",,,,,,";
        long present = record.presentedAtUs;
        long submitted = present + record.queueUs;
        long lastSent = present + record.hostUs;
        return "," + Long.toUnsignedString(present) +
                "," + Long.toUnsignedString(present + record.captureUs) +
                "," + Long.toUnsignedString(submitted) +
                "," + Long.toUnsignedString(submitted + record.encodeUs) +
                "," + Long.toUnsignedString(lastSent - record.sendUs) +
                "," + Long.toUnsignedString(lastSent);
    }

    private static String timestamp(long timeNs) {
        return timeNs == 0 ? "" : Long.toString(timeNs);
    }

    private static String duration(long startNs, long endNs) {
        return startNs == 0 || endNs == 0 || endNs < startNs ? "" :
                milliseconds(endNs - startNs);
    }

    private static String milliseconds(long durationNs) {
        return durationNs / 1000000 + "." + Long.toString(1000000 + durationNs % 1000000).substring(1);
    }
}
