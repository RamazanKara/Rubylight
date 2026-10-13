package com.limelight.binding.video;

import java.io.IOException;
import java.io.Writer;
import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.HashMap;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Locale;

class FrameLatencyStats {
    static final int WINDOW_SIZE = 600;
    // Cover the five-second callback timeout at the maximum 1000 FPS stream rate.
    private static final int MAX_PENDING = 8192;
    private static final int MAX_COMPLETED = 4096;
    private static final long CALLBACK_TIMEOUT_NS = 5000000000L;
    private static final long FPS_WINDOW_NS = 2000000000L;
    static final String CSV_HEADER = "frame_number,pts_us,receive_ns,decoder_input_ns,decoder_output_ns," +
            "render_ns,release_ns,status,receive_to_input_ms,input_to_output_ms,output_to_render_ms," +
            "receive_to_render_ms,csv_rows_lost,host_processing_ms," +
            "decode_to_present_ms,receive_to_present_ms," +
            "decode_to_release_ms,decode_interval_ms,release_interval_ms," +
            "fps_sample_ns,received_fps,released_fps,shown_fps,present_drops,enqueue_ns,queue_wait_ms\n";

    private final LinkedHashMap<Long, Frame> pending = new LinkedHashMap<>();
    private final HashMap<Integer, Frame> outputs = new HashMap<>();
    private final ArrayDeque<Frame> completed = new ArrayDeque<>();
    private final long[][] samples = new long[5][WINDOW_SIZE];
    private final int[] counts = new int[5];
    private final int[] positions = new int[5];
    private long csvRowsLost;
    private final boolean vrr;
    private long lastOutputNs;
    private long lastReleaseNs;
    private long outputIntervalTotalNs;
    private int outputIntervalCount;
    private long outputIntervalMinNs = Long.MAX_VALUE;
    private long outputIntervalMaxNs;
    private long outputReceiveIntervalTotalNs;
    private long lastOutputReceiveNs;
    private int lastOutputFrameNumber;
    private boolean outputCadenceInterrupted;
    private long decodeTimeTotalNs;
    private int decodeTimeSamples;
    private long totalDecodeTimeNs;
    private long totalDecodedFrames;
    private long totalClientTimeNs;
    private long totalClientTimeSamples;
    private final long[][] frameTimes = new long[3][2048];
    private final int[] frameTimePositions = new int[3];
    private long fpsStartNs;
    private boolean renderCallbacksAvailable;
    private long presentDrops;

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
        long releaseNs;
        long hostProcessingNs;
        long previousOutputNs;
        long previousReleaseNs;
        boolean awaitingRender;
        String status;
    }

    final LatencyProbe latencyProbe = new LatencyProbe();

    synchronized void onFrameReceived(long receiveNs) {
        if (fpsStartNs == 0) fpsStartNs = receiveNs;
        recordFrameTime(0, receiveNs);
        latencyProbe.onFrameReceived(receiveNs);
    }

    synchronized void onDecoderInput(int frameNumber, long ptsUs, long receiveNs, long enqueueNs,
                                     long inputNs, char hostProcessingLatency) {
        if (pending.size() == MAX_PENDING) {
            finish(pending.values().iterator().next(), "tracking_overflow");
        }
        Frame frame = new Frame();
        frame.number = frameNumber;
        frame.ptsUs = ptsUs;
        frame.receiveNs = receiveNs;
        frame.enqueueNs = enqueueNs;
        frame.inputNs = inputNs;
        // The unsigned wire value is in 100 us units; zero means unavailable.
        frame.hostProcessingNs = hostProcessingLatency * 100000L;
        pending.put(ptsUs, frame);
        if (frame.hostProcessingNs != 0) {
            addDuration(4, frame.hostProcessingNs);
        }
    }

    // queueInputBuffer() has returned: decode time starts here, like other Moonlight clients count
    // it, and the queue call itself belongs to the queue wait. A frame whose output already came
    // (the decoder beat the bookkeeping) keeps its earlier start.
    synchronized void onDecoderQueued(long ptsUs, long queuedNs) {
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
        Frame frame = pending.get(ptsUs);
        if (frame != null) {
            frame.outputIndex = index;
            frame.outputNs = outputNs;
            if (vrr) {
                frame.previousOutputNs = lastOutputNs;
                if (lastOutputNs > 0) {
                    long intervalNs = outputNs - lastOutputNs;
                    long receiveIntervalNs = frame.receiveNs - lastOutputReceiveNs;
                    if (intervalNs <= 0 || receiveIntervalNs <= 0 || frame.number - lastOutputFrameNumber != 1) {
                        outputCadenceInterrupted = true;
                    }
                    if (intervalNs > 0) {
                        outputIntervalTotalNs += intervalNs;
                        outputIntervalCount++;
                        outputIntervalMinNs = Math.min(outputIntervalMinNs, intervalNs);
                        outputIntervalMaxNs = Math.max(outputIntervalMaxNs, intervalNs);
                        outputReceiveIntervalTotalNs += Math.max(0, receiveIntervalNs);
                    }
                }
                lastOutputNs = outputNs;
                lastOutputReceiveNs = frame.receiveNs;
                lastOutputFrameNumber = frame.number;
            }
            outputs.put(index, frame);
            addSample(1, frame.inputNs, outputNs);
            if (frame.inputNs > 0 && outputNs >= frame.inputNs) {
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

    synchronized void onFrameRendered(long ptsUs, long renderNs) {
        Frame frame = pending.get(ptsUs);
        if (frame != null) {
            frame.renderNs = renderNs;
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
        lastOutputNs = lastReleaseNs = lastOutputReceiveNs = 0;
        resetOutputIntervals();
        decodeTimeTotalNs = 0;
        decodeTimeSamples = 0;
        fpsStartNs = 0;
        for (long[] times : frameTimes) Arrays.fill(times, 0);
    }

    synchronized float takeDecodeTimeMs() {
        float result = decodeTimeSamples == 0 ? -1 : decodeTimeTotalNs / (decodeTimeSamples * 1000000.0f);
        decodeTimeTotalNs = 0;
        decodeTimeSamples = 0;
        return result;
    }

    synchronized int getAverageDecoderLatency() {
        return totalDecodedFrames == 0 ? 0 : (int) (totalDecodeTimeNs / totalDecodedFrames / 1000000);
    }

    synchronized int getAverageEndToEndLatency() {
        return totalClientTimeSamples == 0 ? 0 : (int) (totalClientTimeNs / totalClientTimeSamples / 1000000);
    }

    synchronized float takeOutputFrameRate(long nowNs) {
        double meanIntervalNs = outputIntervalCount == 0 ? 0 :
                (double) outputIntervalTotalNs / outputIntervalCount;
        // Decoder completion is independent of display pacing, including frames replaced before
        // presentation. Loss, stalls, bursts and decoder backpressure are not evidence of lower
        // game cadence. Receive timestamps distinguish a slow decoder from a slower source, but
        // neither timestamp can distinguish a steadily updating desktop from a game.
        boolean continuous = outputIntervalCount >= 2 && !outputCadenceInterrupted &&
                outputIntervalMinNs >= meanIntervalNs * 0.5 && outputIntervalMaxNs <= meanIntervalNs * 1.5 &&
                Math.abs((double) outputReceiveIntervalTotalNs - outputIntervalTotalNs) <= outputIntervalTotalNs * 0.1 &&
                nowNs >= lastOutputNs && nowNs - lastOutputNs <= Math.max(50000000, meanIntervalNs * 2);
        float frameRate = continuous ? (float) (1000000000.0 / meanIntervalNs) : 0;
        resetOutputIntervals();
        return frameRate;
    }

    private void resetOutputIntervals() {
        outputIntervalTotalNs = 0;
        outputIntervalCount = 0;
        outputIntervalMinNs = Long.MAX_VALUE;
        outputIntervalMaxNs = outputReceiveIntervalTotalNs = 0;
        outputCadenceInterrupted = false;
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
        addDuration(stage, endNs - startNs);
    }

    private void addDuration(int stage, long durationNs) {
        samples[stage][positions[stage]] = durationNs;
        positions[stage] = (positions[stage] + 1) % WINDOW_SIZE;
        counts[stage] = Math.min(counts[stage] + 1, WINDOW_SIZE);
    }

    double[][] summarize() {
        long[][] snapshot = new long[samples.length][];
        synchronized (this) {
            for (int stage = 0; stage < snapshot.length; stage++) {
                snapshot[stage] = Arrays.copyOf(samples[stage], counts[stage]);
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
                    timestamp(frame.enqueueNs) + "," + duration(frame.enqueueNs, frame.inputNs) + "\n");
        }
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
