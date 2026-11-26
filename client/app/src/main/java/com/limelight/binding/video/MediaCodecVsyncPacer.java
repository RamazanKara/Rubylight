package com.limelight.binding.video;

import java.util.Locale;

import androidx.annotation.RequiresApi;

import android.content.Context;
import android.hardware.display.DisplayManager;
import android.media.MediaCodec;
import android.media.MediaCodec.BufferInfo;
import android.os.Build;
import android.os.Handler;
import android.os.HandlerThread;
import android.os.Process;
import android.view.Choreographer;

import com.limelight.LimeLog;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.nvstream.jni.NightjarPolicyJni;
import com.limelight.preferences.PreferenceConfiguration;

/**
 * Manages Choreographer vsync tracking, Android 13 frame timelines, display
 * timing updates, fractional V-Sync hold grids, Balanced refresh slots, phase
 * lock reporting, and scheduled release pacing for {@link MediaCodecDecoderRenderer}.
 */
final class MediaCodecVsyncPacer {
    private final MediaCodecDecoderRenderer r;

    private long nextFrameTimeNanos;
    private int pacingFrameRateMilliHz;
    // Fractional V-Sync: native hold stamp state and the refresh grid the phase report aims at
    // (0 and 1 without a hold), set where a frame is released as it comes out (holdStampNs).
    final FramePacingPolicy.HoldStamp holdStamp = new FramePacingPolicy.HoldStamp();
    private final long[] heldReportGridOut = new long[2];
    private volatile long heldAnchorNs;
    /** The refreshes smoothDisplayNs last logged holding each frame for, 1 before it held any. Decoder thread. */
    private int smoothHoldLogged = 1;
    // Balanced at the panel's own rate: each thread owns its own reusable Slot; reset when the display changes.
    private final RefreshSlots.Slot refreshSlot = new RefreshSlots.Slot();
    private final RefreshSlots.Slot vsyncRefreshSlot = new RefreshSlots.Slot();
    private volatile boolean slotPlateauLogged;
    private volatile boolean slotSharedLogged;
    HandlerThread choreographerHandlerThread;
    private Handler choreographerHandler;
    /** The display's fixed presentation deadline, kept for the frame timeline log line. */
    private long reportedPresentationDeadlineNs;
    /** The Android 13 frame timeline callback ({@link FrameTimelineCallback}), created on first use. */
    private Object frameTimelineCallback;
    /** The frame timeline deadline last written to the log; -1 before the first. */
    private long loggedDeadlineLeadNs = -1;
    private DisplayManager displayManager;
    private long heldPtsUs;

    MediaCodecVsyncPacer(MediaCodecDecoderRenderer renderer) {
        this.r = renderer;
    }

    DisplayManager.DisplayListener createDisplayListener() {
        return new DisplayManager.DisplayListener() {
            @Override public void onDisplayAdded(int displayId) { }
            @Override public void onDisplayRemoved(int displayId) { }
            @Override public void onDisplayChanged(int displayId) {
                if (r.streamDisplay.getDisplayId() == displayId) updateDisplayTiming();
            }
        };
    }

    void resetPacingOnRecovery() {
        nextFrameTimeNanos = 0;
        pacingFrameRateMilliHz = 0;
    }

    static long nextVsyncTimeNs(long nowNs, long vsyncNs, long intervalNs, long presentationDeadlineNs) {
        // Before the first callback there is no display phase to align to.
        return NightjarPolicyJni.vsyncTiming(1, nowNs, vsyncNs, intervalNs, presentationDeadlineNs, 0, false);
    }

    // Phase lock: how long a frame ready at readyNs waits for the next latch deadline.
    static long latchSlackNs(long readyNs, long vsyncNs, long intervalNs, long presentationDeadlineNs) {
        return NightjarPolicyJni.vsyncTiming(2, readyNs, vsyncNs, intervalNs, presentationDeadlineNs, 0, false);
    }

