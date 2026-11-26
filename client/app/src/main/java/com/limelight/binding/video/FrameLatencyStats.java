package com.limelight.binding.video;

import com.limelight.nvstream.av.video.HostTimingReport;
import com.limelight.nvstream.DecodeTiming;

import java.io.IOException;
import java.io.Writer;
import java.util.ArrayList;
import java.util.Locale;

class FrameLatencyStats {
    static final int WINDOW_SIZE = 600;
    // Cover the five-second callback timeout at the maximum 1000 FPS stream rate.
    static final int MAX_PENDING = 8192;
    static final long CALLBACK_TIMEOUT_NS = 5000000000L;
    static final long FPS_WINDOW_NS = 2000000000L;
    // Cap on compositor report lag (MediaCodec batching / Vulkan timing) compensated in Presented FPS.
    static final long MAX_PRESENT_REPORT_LAG_NS = 150_000_000L;
    // getFrameRates() indexes: frames submitted for display, and frames the compositor confirmed.
    static final int RENDERED_STAGE = 1;
    static final int PRESENTED_STAGE = 2;
    // Sample stages: 0 queue wait, 1 decode, 2 display wait, 3 client latency (frame complete to
    // hand-off, the three before it together), 4 host processing
    // (the video frame header's value, the one Artemis and Moonlight show), 5 to 7 the host's queue,
    // encode and send split, 8 its present to last packet sent (only from host timing reports), and
    // 9 decoded to on screen.
    // Display wait and client latency end where Nightjar hands the frame to the display: at the time
    // it asked the compositor to show it (direct path, onOutputHandedOff), or at the queued present
    // (Vulkan, onFrameShown, and PyroWave, onPresentQueued). Android's compositor and the panel come
    // after that; that part only goes to stage 9, where the device reports it, which the stream doctor
    // judges and the overlays do not show.
    static final int STAGES = 10;
    private static final int HOST_STAGE = 4;
    static final int HOST_QUEUE_STAGE = 5;
    static final int HOST_SEND_STAGE = 7;
    static final int HOST_SENT_STAGE = 8;
    static final int SCREEN_STAGE = 9;
    // A display time further ahead than this is not a plan the compositor keeps (MediaCodec ignores
    // timestamps a second or more away), so the hand-off is the release itself.
    private static final long MAX_DISPLAY_AHEAD_NS = 1_000_000_000L;
    // Queued frames Artemis never counts (replaced before display) are forgotten past this many.
    static final int MAX_ARTEMIS_QUEUED = 1024;
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

    // Every renderer, decoder and display callback touches these per frame, under this lock:
    // primitive keys and recycled frames keep that path free of boxing and garbage.
    private final FrameLatencyTables.PendingFrames pending = new FrameLatencyTables.PendingFrames(MAX_PENDING);
    private final FrameLatencyTables.OutputIndexTable outputTable = new FrameLatencyTables.OutputIndexTable(MAX_PENDING);
    private final FrameLatencyTables.FrameStore store = new FrameLatencyTables.FrameStore();
    private final FrameLatencyMetrics metrics = new FrameLatencyMetrics();
    private Frame lastRenderRelease;
    private final boolean vrr;
    private long lastOutputNs;
    private long lastReleaseNs;
    // Present intervals for the overlay's frame pacing lines; records only while the overlay is on.
    private final PresentJitter presentJitter = new PresentJitter();
    // Lowest latency and Balanced: watches the display's queue through the waits of handed-off frames.
    private DisplayQueueWatch displayQueueWatch;
    // Self-detecting keepalive monitor for compositor stalls / post-handoff judder.
    private CompositorWatch compositorWatch;
    // A flush's rows and drained frames are reused across flushes; only the latency thread flushes.
    private final StringBuilder csvRows = new StringBuilder(16 * 1024);
    private final ArrayList<Frame> csvDrain = new ArrayList<>(256);

    FrameLatencyStats() {
        this(false);
    }

