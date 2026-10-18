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

    @Test public void fractionalStreamsKeepTheirRateAcrossTenMinutesAndPanelPhases() {
        for (int milliHz : new int[] {59_940, 119_880}) {
            for (int panelHz : new int[] {60, 90, 120, 144}) {
                if (panelHz * 1000 < milliHz) continue;
                for (long phaseNs : new long[] {0, 2_000_000, 7_000_000}) {
                    long due = 0;
                    long interval = Math.round(1_000_000_000.0 / panelHz);
                    int rendered = 0;
                    for (int tick = 0; tick < panelHz * 600; tick++) {
                        long vsync = 1_000_000_000L + phaseNs + Math.round(tick * 1_000_000_000.0 / panelHz) +
                                (tick % 2 == 0 ? 5000 : -5000);
                        if (FramePacingPolicy.isFrameDue(due, vsync, interval)) {
                            rendered++;
                            due = FramePacingPolicy.nextFrameTimeNsMilliHz(due, vsync, milliHz);
                        }
                    }
                    assertEquals("stream " + milliHz + ", panel " + panelHz + ", phase " + phaseNs,
                            milliHz * 600 / 1000, rendered);
                }
            }
        }
    }

    @Test public void fractionalCadenceSkipsExpiredSlotsAfterAStallAndRestartsAfterRecovery() {
        long previousDue = 1_000_000_000L;
        long resumedVsync = 1_253_000_000L;
        long next = FramePacingPolicy.nextFrameTimeNsMilliHz(previousDue, resumedVsync, 59_940);
        assertTrue(next > resumedVsync);
        assertTrue(next - resumedVsync <= FramePacingPolicy.frameIntervalNs(59_940));
        assertEquals(resumedVsync + FramePacingPolicy.frameIntervalNs(119_880),
                FramePacingPolicy.nextFrameTimeNsMilliHz(0, resumedVsync, 119_880));
    }

    @Test public void fractionalHintCannotOverrideANegotiatedOrCappedIntegerRate() {
        assertEquals(59_940, FramePacingPolicy.negotiatedFrameRateMilliHz(59_940, 60));
        assertEquals(119_880, FramePacingPolicy.negotiatedFrameRateMilliHz(119_880, 120));
        assertEquals(59_000, FramePacingPolicy.negotiatedFrameRateMilliHz(59_940, 59));
        assertEquals(30_000, FramePacingPolicy.negotiatedFrameRateMilliHz(59_940, 30));
        assertEquals(60_000, FramePacingPolicy.negotiatedFrameRateMilliHz(0, 60));
    }

    @Test public void smoothModesRetainOrdinaryFramesAndRejectUnreliableAges() {
        long now = 1_000_000_000L;
        for (int mode : new int[] {FRAME_PACING_CAP_FPS, FRAME_PACING_MAX_SMOOTHNESS}) {
            long period = FramePacingPolicy.frameIntervalNs(60_000);
            for (long enqueue : new long[] {0, -1, now + 1, now, now - 16_000_000, now - period}) {
                assertFalse(FramePacingPolicy.shouldDrainOutput(mode, enqueue, now, 60_000, 0));
            }
            assertTrue(FramePacingPolicy.shouldDrainOutput(mode, now - period - 1, now, 60_000, 0));
            // The jitter budget follows the negotiated rate, including fractional frame periods.
            for (int milliHz : new int[] {24_000, 59_940, 119_880, 240_000}) {
                long interval = FramePacingPolicy.frameIntervalNs(milliHz);
                assertFalse(FramePacingPolicy.shouldDrainOutput(mode, now - interval, now, milliHz, 0));
                assertTrue(FramePacingPolicy.shouldDrainOutput(mode, now - interval - 1, now, milliHz, 0));
            }
            // Extremely low-rate streams retain the finite ceiling.
            assertTrue(FramePacingPolicy.shouldDrainOutput(mode, now - 250_000_001, now, 1000, 0));
        }
    }

    @Test public void catchupBudgetUsesOneExactFramePeriodAndRejectsReversedTime() {
        for (int milliHz : new int[] {1000, 24_000, 59_940, 120_000, 240_000}) {
            long period = FramePacingPolicy.frameIntervalNs(milliHz);
            assertFalse(FramePacingPolicy.hasOutputDrainTimeRemaining(-1, milliHz));
            assertTrue(FramePacingPolicy.hasOutputDrainTimeRemaining(0, milliHz));
            assertTrue(FramePacingPolicy.hasOutputDrainTimeRemaining(period - 1, milliHz));
            assertFalse(FramePacingPolicy.hasOutputDrainTimeRemaining(period, milliHz));
            assertFalse(FramePacingPolicy.hasOutputDrainTimeRemaining(period + 1, milliHz));
        }
    }

}