    /** Changes only this stream's preference object, never the saved pacing setting. */
    boolean applyOutputPacingFallback() {
        // Vulkan has its own presentation clock, including actual external-display timestamps.
        // Keep its requested mode before negotiation; conventional fallback still uses this policy.
        boolean isPyroWave = (r.videoFormat & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) != 0;
        boolean forcePyroWave = r.videoFormat == 0 && r.prefs.videoFormat == PreferenceConfiguration.FormatOption.FORCE_PYROWAVE;
        if (!FramePacingPolicy.applyOutputPacingFallback(isPyroWave, forcePyroWave, r.outputVsyncAvailable, r.prefs.framePacing)) {
            return false;
        }
        r.prefs.framePacing = PreferenceConfiguration.FRAME_PACING_MIN_LATENCY;
        return true;
    }

    /**
     * Phase lock: reports the slack of a frame ready at readyNs, also with host frame times. The
     * scheduler's hold is the frame's lateness against its host time, which a capture the lock
     * moves keeps; only the vsync the hold lands on follows the new phase, and nothing it decides
     * changes readyNs. Without the report the capture phase drifted across the whole refresh.
     */
    void reportPhaseSlack(long readyNs) {
        // Adaptive refresh has no fixed latch; an upscaled frame is not display-ready yet.
        // hostPhaseLock is read once at the stream start: with the host not taking reports, the
        // rest, and the bridge's locks, would only run to be turned down
        if (r.stopping || !r.outputVsyncAvailable || !r.hostPhaseLock) return;
        if (r.upscaler != null) {
            r.telemetry.phaseLockOff = "upscaling";
            return;
        }
        // The scheduler gives each frame the first vsync its plan allows. With the cluster right at
        // the latch, ordinary jitter flips frames between two refreshes, so the host keeps it a
        // little earlier there.
        long roomNs = FramePacingPolicy.schedulesFrames(r.hostFrameTimes, r.prefs.framePacing) ?
                MediaCodecDecoderRenderer.SCHEDULED_LATCH_ROOM_NS : 0;
        long slackNs, intervalNs;
        // Fractional V-Sync: slack to the refresh each frame's turn comes at, over its hold.
        long anchorNs = heldAnchorNs;
        int refreshes = r.heldAnchorRefreshes;
        synchronized (r.vsyncMonitor) {
            if (adaptiveInUse()) {
                r.telemetry.phaseLockOff = "adaptive screen";
                return;
            }
            FramePacingPolicy.heldReportGrid(r.vsyncTimeNs, r.vsyncIntervalNs, anchorNs, refreshes, heldReportGridOut);
            intervalNs = heldReportGridOut[1];
            slackNs = latchSlackNs(readyNs, heldReportGridOut[0], intervalNs, r.vsyncPresentationDeadlineNs + roomNs);
        }
        if (slackNs >= 0 && MoonBridge.reportFrameSlack(slackNs, intervalNs)) {
            r.phaseReports++;
            Handler handler = r.latencyHandler;
            if (handler != null && r.telemetry.phaseFlushQueued.compareAndSet(false, true) &&
                    !handler.post(r.telemetry.flushPhaseLockReport)) {
                r.telemetry.phaseFlushQueued.set(false);
            }
        }
    }

    void updateDisplayTiming() {
        // ARR is a property of the active output mode, not the saved VRR preference.
        // A fold or mode change can invalidate a fixed-refresh latch estimate.
        boolean arrSupported = Build.VERSION.SDK_INT >= Build.VERSION_CODES.BAKLAVA &&
                r.streamDisplay.hasArrSupport();
        float physicalRefreshRate = r.streamDisplay.getMode().getRefreshRate();
        r.reconfigure.updateUpscalingFrameRate(physicalRefreshRate);
        synchronized (r.vsyncMonitor) {
            r.outputHasArrSupport = arrSupported;
            r.vsyncIntervalNs = (long) (1000000000.0 / physicalRefreshRate);
            r.vsyncPresentationDeadlineNs = r.streamDisplay.getPresentationDeadlineNanos();
            reportedPresentationDeadlineNs = r.vsyncPresentationDeadlineNs;
            r.appVsyncOffsetNs = r.streamDisplay.getAppVsyncOffsetNanos();
            // Wait for a sample from the new mode before reporting its phase.
            r.vsyncTimeNs = 0;
            r.refreshSlots.reset();
            r.pyroWaveDisplayClock.reset(r.vsyncIntervalNs);
        }
        r.compositorWatch.resetWindow();
    }