    FrameLatencyStats(boolean vrr) {
        this.vrr = vrr;
    }

    static class Frame {
        int number;
        int outputIndex = -1;
        long ptsUs;
        long receiveNs;
        long enqueueNs;
        long inputNs;
        // Just before queueInputBuffer(); inputNs moves to when it returned.
        long submitNs;
        long outputNs;
        long renderNs;
        long callbackReceivedNs;
        long releaseNs;
        // When Nightjar handed the frame to the display (onOutputHandedOff), 0 on other paths.
        long handoffNs;
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
        // PendingFrames' arrival order.
        Frame older, newer;

        void reset() {
            number = 0;
            outputIndex = -1;
            ptsUs = receiveNs = enqueueNs = inputNs = submitNs = outputNs = renderNs = 0;
            callbackReceivedNs = releaseNs = handoffNs = hostProcessingNs = 0;
            hostRtpUs = -1;
            hostTiming = null;
            previousOutputNs = previousReleaseNs = 0;
            awaitingRender = false;
            status = null;
            schedVsyncNs = 0;
            schedHoldNs = -1;
            schedLate = false;
            queueDepth = -1;
            residencyExcessNs = -1;
            inFlight = -1;
            dropReason = null;
            older = newer = null;
        }
    }

    /** Slots the output index tables hold, for tests: bounded whatever the indices. */
    synchronized int outputSlotCount() {
        return outputTable.slotCount();
    }

    // Only for frames nothing else holds: dropped from the completed queue or written to the CSV.
    private void recycle(Frame frame) {
        if (lastRenderRelease == frame) lastRenderRelease = null;
        store.recycle(frame);
    }

    final LatencyProbe latencyProbe = new LatencyProbe();

    synchronized void onFrameReceived(long receiveNs) {
        metrics.onFrameReceived(receiveNs);
        latencyProbe.onFrameReceived(receiveNs);
    }

    synchronized void onDecoderInput(int frameNumber, long ptsUs, long receiveNs, long enqueueNs,
                                     long inputNs, char hostProcessingLatency) {
        onDecoderInput(frameNumber, ptsUs, receiveNs, enqueueNs, inputNs, hostProcessingLatency, -1);
    }

    // hostRtpUs is the frame's RTP timestamp in host microseconds (moonlight's presentationTimeUs).
    synchronized void onDecoderInput(int frameNumber, long ptsUs, long receiveNs, long enqueueNs,
                                     long inputNs, char hostProcessingLatency, long hostRtpUs) {
        if (pending.size == MAX_PENDING) {
            finish(pending.oldest, "tracking_overflow");
        }
        Frame frame = store.newFrame();
        frame.number = frameNumber;
        frame.ptsUs = ptsUs;
        frame.hostRtpUs = hostRtpUs;
        frame.receiveNs = receiveNs;
        frame.enqueueNs = enqueueNs;
        frame.inputNs = inputNs;
        frame.submitNs = inputNs;
        // The unsigned wire value is in 100 us units; zero means unavailable.
        frame.hostProcessingNs = hostProcessingLatency * 100000L;
        pending.put(frame);
        store.indexByNumber(frame);
        frame.hostTiming = store.takeEarlyReport(frameNumber);
        // Stage 4 is only ever the header's value. Never feed a host timing report into it:
        // present to last packet sent includes the network send and reads several times higher.
        if (frame.hostProcessingNs != 0) {
            metrics.addDuration(HOST_STAGE, frame.hostProcessingNs, inputNs);
        }
    }

