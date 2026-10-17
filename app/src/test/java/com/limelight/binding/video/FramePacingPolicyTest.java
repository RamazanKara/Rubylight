package com.limelight.binding.video;

import org.junit.Test;
import static com.limelight.preferences.PreferenceConfiguration.*;
import static org.junit.Assert.*;

public class FramePacingPolicyTest {
    @Test public void pyrowavePreservesAllRequestedModesUnlessVariableRefreshIsEnabled() {
        for (int mode : new int[] {FRAME_PACING_MIN_LATENCY, FRAME_PACING_BALANCED,
                FRAME_PACING_CAP_FPS, FRAME_PACING_MAX_SMOOTHNESS}) {
            assertEquals(mode, FramePacingPolicy.pyroWaveMode(mode, false));
            assertEquals(FRAME_PACING_MIN_LATENCY, FramePacingPolicy.pyroWaveMode(mode, true));
        }
    }

    @Test public void onlyPacedLocalOutputUsesTheFixedDisplayClock() {
        for (int mode : new int[] {FRAME_PACING_MIN_LATENCY, FRAME_PACING_BALANCED,
                FRAME_PACING_CAP_FPS, FRAME_PACING_MAX_SMOOTHNESS}) {
            assertEquals(mode != FRAME_PACING_MIN_LATENCY,
                    FramePacingPolicy.usesPyroWaveDisplayClock(mode, false, true));
            assertFalse(FramePacingPolicy.usesPyroWaveDisplayClock(mode, true, true));
            assertFalse(FramePacingPolicy.usesPyroWaveDisplayClock(mode, false, false));
            assertFalse(FramePacingPolicy.usesPyroWaveDisplayClock(mode, true, false));
        }
    }

    @Test public void smoothnessModesKeepQueuedFramesAndLatencyModesDropStaleFrames() {
        assertFalse(FramePacingPolicy.dropQueuedFrames(FRAME_PACING_MAX_SMOOTHNESS));
        assertFalse(FramePacingPolicy.dropQueuedFrames(FRAME_PACING_CAP_FPS));
        assertTrue(FramePacingPolicy.dropQueuedFrames(FRAME_PACING_MIN_LATENCY));
        assertTrue(FramePacingPolicy.dropQueuedFrames(FRAME_PACING_BALANCED));
    }

    @Test public void sixtyFpsOnNinetyHzDoesNotCollapseToFortyFiveFps() {
        long due = 0;
        int frames = 0;
        for (int tick = 0; tick < 900; tick++) {
            long vsync = 1_000_000_000L + Math.round(tick * 1_000_000_000.0 / 90);
            if (FramePacingPolicy.isFrameDue(due, vsync, 1_000_000_000L / 90)) {
                frames++;
                due = FramePacingPolicy.nextFrameTimeNs(due, vsync, 60);
            }
        }
        assertEquals(600, frames);
    }

    @Test public void lateCallbackSkipsMissedSlotsInsteadOfBuildingCatchUpQueue() {
        long due = FramePacingPolicy.nextFrameTimeNs(1_000_000_000L, 1_250_000_000L, 100);
        assertEquals(1_260_000_000L, due);
    }

    @Test public void fractionalPanelRatesPreserveStreamCadence() {
        long due = 0;
        int frames = 0;
        for (int tick = 0; tick < 1199; tick++) {
            long vsync = 1_000_000_000L + Math.round(tick * 1_000_000_000.0 / 119.88);
            if (FramePacingPolicy.isFrameDue(due, vsync, Math.round(1_000_000_000.0 / 119.88))) {
                frames++;
                due = FramePacingPolicy.nextFrameTimeNs(due, vsync, 60);
            }
        }
        assertEquals(600, frames);
    }

    @Test public void matchingPanelAndStreamRatesTolerateVsyncTimestampNoise() {
        for (int fps : new int[] {60, 120}) {
            long due = 0;
            long interval = 1_000_000_000L / fps;
            for (int tick = 0; tick < fps * 10; tick++) {
                long vsync = 1_000_000_000L + tick * interval + (tick % 2 == 0 ? 5000 : -5000);
                assertTrue("Skipped tick " + tick + " at " + fps + " FPS",
                        FramePacingPolicy.isFrameDue(due, vsync, interval));
                due = FramePacingPolicy.nextFrameTimeNs(due, vsync, fps);
            }
        }
    }

    @Test public void sixtyFpsOnOneTwentyHzStillRendersEveryOtherVsync() {
        long due = 0;
        int frames = 0;
        for (int tick = 0; tick < 1200; tick++) {
            long vsync = 1_000_000_000L + Math.round(tick * 1_000_000_000.0 / 120) +
                    (tick % 2 == 0 ? 5000 : -5000);
            if (FramePacingPolicy.isFrameDue(due, vsync, 1_000_000_000L / 120)) {
                frames++;
                due = FramePacingPolicy.nextFrameTimeNs(due, vsync, 60);
            }
        }
        assertEquals(600, frames);
    }
}