    void doFrame(long frameTimeNanos) {
        onVsync(frameTimeNanos, 0, 0, -1);
    }

    /**
     * Android 13 and newer: the vsync with SurfaceFlinger's own timeline for it. The grid anchor
     * and the latch deadline come from the preferred frame timeline instead of the display's
     * fixed offsets (see {@link VsyncTimeline}); values that cannot be right keep those.
     */
    void onFrameTimeline(long frameTimeNanos, long expectedPresentNs, long deadlineNs) {
        long intervalNs;
        synchronized (r.vsyncMonitor) {
            intervalNs = r.vsyncIntervalNs;
        }
        long anchorNs = VsyncTimeline.anchorNs(expectedPresentNs, frameTimeNanos, intervalNs);
        // A presentation time that cannot be right says nothing about the deadline either.
        long leadNs = anchorNs > 0 ? VsyncTimeline.deadlineLeadNs(expectedPresentNs, deadlineNs, intervalNs) : -1;
        onVsync(frameTimeNanos, anchorNs, anchorNs > 0 ? expectedPresentNs : 0, leadNs);
    }

    /** One vsync: anchorNs (0 = frameTime - appVsyncOffset), expectedPresentNs (0 = none), deadlineLeadNs (< 0 = keep fixed). */
    private void onVsync(long frameTimeNanos, long anchorNs, long expectedPresentNs, long deadlineLeadNs) {
        if (r.stopping || !r.outputVsyncAvailable) return;

        long callbackVsyncNs, reportedNs;
        synchronized (r.vsyncMonitor) {
            // Keep Choreographer cadence monotonic even when VsyncTimeline.anchorNs steps by a refresh.
            callbackVsyncNs = frameTimeNanos - r.appVsyncOffsetNs;
            r.vsyncTimeNs = anchorNs > 0 ? anchorNs : callbackVsyncNs;
            if (deadlineLeadNs >= 0) r.vsyncPresentationDeadlineNs = deadlineLeadNs;
            reportedNs = reportedPresentationDeadlineNs;
        }
        if (deadlineLeadNs >= 0 && VsyncTimeline.worthLogging(loggedDeadlineLeadNs, deadlineLeadNs)) {
            loggedDeadlineLeadNs = deadlineLeadNs;
            LimeLog.info(String.format(Locale.US,
                    "Vsync timeline: buffers due %.2f ms before their vsync (the display reports %.2f ms)",
                    deadlineLeadNs / 1e6, reportedNs / 1e6));
        }
        if ((r.videoFormat & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) != 0 &&
                FramePacingPolicy.usesPyroWaveDisplayClock(r.prefs.framePacing, r.prefs.vrr, r.outputVsyncAvailable)) {
            // This only publishes immutable Java values. No Vulkan call or renderer lock can
            // delay the display callback or race native teardown.
            r.pyroWaveDisplayClock.observe(expectedPresentNs > 0 ? expectedPresentNs : callbackVsyncNs);
        }
        if (r.prefs.framePacing != PreferenceConfiguration.FRAME_PACING_BALANCED ||
                (r.videoFormat & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) != 0) {
            // Other modes render from the decoder thread; this callback only tracks vsync.
            postVsyncCallback();
            return;
        }

        // Only the Choreographer thread advances the cadence. A rate change starts a new
        // timeline here instead of racing its deadline from a connection/UI callback.
        int fpsMilliHz = r.streamFrameRateMilliHz;
        if (pacingFrameRateMilliHz != fpsMilliHz) {
            pacingFrameRateMilliHz = fpsMilliHz;
            nextFrameTimeNanos = 0;
        }

        // Don't render unless a new frame is due. This prevents microstutter when streaming
        // at a frame rate that doesn't match the display (such as 60 FPS on 120 Hz): on a whole
        // multiple each frame gets the same number of refreshes, as fractional V-Sync holds them,
        // so the setting does nothing here (SettingRules.fractionalVsyncApplies). Frames from this
        // queue send no phase report (reportPhaseSlack runs for frames released as they come out).
        if (FramePacingPolicy.isFrameDue(nextFrameTimeNanos, callbackVsyncNs, r.vsyncIntervalNs)) {
            int nextOutputBuffer;
            synchronized (r.outputBufferQueue) {
                nextOutputBuffer = r.outputBufferQueue.poll();
            }
            if (nextOutputBuffer != IntRing.EMPTY) {
                long releaseNs = System.nanoTime();
                try {
                    if (r.framePresenter.displayQueueDropDue(releaseNs)) {
                        // The cadence goes on as if it was shown: the screen keeps the previous frame.
                        r.framePresenter.dropForDisplayQueue(nextOutputBuffer, releaseNs);
                    } else {
                        long displayAtNs = planRefreshSlot(releaseNs, vsyncRefreshSlot, false);
                        long handoffAtNs = vsyncRefreshSlot.vsyncNs != 0 ?
                                releaseNs + vsyncRefreshSlot.holdNs : displayAtNs;
                        r.framePresenter.countRenderRelease(nextOutputBuffer, releaseNs, handoffAtNs);
                        r.framePresenter.releaseOutputFrame(nextOutputBuffer, displayAtNs);
                        r.activeWindowVideoStats.totalFramesRendered++;
                    }
                    nextFrameTimeNanos = FramePacingPolicy.nextFrameTimeNsMilliHz(
                            nextFrameTimeNanos, callbackVsyncNs, fpsMilliHz);
                } catch (IllegalStateException ignored) {
                    // The frame was counted as submitted before the release threw.
                    r.frameLatencyStats.onReleaseFailed(releaseNs);
                    try {
                        // Try to avoid leaking the output buffer by releasing it without rendering
                        r.videoDecoder.releaseOutputBuffer(nextOutputBuffer, false);
                    } catch (IllegalStateException e) {
                        // This will leak nextOutputBuffer, but there's really nothing else we can do
                        e.printStackTrace();
                        r.recovery.handleDecoderException(e);
                    }
                }
            }
        }

        // Attempt codec recovery even if we have nothing to render right now. Recovery can still
        // be required even if the codec died before giving any output.
        r.recovery.doCodecRecoveryIfRequired(MediaCodecDecoderRenderer.CR_FLAG_CHOREOGRAPHER);

        // Request another callback for next frame
        postVsyncCallback();
    }