    // A host timing report (0x5536), which arrives on the native control thread. Its present to
    // last-packet-sent time and the queue, encode and send split get their own stages; they never
    // touch the host processing stage, which stays the frame header's value.
    synchronized void onHostTiming(HostTimingReport report, long nowNs) {
        for (int i = 0; i < report.size(); i++) {
            HostTimingReport.Record record = report.get(i);
            metrics.addDuration(HOST_SENT_STAGE, record.hostUs * 1000L, nowNs);
            metrics.addDuration(HOST_QUEUE_STAGE, record.queueUs * 1000L, nowNs);
            metrics.addDuration(HOST_QUEUE_STAGE + 1, record.encodeUs * 1000L, nowNs);
            metrics.addDuration(HOST_SEND_STAGE, record.sendUs * 1000L, nowNs);
            Frame frame = store.findFrame(record.frame, pending);
            if (frame != null) {
                frame.hostTiming = record;
            } else {
                store.putEarlyReport(record.frame, record);
            }
        }
    }

    // queueInputBuffer() has returned: decode time starts here, like other Moonlight clients count
    // it, and the queue call itself belongs to the queue wait. A frame whose output already came
    // (the decoder beat the bookkeeping) keeps its earlier start.
    synchronized void onDecoderQueued(long ptsUs, long queuedNs) {
        Frame frame = pending.get(ptsUs);
        metrics.onArtemisQueued(ptsUs, queuedNs, frame != null ? frame.outputNs : 0);
        recordDecoderQueued(frame, queuedNs);
    }

    // PyroWave measures CPU submission separately and never consumes the MediaCodec hand-off
    // counter. Keep its precise queue/completion samples without retaining unused boxed entries.
    synchronized void onPyroWaveDecoderQueued(long ptsUs, long queuedNs) {
        recordDecoderQueued(pending.get(ptsUs), queuedNs);
    }

    /** Combined PyroWave input bookkeeping in one monitor acquisition (received + input + queued). */
    synchronized void onPyroWaveInput(int frameNumber, long ptsUs, long receiveNs, long enqueueNs,
                                      long inputNs, char hostProcessingLatency, long hostRtpUs, long queuedNs) {
        onFrameReceived(receiveNs);
        onDecoderInput(frameNumber, ptsUs, receiveNs, enqueueNs, inputNs, hostProcessingLatency, hostRtpUs);
        recordDecoderQueued(pending.get(ptsUs), queuedNs);
    }

    /** Combined drop-reason + non-render output release in one monitor acquisition. */
    synchronized void onOutputDropped(int outputIndex, String reason, long releaseNs, boolean hasRenderCallback) {
        onDropReason(outputIndex, reason);
        onOutputReleased(outputIndex, releaseNs, 0, false, hasRenderCallback, false);
    }

    /** Combined scheduler plan + output hand-off in one monitor acquisition. */
    synchronized void onScheduledHandedOff(int outputIndex, long vsyncNs, long holdNs, boolean late, int inFlight,
                                           long releaseNs, long displayAtNs, boolean hasRenderCallback) {
        onScheduled(outputIndex, vsyncNs, holdNs, late, inFlight);
        onOutputReleased(outputIndex, releaseNs, displayAtNs, true, hasRenderCallback, true);
    }

    static final int PYROWAVE_EVENT_DECODED = 0;
    static final int PYROWAVE_EVENT_QUEUED = 1;
    static final int PYROWAVE_EVENT_RENDERED = 2;
    static final int PYROWAVE_EVENT_STRIDE = 4;

    /** Applies a native batch of PyroWave frame lifecycle events under one monitor acquisition. */
    synchronized void applyPyroWaveEvents(long[] events, int eventCount, long pollNs) {
        for (int i = 0; i < eventCount; i++) {
            int base = i * PYROWAVE_EVENT_STRIDE;
            int kind = (int) events[base];
            long ptsUs = events[base + 1];
            long timeNs = events[base + 2];
            if (kind == PYROWAVE_EVENT_DECODED) {
                onDecoderOutput((int) ptsUs, ptsUs, timeNs);
            } else if (kind == PYROWAVE_EVENT_QUEUED) {
                long flags = events[base + 3];
                onPresentQueued((int) ptsUs, timeNs, (flags & 1L) != 0, (flags & 2L) != 0);
            } else if (kind == PYROWAVE_EVENT_RENDERED) {
                if (timeNs == 0) discard(ptsUs, "surface_changed");
                else onFrameRendered(ptsUs, timeNs, pollNs);
            }
        }
    }

    private void recordDecoderQueued(Frame frame, long queuedNs) {
        if (frame == null) {
            return;
        }
        if (frame.outputNs == 0) {
            frame.inputNs = queuedNs;
        }
        metrics.addSample(0, frame.enqueueNs, frame.inputNs);
    }

    synchronized void onDecoderOutput(int index, long ptsUs, long outputNs) {
        metrics.onArtemisOutput(ptsUs, outputNs);
        Frame frame = pending.get(ptsUs);
        if (frame != null) {
            frame.outputNs = outputNs;
            if (vrr) {
                frame.previousOutputNs = lastOutputNs;
                lastOutputNs = outputNs;
            }
            outputTable.put(index, frame);
            if (frame.inputNs > 0 && outputNs >= frame.inputNs) {
                metrics.recordDecodedFrame(frame.inputNs, outputNs, frame.receiveNs);
            }
        }
    }

    synchronized void onOutputReleased(int index, long releaseNs, boolean render, boolean hasRenderCallback) {
        onOutputReleased(index, releaseNs, 0, render, hasRenderCallback, false);
    }

    /**
     * A frame released straight to the display surface for rendering: display wait and client latency
     * end at the hand-off, the time Nightjar asked the compositor to show it (displayAtNs, when that is
     * later than the release and within a second), else the release. Its render callback, if any,
     * then only adds decoded to on screen.
     */
    synchronized void onOutputHandedOff(int index, long releaseNs, long displayAtNs, boolean hasRenderCallback) {
        onOutputReleased(index, releaseNs, displayAtNs, true, hasRenderCallback, true);
    }

    /**
     * PyroWave's release event: a shown frame's present was queued at releaseNs, which is its hand-off
     * (its display time, when the device reports one, only adds decoded to on screen); a frame that
     * was not shown was replaced.
     */
    synchronized void onPresentQueued(int index, long releaseNs, boolean shown, boolean hasRenderCallback) {
        onOutputReleased(index, releaseNs, 0, shown, hasRenderCallback, shown);
    }

    /**
     * Where client latency starts: when the frame was complete, so it is queue wait, decode and display
     * wait together. The time its packets took to arrive (from receiveNs) is the network's.
     */
    private static long clientStartNs(Frame frame) {
        return frame.enqueueNs != 0 ? frame.enqueueNs : frame.receiveNs;
    }

    static long handoffNs(long releaseNs, long displayAtNs) {
        return displayAtNs > releaseNs && displayAtNs - releaseNs < MAX_DISPLAY_AHEAD_NS ? displayAtNs : releaseNs;
    }

    private void onOutputReleased(int index, long releaseNs, long displayAtNs, boolean render, boolean hasRenderCallback,
                                  boolean handsOff) {
        Frame frame = outputTable.remove(index);
        if (frame != null) {
            frame.releaseNs = releaseNs;
            if (render && handsOff && frame.inputNs > 0 && frame.outputNs >= frame.inputNs && releaseNs >= frame.outputNs) {
                frame.handoffNs = handoffNs(releaseNs, displayAtNs);
                metrics.addSample(2, frame.outputNs, frame.handoffNs);
                metrics.addSample(3, clientStartNs(frame), frame.handoffNs);
            }
            frame.awaitingRender = render && hasRenderCallback;
            if (render) {
                metrics.onOutputRendered(releaseNs, hasRenderCallback);
                lastRenderRelease = frame;
                CompositorWatch cw = compositorWatch;
                if (cw != null) cw.onReleased(releaseNs, hasRenderCallback);
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
                presentJitter.record(releaseNs, true);
                finish(frame, "render_unavailable");
            }
        }
    }