    /** Asks for the next vsync, with its frame timeline on Android 13 and newer. Choreographer thread only. */
    private void postVsyncCallback() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            if (frameTimelineCallback == null) frameTimelineCallback = FrameTimelineCallback.create(r);
            FrameTimelineCallback.post(frameTimelineCallback);
        } else {
            Choreographer.getInstance().postFrameCallback(r);
        }
    }

    /** Cancels the vsync request. Choreographer thread only. */
    private void removeVsyncCallback() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU && frameTimelineCallback != null) {
            FrameTimelineCallback.remove(frameTimelineCallback);
        }
        Choreographer.getInstance().removeFrameCallback(r);
    }

    /** Android 13's vsync callback, kept apart so older versions never load its types. */
    @RequiresApi(Build.VERSION_CODES.TIRAMISU)
    private static final class FrameTimelineCallback implements Choreographer.VsyncCallback {
        private final MediaCodecDecoderRenderer renderer;

        private FrameTimelineCallback(MediaCodecDecoderRenderer renderer) {
            this.renderer = renderer;
        }

        static Object create(MediaCodecDecoderRenderer renderer) {
            return new FrameTimelineCallback(renderer);
        }

        static void post(Object callback) {
            Choreographer.getInstance().postVsyncCallback((FrameTimelineCallback) callback);
        }

        static void remove(Object callback) {
            Choreographer.getInstance().removeVsyncCallback((FrameTimelineCallback) callback);
        }

        @Override
        public void onVsync(Choreographer.FrameData data) {
            Choreographer.FrameTimeline timeline = data.getPreferredFrameTimeline();
            renderer.onFrameTimeline(data.getFrameTimeNanos(), timeline.getExpectedPresentationTimeNanos(),
                    timeline.getDeadlineNanos());
        }
    }

    void startChoreographerThread() {
        // Local output tracks vsync for phase-lock reports; only balanced renders here.
        // External output still observes mode changes for upscaling, but cannot use
        // the internal display's callback as an external pacing or phase signal.
        // We use a separate thread to avoid any main thread delays from delaying rendering
        choreographerHandlerThread = new HandlerThread("Video - Choreographer", Process.THREAD_PRIORITY_DISPLAY);
        choreographerHandlerThread.start();

        r.streamDisplay = r.outputDisplay;
        displayManager = (DisplayManager) r.context.getSystemService(Context.DISPLAY_SERVICE);

        // Start the frame callbacks
        choreographerHandler = new Handler(choreographerHandlerThread.getLooper());
        choreographerHandler.post(new Runnable() {
            @Override
            public void run() {
                displayManager.registerDisplayListener(r.displayListener, choreographerHandler);
                updateDisplayTiming();
                if (r.outputVsyncAvailable) {
                    postVsyncCallback();
                }
            }
        });
    }

    void quitChoreographerThread() {
        // Post a quit message to the Choreographer looper (if we have one)
        if (choreographerHandler != null) {
            choreographerHandler.post(new Runnable() {
                @Override
                public void run() {
                    displayManager.unregisterDisplayListener(r.displayListener);
                    // Don't allow any further messages to be queued
                    choreographerHandlerThread.quit();

                    // Deregister the frame callback (if registered)
                    if (r.outputVsyncAvailable) {
                        removeVsyncCallback();
                    }
                }
            });
        }
    }

    /** Whether the stream's screen is timed as adaptive (DisplayFrameRatePolicy.adaptiveInUse). Under vsyncMonitor. */
    boolean adaptiveInUse() {
        return DisplayFrameRatePolicy.adaptiveInUse(r.outputHasArrSupport, r.prefs.vrr, r.prefs.arrFixedTiming);
    }

    /** How often the game presents: the measured content rate once steady, else the stream's. */
    long contentIntervalNs() {
        int milliHz = r.hostFrameClock.contentRateMilliHz();
        return milliHz >= 1000 ? 1_000_000_000_000L / milliHz : FramePacingPolicy.frameIntervalNs(r.streamFrameRateMilliHz);
    }

    int heldRefreshes() {
        return heldRefreshes(r.prefs.fractionalVsync);
    }

    private int heldRefreshes(boolean fractional) {
        long intervalNs;
        boolean arr;
        synchronized (r.vsyncMonitor) {
            intervalNs = r.vsyncIntervalNs;
            arr = adaptiveInUse();
        }
        if (intervalNs <= 0) return 1;
        return FramePacingPolicy.refreshesPerFrame(1e9 / intervalNs, r.streamFrameRateMilliHz / 1000.0,
                vrrForHolds(arr), fractional);
    }

    private boolean vrrForHolds(boolean arr) {
        return r.prefs.vrr || arr;
    }

    boolean holdsNothingForVrr() {
        boolean arr;
        synchronized (r.vsyncMonitor) {
            arr = adaptiveInUse();
        }
        return vrrForHolds(arr);
    }

    long heldDisplayNs(long releaseNs) {
        long stampNs = holdStampNs(releaseNs, heldRefreshes(), false);
        return stampNs != 0 ? stampNs : releaseNs;
    }

    long smoothDisplayNs(long releaseNs) {
        return holdStampNs(releaseNs, heldRefreshes(true), true);
    }

    private long holdStampNs(long releaseNs, int refreshes, boolean everyFrame) {
        long vsyncNs, intervalNs, deadlineNs;
        synchronized (r.vsyncMonitor) {
            vsyncNs = r.vsyncTimeNs;
            intervalNs = r.vsyncIntervalNs;
            deadlineNs = r.vsyncPresentationDeadlineNs;
        }
        if (everyFrame && refreshes > 1 && vsyncNs != 0 && intervalNs > 0 && r.outputVsyncAvailable &&
                refreshes != smoothHoldLogged) {
            smoothHoldLogged = refreshes;
            LimeLog.info(String.format(Locale.US,
                    "Smooth pacing: each frame waits for its turn, %d refreshes of %.2f ms", refreshes, intervalNs / 1e6));
        }
        if (!holdStamp.step(releaseNs, vsyncNs, intervalNs, deadlineNs, refreshes, everyFrame, r.outputVsyncAvailable) ||
                holdStamp.out[0] == 0) {
            r.heldAnchorRefreshes = 1;
            return 0;
        }
        heldAnchorNs = holdStamp.out[1];
        r.heldAnchorRefreshes = (int) holdStamp.out[2];
        return holdStamp.out[0];
    }

    boolean keepsBurstForRefreshSlot(long nowNs) {
        if (r.prefs.framePacing != PreferenceConfiguration.FRAME_PACING_BALANCED || r.hostFrameTimes) {
            return false;
        }
        long vsyncNs, intervalNs, deadlineNs;
        boolean grid;
        synchronized (r.vsyncMonitor) {
            grid = r.refreshSlotsEnabled && r.outputVsyncAvailable && r.upscaler == null && !r.prefs.vrr &&
                    r.vsyncTimeNs != 0 && r.vsyncIntervalNs > 0;
            vsyncNs = r.vsyncTimeNs;
            intervalNs = r.vsyncIntervalNs;
            deadlineNs = r.vsyncPresentationDeadlineNs;
        }
        int fpsMilliHz = r.streamFrameRateMilliHz;
        if (!grid || fpsMilliHz < 1000 || fpsMilliHz > 1_000_000 ||
                !RefreshSlots.applies(FramePacingPolicy.frameIntervalNs(fpsMilliHz), intervalNs)) {
            return false;
        }
        return !r.refreshSlots.earliestClaimed(nowNs, vsyncNs, intervalNs, deadlineNs);
    }

    long refreshSlotHandoffNs(long releaseNs, long displayAtNs) {
        return refreshSlot.vsyncNs != 0 ? releaseNs + refreshSlot.holdNs : displayAtNs;
    }

    long planRefreshSlot(long releaseNs) {
        return planRefreshSlot(releaseNs, refreshSlot, true);
    }

    private long planRefreshSlot(long releaseNs, RefreshSlots.Slot slot, boolean allowHold) {
        long vsyncNs, intervalNs, deadlineNs;
        boolean grid;
        synchronized (r.vsyncMonitor) {
            // VRR off votes a fixed-source refresh even on Android 16 ARR panels; FrameData gives live timing.
            grid = r.refreshSlotsEnabled && r.outputVsyncAvailable && r.upscaler == null && !r.prefs.vrr &&
                    r.vsyncTimeNs != 0 && r.vsyncIntervalNs > 0;
            vsyncNs = r.vsyncTimeNs;
            intervalNs = r.vsyncIntervalNs;
            deadlineNs = r.vsyncPresentationDeadlineNs;
        }
        int fpsMilliHz = r.streamFrameRateMilliHz;
        if (!grid || fpsMilliHz < 1000 || fpsMilliHz > 1_000_000 ||
                !RefreshSlots.applies(FramePacingPolicy.frameIntervalNs(fpsMilliHz), intervalNs)) {
            r.refreshSlots.reset();
            slot.vsyncNs = 0;
            slot.shared = false;
            slot.holdNs = 0;
            return allowHold ? heldDisplayNs(releaseNs) : releaseNs;
        }
        r.refreshSlots.plan(releaseNs, vsyncNs, intervalNs, deadlineNs, slot);
        if (!slotPlateauLogged) {
            slotPlateauLogged = true;
            LimeLog.info(String.format(Locale.US,
                    "Balanced: each output takes the next free refresh, one per refresh (%.2f ms, due %.2f ms before it)",
                    intervalNs / 1e6, (intervalNs / 2) / 1e6));
        }
        if (slot.shared && !slotSharedLogged) {
            slotSharedLogged = true;
            LimeLog.info("Balanced: a surplus output shares the refresh already claimed, replacing the older buffer there");
        }
        return slot.targetNs;
    }

    /** Balanced, see FramePacingPolicy.balancedPacesOnVsync. Host frame times keep the vsync callback. */
    boolean balancedPacesOnVsync() {
        if (r.hostFrameTimes) return true;
        long intervalNs;
        synchronized (r.vsyncMonitor) {
            if (!r.outputVsyncAvailable) return true;
            intervalNs = r.vsyncIntervalNs;
        }
        return FramePacingPolicy.balancedPacesOnVsync(r.hostPhaseLock, r.prefs.dropLateFrames,
                r.streamFrameRateMilliHz, intervalNs);
    }

    ScheduledRelease.Port createReleasePort() {
        return new ReleasePort();
    }

    /** The scheduled release's view of the codec, the clock and the display. */
    private final class ReleasePort implements ScheduledRelease.Port {
        private final BufferInfo info = new BufferInfo();

        @Override public long nanoTime() {
            return System.nanoTime();
        }

        @Override public void timing(ScheduledRelease.Timing out) {
            synchronized (r.vsyncMonitor) {
                out.grid = r.outputVsyncAvailable && r.upscaler == null && !adaptiveInUse() &&
                        r.vsyncTimeNs != 0 && r.vsyncIntervalNs > 0;
                out.vsyncNs = r.vsyncTimeNs;
                out.intervalNs = r.vsyncIntervalNs;
                out.deadlineNs = r.vsyncPresentationDeadlineNs;
            }
        }

        @Override public long heldPtsUs() {
            return heldPtsUs;
        }

        /** The caller has checked that the held frame may be dropped (the previous one was shown). */
        @Override public boolean replaceHeld(String reason, long basisNs) {
            int newer = r.videoDecoder.dequeueOutputBuffer(info, 0);
            if (newer == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED) {
                r.framePresenter.onOutputFormatChanged();
                return false;
            }
            if (newer < 0) return false;
            long nowNs = System.nanoTime();
            r.framePresenter.noteDecoderOutput(newer, info.presentationTimeUs, nowNs);
            r.frameLatencyStats.onOutputDropped(r.heldIndex, reason, nowNs, r.framePresenter.reportsRenderTimes());
            r.videoDecoder.releaseOutputBuffer(r.heldIndex, false);
            r.numFramesOut++;
            r.heldIndex = newer;
            heldPtsUs = info.presentationTimeUs;
            r.framePresenter.noteResidency(r.heldIndex);
            return true;
        }

        @Override public long fallbackPresentNs(long codecPtsUs, long basisNs) {
            return r.hostFrameClock.presentNs(codecPtsUs, basisNs);
        }

        @Override public void release(long releaseNs, long dueNs, boolean planned, long holdNs, boolean late,
                                      int inFlightCount) {
            long nowNs = System.nanoTime();
            if (r.upscaler == null) {
                r.frameLatencyStats.onScheduledHandedOff(r.heldIndex, planned ? dueNs : 0, holdNs, late, inFlightCount,
                        nowNs, releaseNs, r.framePresenter.reportsRenderTimes());
            } else {
                r.frameLatencyStats.onScheduled(r.heldIndex, planned ? dueNs : 0, holdNs, late, inFlightCount);
                r.framePresenter.countRenderRelease(r.heldIndex, nowNs, releaseNs);
            }
            r.framePresenter.releaseOutputFrame(r.heldIndex, releaseNs);
        }
    }

    long releaseScheduled(int index, long ptsUs, boolean droppedBefore) {
        r.heldIndex = index;
        heldPtsUs = ptsUs;
        boolean renderCallbacks = r.framePresenter.reportsRenderTimes();
        return r.scheduledRelease.run(droppedBefore, contentIntervalNs(),
                renderCallbacks && r.session.frameRenderedListenerSet);
    }
}