    /**
     * releaseOutputBuffer() threw after onOutputReleased(render = true) had counted the frame as
     * submitted: take it back, so rendered FPS only counts frames the codec accepted.
     */
    synchronized void onReleaseFailed(long releaseNs) {
        Frame frame = lastRenderRelease;
        if (frame == null || frame.releaseNs != releaseNs) {
            return;
        }
        lastRenderRelease = null;
        if (frame.handoffNs != 0) {
            metrics.removeLastSample(2, frame.handoffNs);
            metrics.removeLastSample(3, frame.handoffNs);
            frame.handoffNs = 0;
        }
        metrics.undoLastRenderedFrameTime(releaseNs);
        frame.awaitingRender = false;
        if (pending.get(frame.ptsUs) == frame) {
            finish(frame, "release_failed");
        } else if ("render_unavailable".equals(frame.status)) {
            frame.status = "release_failed";
        }
    }

    /** Input to output time of a still-owned output buffer, or zero when untracked. */
    synchronized long getInputToOutputNs(int outputIndex) {
        Frame frame = outputTable.get(outputIndex);
        return frame == null || frame.inputNs <= 0 || frame.outputNs < frame.inputNs ? 0 :
                frame.outputNs - frame.inputNs;
    }

    /**
     * The time from just before a still-owned output's queue call, or zero when untracked. Unlike
     * getInputToOutputNs it holds no jitter of the call's return, which no output can follow.
     */
    synchronized long getSubmitToOutputNs(int outputIndex) {
        Frame frame = outputTable.get(outputIndex);
        return frame == null || frame.submitNs <= 0 || frame.outputNs < frame.submitNs ? 0 :
                frame.outputNs - frame.submitNs;
    }

    /** The three timings of getInputToOutputNs, getSubmitToOutputNs and getOutputNs, read in one call. */
    static final class OutputTimes {
        long inputToOutputNs;
        long submitToOutputNs;
        long outputNs;
    }

    /**
     * Fills out with what getInputToOutputNs, getSubmitToOutputNs and getOutputNs return for the
     * output, under one lock instead of three. Zeros when untracked.
     */
    synchronized void readOutputTimes(int outputIndex, OutputTimes out) {
        Frame frame = outputTable.get(outputIndex);
        if (frame == null) {
            out.inputToOutputNs = out.submitToOutputNs = out.outputNs = 0;
            return;
        }
        out.inputToOutputNs = frame.inputNs <= 0 || frame.outputNs < frame.inputNs ? 0 : frame.outputNs - frame.inputNs;
        out.submitToOutputNs = frame.submitNs <= 0 || frame.outputNs < frame.submitNs ? 0 : frame.outputNs - frame.submitNs;
        out.outputNs = frame.outputNs;
    }

    /** When a still-owned output buffer came out of the decoder, or zero when untracked. */
    synchronized long getOutputNs(int outputIndex) {
        Frame frame = outputTable.get(outputIndex);
        return frame == null ? 0 : frame.outputNs;
    }

    /** Notes where the scheduler planned a still-owned output; call before it is released. */
    synchronized void onScheduled(int outputIndex, long vsyncNs, long holdNs, boolean late, int inFlight) {
        Frame frame = outputTable.get(outputIndex);
        if (frame != null) {
            frame.schedVsyncNs = vsyncNs;
            frame.schedHoldNs = Math.max(0, holdNs);
            frame.schedLate = late;
            frame.inFlight = inFlight;
        }
    }

    /** Notes the decoder queue seen by a still-owned output: frames behind it, and its residency excess. */
    synchronized void onQueueState(int outputIndex, int depth, long excessNs) {
        Frame frame = outputTable.get(outputIndex);
        if (frame != null) {
            frame.queueDepth = Math.max(0, depth);
            frame.residencyExcessNs = Math.max(0, excessNs);
        }
    }

    /** Notes why a still-owned output is about to be dropped; call before the release. */
    synchronized void onDropReason(int outputIndex, String reason) {
        Frame frame = outputTable.get(outputIndex);
        if (frame != null) frame.dropReason = reason;
        metrics.recordDropReason(reason);
    }

    /**
     * The stream's drops by reason for its problem-report row, "other" for decoded frames not shown
     * without one (flushes, failed releases): notShown is frames out of the decoder minus rendered.
     */
    synchronized String describeDrops(long notShown) {
        return metrics.describeDrops(notShown);
    }

    /** The local queue timestamp for a still-owned output buffer, or zero when untracked. */
    synchronized long getEnqueueTimeNsForOutput(int outputIndex) {
        Frame frame = outputTable.get(outputIndex);
        return frame == null || frame.enqueueNs <= 0 || frame.enqueueNs > frame.outputNs ? 0 : frame.enqueueNs;
    }

    void onFrameRendered(long ptsUs, long renderNs) { onFrameRendered(ptsUs, renderNs, 0); }

    /** Codec render callback or PyroWave display report: when the frame reached the screen. */
    synchronized void onFrameRendered(long ptsUs, long renderNs, long callbackReceivedNs) {
        rendered(ptsUs, renderNs, callbackReceivedNs, renderNs);
    }

    /** Vulkan report: queuedNs ends display wait; scanoutNs (<= 0 when unknown) goes to screen stage, Presented FPS and jitter. */
    synchronized void onFrameShown(long ptsUs, long queuedNs, long scanoutNs, long callbackReceivedNs) {
        rendered(ptsUs, queuedNs > 0 ? queuedNs : scanoutNs, callbackReceivedNs, Math.max(0, scanoutNs));
    }

    synchronized void setDisplayQueueWatch(DisplayQueueWatch watch) { displayQueueWatch = watch; }

    synchronized void setCompositorWatch(CompositorWatch watch) { compositorWatch = watch; }

    // screenNs: when the frame reached the screen, 0 when unknown.
    private void rendered(long ptsUs, long renderNs, long callbackReceivedNs, long screenNs) {
        Frame frame = pending.get(ptsUs);
        if (frame == null) return;
        frame.renderNs = renderNs;
        // Delivery is diagnostic only. Render latency and FPS use the platform timestamp.
        frame.callbackReceivedNs = callbackReceivedNs;
        boolean valid = frame.inputNs > 0 && frame.outputNs >= frame.inputNs && renderNs >= frame.outputNs;
        if (valid) {
            // Presented FPS and present-interval jitter use scanoutNs when known (Vulkan passes queuedNs as renderNs).
            long presentedNs = screenNs > 0 && screenNs >= frame.outputNs ? screenNs : renderNs;
            metrics.onFramePresented(presentedNs);
            if (frame.handoffNs == 0) {
                metrics.addSample(2, frame.outputNs, renderNs);
                metrics.addSample(3, clientStartNs(frame), renderNs);
            }
            latencyProbe.onFrameRendered(frame.receiveNs, renderNs);
            presentJitter.record(presentedNs);
            CompositorWatch cw = compositorWatch;
            if (cw != null) {
                cw.onRendered(frame.handoffNs, presentedNs);
            }
            DisplayQueueWatch watch = displayQueueWatch;
            if (watch != null && frame.handoffNs != 0 && renderNs >= frame.handoffNs) {
                watch.onRendered(renderNs - frame.handoffNs, renderNs);
                String event = watch.takeEvent();
                if (event != null) com.limelight.LimeLog.info(event);
            }
        }
        if (screenNs > 0 && frame.outputNs > 0 && screenNs >= frame.outputNs) {
            metrics.recordScanout(screenNs - frame.outputNs, screenNs);
        }
        finish(frame, valid ? "rendered" : "invalid_render_time");
    }

    /**
     * Decoded to on screen since the last call, as a line for the log, or null without frames. It is
     * not display wait: it adds the compositor's queueing and the panel to what display wait counts.
     */
    synchronized String takeScanoutLatency() {
        return metrics.takeScanoutLatency();
    }

    /**
     * The renderer dropped or replaced a frame it was given, so it never reached the screen:
     * counted as a present drop now, instead of when its report fails to arrive.
     */
    synchronized void onFrameReplaced(long ptsUs) {
        Frame frame = pending.get(ptsUs);
        if (frame != null) {
            if (frame.awaitingRender) metrics.countPresentDrop(frame.releaseNs);
            finish(frame, "replaced");
        }
    }

    synchronized void discard(long ptsUs, String reason) {
        Frame frame = pending.get(ptsUs);
        if (frame != null) {
            // Vulkan discovers missing display timing when polling after the first release.
            if ("render_unavailable".equals(reason)) metrics.markRenderCallbacksUnavailable();
            finish(frame, reason);
        }
    }

    synchronized void discardPending(String reason) {
        while (pending.oldest != null) {
            finish(pending.oldest, reason);
        }
        lastOutputNs = lastReleaseNs = 0;
        metrics.onDiscardPending();
        lastRenderRelease = null;
        presentJitter.clear();
    }

    /** Records present intervals only while the overlay shows them; switching starts an empty window. */
    synchronized void setPresentJitterEnabled(boolean enabled) {
        presentJitter.setEnabled(enabled);
    }

    /** The last present intervals counted in vsyncs of vsyncNs, or null while disabled or empty. */
    synchronized PresentJitter.Snapshot presentJitter(long vsyncNs) {
        return presentJitter.snapshot(vsyncNs);
    }

    synchronized float takeDecodeTimeMs() {
        return metrics.takeDecodeTimeMs();
    }

    /**
     * Only new outputs since the last quality tick count. The existing ring bounds this scan;
     * observing it neither consumes overlay/CSV data nor allocates anything per decoded frame.
     */
    synchronized DecodeTiming takeDecodeTiming(long nowNs, int fps) {
        return metrics.takeDecodeTiming(nowNs, fps);
    }

    /** A reset/reconfigure must not carry evidence from the previous decoder workload. */
    synchronized void resetDecodeTiming() {
        metrics.resetDecodeTiming();
    }

    synchronized void onArtemisDecodeCounted(long ptsUs) {
        metrics.onArtemisDecodeCounted(ptsUs);
    }

    synchronized long getArtemisDecodeMs() {
        return metrics.getArtemisDecodeMs();
    }

    // Whole milliseconds counted since the last call; the caller divides by frames received.
    synchronized long takeArtemisDecodeMs() {
        return metrics.takeArtemisDecodeMs();
    }

    synchronized int getAverageDecoderLatency() {
        return metrics.getAverageDecoderLatency();
    }

    synchronized int getAverageEndToEndLatency() {
        return metrics.getAverageEndToEndLatency();
    }

    synchronized float[] getFrameRates(long nowNs) {
        return metrics.getFrameRates(nowNs);
    }

    synchronized long getPresentDrops() {
        return metrics.getPresentDrops();
    }

    /**
     * The share of the frames submitted for display in a settled FPS window that never reached the
     * screen, at most 100. A drop is mostly known only once its render callback timed out, so the
     * window is the FPS_WINDOW_NS of frames released up to CALLBACK_TIMEOUT_NS before the last
     * expire(): every frame in it is decided, and the drops (stamped at their release) and the frames
     * they are a share of cover the same time, whatever the frame rate did since. -1 without render
     * callbacks, where no drop can be seen, before such a window, or when the frame ring no longer
     * holds all of it.
     */
    synchronized float getDisplayDropPercent(long nowNs) {
        return metrics.getDisplayDropPercent(nowNs);
    }

    synchronized void expire(long nowNs) {
        metrics.noteExpire(nowNs);
        for (Frame frame = pending.oldest, next; frame != null; frame = next) {
            next = frame.newer;
            if (nowNs - (frame.awaitingRender ? frame.releaseNs : frame.inputNs) >= CALLBACK_TIMEOUT_NS) {
                // MediaCodec exposes no drop callback. Allow delayed render callbacks before estimating a drop.
                if (frame.awaitingRender) metrics.countPresentDrop(frame.releaseNs);
                finish(frame, frame.releaseNs != 0 ? "render_unobserved" : "output_unobserved");
            }
        }
    }

    private void finish(Frame frame, String status) {
        pending.remove(frame);
        if (frame.outputIndex != -1) {
            outputTable.remove(frame.outputIndex);
        }
        frame.status = status;
        Frame evicted = store.addCompleted(frame);
        if (evicted != null) {
            recycle(evicted);
        }
    }

    double[][] summarize() {
        return summarize(0, 0, false, false);
    }

    /** As summarize(), with each stage's median added as a fifth value: what the debug overlay shows. */
    double[][] summarizeWithMedian() {
        return summarize(0, 0, false, true);
    }

    // Unlike the overlay's last-N-frame summary, a diagnostic tick must not revive a stage
    // value from minutes ago simply because new frames arrived at a different stage.
    double[][] summarizeRecent(long nowNs, long windowNs) {
        return summarize(nowNs, Math.max(0, windowNs), true, false);
    }

    private double[][] summarize(long nowNs, long windowNs, boolean recentOnly, boolean median) {
        long[][] snapshot;
        synchronized (this) {
            snapshot = metrics.snapshotSamples(nowNs, windowNs, recentOnly);
        }
        return FrameLatencyMetrics.summarizeSnapshot(snapshot, median);
    }

    synchronized long getCsvRowsLost() {
        return store.getCsvRowsLost();
    }

    synchronized boolean hasCompletedFrames() {
        return store.hasCompletedFrames();
    }

    long writeCsv(Writer writer) throws IOException {
        return writeCsv(writer, null);
    }

    // The clock offset is read once per flush, before the lock: it calls into native code that
    // takes its own lock, and one value is plenty for the few hundred ms a flush covers.
    // Returns the number of characters written; the CSV is ASCII, so that is also its byte count.
    long writeCsv(Writer writer, ClockOffsetSource clockOffset) throws IOException {
        long[] clock = new long[2];
        String clockColumns = clockOffset != null && clockOffset.read(clock) ?
                clock[0] + "," + clock[1] : ",";
        long lost;
        long dropped;
        long sampleNs = System.nanoTime();
        float[] rates;
        csvDrain.clear();
        synchronized (this) {
            store.drainCompleted(csvDrain);
            lost = store.getCsvRowsLost();
            dropped = metrics.getPresentDrops();
            rates = metrics.getFrameRates(sampleNs);
        }
        // Disk I/O and formatting must never hold the lock used by the decoder threads.
        String frameRates = String.format(Locale.ROOT, "%.2f,%.2f,", rates[0], rates[1]) +
                (rates[2] < 0 ? "" : String.format(Locale.ROOT, "%.2f", rates[2]));
        // One buffer for all the rows of a flush: put together from pieces, a row made some fifty short strings.
        StringBuilder rows = csvRows;
        rows.setLength(0);
        for (int i = 0; i < csvDrain.size(); i++) {
            appendRow(rows, csvDrain.get(i), vrr, lost, dropped, sampleNs, frameRates, clockColumns);
        }
        long written = rows.length();
        if (written > 0) writer.append(rows);
        synchronized (this) {
            for (int i = 0; i < csvDrain.size(); i++) recycle(csvDrain.get(i));
        }
        csvDrain.clear();
        return written;
    }

    /** One frame's row with its line end. FrameLatencyCsvRowTest holds every character to what was written before. */
    static void appendRow(StringBuilder row, Frame frame, boolean vrr, long lost, long dropped, long sampleNs,
                          String frameRates, String clockColumns) {
        FrameLatencyCsv.appendRow(row, frame, vrr, lost, dropped, sampleNs, frameRates, clockColumns);
    }
}
