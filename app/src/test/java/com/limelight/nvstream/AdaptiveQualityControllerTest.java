package com.limelight.nvstream;

import org.junit.Test;
import static org.junit.Assert.*;

public class AdaptiveQualityControllerTest {
    private DecodeTiming timing(long nowMs, int fps, int samples, int late) {
        long end = nowMs * 1_000_000L;
        return new DecodeTiming(0, end - 1_000_000_000L, end,
                end - 950_000_000L, end - 50_000_000L, fps, samples, late);
    }

    private AdaptiveQualityController.Target tail(AdaptiveQualityController quality, long now,
                                                  DecodeTiming timing) {
        return quality.sample(now, true, false, 0, 2, 0, 0, 0, true, timing);
    }

    @Test public void sustainedLateFramesReduceQualityDespiteLowMeanAndAvailableBitrate() {
        AdaptiveQualityController quality = controller(AdaptiveQualityController.Mode.MOTION);
        for (long now = 20000; now < 23000; now += 1000) {
            assertNull(tail(quality, now, timing(now, 120, 120, 12)));
        }
        AdaptiveQualityController.Target lower = tail(quality, 23000, timing(23000, 120, 120, 12));
        assertNotNull(lower);
        assertEquals(120, lower.fps);
        assertTrue(lower.height < 1080);
        quality.applied(lower, 23000);
        for (long now = 24000; now < 38000; now += 1000) {
            assertNull(tail(quality, now, timing(now, 120, 120, 12)));
        }
        assertNotNull(tail(quality, 38000, timing(38000, 120, 120, 12)));
    }

    @Test public void isolatedKeyframesBurstsAndRepeatedOrOverlappingWindowsNeverAccumulatePressure() {
        for (int scenario = 0; scenario < 4; scenario++) {
            AdaptiveQualityController quality = controller(AdaptiveQualityController.Mode.MOTION);
            for (long now = 20000; now < 60000; now += 1000) {
                DecodeTiming sample;
                if (scenario == 0) sample = timing(now, 120, 120, 1);
                else if (scenario == 1) sample = timing(now, 120, 120, now == 20000 ? 60 : 0);
                else if (scenario == 2) sample = timing(20000, 120, 120, 60);
                else {
                    long end = now * 1_000_000L;
                    sample = new DecodeTiming(0, end - 2_000_000_000L, end,
                            end - 1_500_000_000L, end - 50_000_000L, 120, 120, 60);
                }
                assertNull("scenario " + scenario, tail(quality, now, sample));
            }
        }
    }

    @Test public void sparseStaleInvalidAndWrongFpsObservationsAreUnknown() {
        for (int scenario = 0; scenario < 7; scenario++) {
            AdaptiveQualityController quality = controller(AdaptiveQualityController.Mode.MOTION);
            for (long now = 20000; now < 60000; now += 1000) {
                long end = now * 1_000_000L;
                DecodeTiming sample;
                switch (scenario) {
                    case 0: sample = timing(now, 120, 29, 20); break;
                    case 1: sample = new DecodeTiming(0, end - 1_000_000_000L, end,
                            end - 950_000_000L, end - 600_000_000L, 120, 120, 60); break;
                    case 2: sample = new DecodeTiming(0, end - 1_000_000_000L, end,
                            end - 400_000_000L, end - 50_000_000L, 120, 120, 60); break;
                    case 3: sample = timing(now, 60, 120, 60); break;
                    case 4: sample = timing(now, 120, 120, 121); break;
                    case 5: sample = new DecodeTiming(0, end - 1_000_000_000L, end,
                            end - 950_000_000L, end + 1, 120, 120, 60); break;
                    default: sample = null;
                }
                assertNull("scenario " + scenario, tail(quality, now, sample));
            }
        }
    }

    @Test public void recoveryResetSuspendAndLongTickGapsBreakThePressureStreak() {
        for (int scenario = 0; scenario < 3; scenario++) {
            AdaptiveQualityController quality = controller(AdaptiveQualityController.Mode.MOTION);
            for (long now = 20000; now < 23000; now += 1000) {
                assertNull(tail(quality, now, timing(now, 120, 120, 12)));
            }
            if (scenario == 0) quality.suspend();
            long next = scenario == 1 ? 26000 : 23000;
            DecodeTiming sample = timing(next, 120, 120, 12);
            if (scenario == 2) sample = new DecodeTiming(1, sample.windowStartNs, sample.windowEndNs,
                    sample.firstSampleNs, sample.lastSampleNs, sample.fps, sample.samples, sample.lateFrames);
            assertNull(tail(quality, next, sample));
        }
    }

    @Test public void tailRecoveryNeedsThirtyFreshHealthySecondsAndCannotOscillateInTheDeadband() {
        AdaptiveQualityController quality = controller(AdaptiveQualityController.Mode.MOTION);
        quality.applied(hot(quality, 15000), 18000);
        for (long now = 19000; now < 60000; now += 1000) {
            assertNull(tail(quality, now, timing(now, 120, 100, 3)));
        }
        for (long now = 60000; now < 90000; now += 1000) {
            assertNull(tail(quality, now, timing(now, 120, 100, 2)));
        }
        assertNotNull(tail(quality, 90000, timing(90000, 120, 100, 2)));
        // Unknown timing cannot certify recovery, even though the mean remains low.
        quality.suspend();
        for (long now = 91000; now < 150000; now += 1000) {
            assertNull(tail(quality, now, null));
        }
    }

    @Test public void missingTailDoesNotDisableThermalOrMeanDecodeProtection() {
        for (boolean heat : new boolean[] {false, true}) {
            AdaptiveQualityController quality = controller(AdaptiveQualityController.Mode.MOTION);
            for (long now = 20000; now <= 23000; now += 1000) {
                AdaptiveQualityController.Target target = quality.sample(now, true, false, 0,
                        heat ? 2 : 12, 0, 0, heat ? 2 : 0, true, null);
                if (now < 23000) assertNull(target);
                else assertNotNull(target);
            }
        }
    }

    @Test public void decoderGenerationChangeAlsoDiscardsOldMeanPressureVotes() {
        AdaptiveQualityController quality = controller(AdaptiveQualityController.Mode.MOTION);
        for (long now = 20000; now < 23000; now += 1000) {
            assertNull(quality.sample(now, true, false, 0, 12, 0, 0, 0, true,
                    timing(now, 120, 120, 0)));
        }
        DecodeTiming next = timing(23000, 120, 120, 0);
        next = new DecodeTiming(1, next.windowStartNs, next.windowEndNs, next.firstSampleNs,
                next.lastSampleNs, next.fps, next.samples, next.lateFrames);
        assertNull(quality.sample(23000, true, false, 0, 12, 0, 0, 0, true, next));
    }

    @Test public void lowFrameRateStillRequiresSeveralLateFramesAndNormalFloors() {
        AdaptiveQualityController quality = new AdaptiveQualityController(AdaptiveQualityController.Mode.DETAIL,
                1920, 1080, 30, 720, 15, 2, 2, 0);
        for (long now = 20000; now < 23000; now += 1000) {
            assertNull(tail(quality, now, timing(now, 30, 30, 3)));
        }
        AdaptiveQualityController.Target lower = tail(quality, 23000, timing(23000, 30, 30, 3));
        assertNotNull(lower);
        assertEquals(15, lower.fps);
        quality.applied(lower, 23000);
        for (long now = 24000; now < 40000; now += 1000) {
            assertNull(tail(quality, now, timing(now, 15, 15, 1)));
        }
        for (long now = 40000; now < 43000; now += 1000) {
            assertNull(tail(quality, now, timing(now, 15, 15, 3)));
        }
        lower = tail(quality, 43000, timing(43000, 15, 15, 3));
        assertNotNull(lower);
        assertEquals(15, lower.fps);
        assertTrue(lower.height >= 720);
    }

    private AdaptiveQualityController controller(AdaptiveQualityController.Mode mode) {
        return new AdaptiveQualityController(mode, 1920, 1080, 120, 720, 30, 2, 2, 0);
    }

    private AdaptiveQualityController.Target hot(AdaptiveQualityController controller, long now) {
        AdaptiveQualityController.Target result = null;
        for (int i = 0; i < 4; i++) result = controller.sample(now + i * 1000, true, false, 0, 2, 0, 0, 2, true);
        return result;
    }

    @Test public void motionLowersResolutionFirstAndDetailLowersFrameRateFirst() {
        AdaptiveQualityController motion = controller(AdaptiveQualityController.Mode.MOTION);
        AdaptiveQualityController.Target m = hot(motion, 15000);
        assertNotNull(m);
        assertEquals(120, m.fps);
        assertTrue(m.width < 1920 && m.height < 1080);
        AdaptiveQualityController detail = controller(AdaptiveQualityController.Mode.DETAIL);
        AdaptiveQualityController.Target d = hot(detail, 15000);
        assertEquals(1920, d.width);
        assertEquals(1080, d.height);
        assertEquals(90, d.fps);
    }

    @Test public void intermittentPressureAndUnobservedFramesDoNotChangeQuality() {
        AdaptiveQualityController quality = controller(AdaptiveQualityController.Mode.MOTION);
        for (int i = 0; i < 100; i++) {
            assertNull(quality.sample(20000 + i * 1000L, true, false, 0, 2, 0, 0, i % 2 == 0 ? 2 : 0, true));
            assertNull(quality.sample(20000 + i * 1000L, false, true, 100, 200, 100, 200, 2, false));
        }
        assertTrue(quality.current().matches(1920, 1080, 120));
    }

    @Test public void congestionUsesBitrateRoomBeforeLoweringResolution() {
        AdaptiveQualityController quality = controller(AdaptiveQualityController.Mode.MOTION);
        for (int i = 0; i < 30; i++) assertNull(quality.sample(15000 + i * 1000L, true, true, 10, 2, 0, 50, 0, true));
        for (int i = 0; i < 7; i++) assertNull(quality.sample(50000 + i * 1000L, true, true, 10, 2, 0, 50, 0, false));
        assertNotNull(quality.sample(57000, true, true, 10, 2, 0, 50, 0, false));
    }

    @Test public void slowDecoderReducesWorkEvenBeforeBitrateHitsItsFloor() {
        AdaptiveQualityController quality = controller(AdaptiveQualityController.Mode.MOTION);
        for (int i = 0; i < 3; i++) assertNull(quality.sample(20000 + i * 1000L, true, false, 0, 12, 0, 0, 0, true));
        assertNotNull(quality.sample(23000, true, false, 0, 12, 0, 0, 0, true));
    }

    @Test public void requestMustBeAppliedAndReductionsAreSeparatedByFifteenSeconds() {
        AdaptiveQualityController quality = controller(AdaptiveQualityController.Mode.MOTION);
        AdaptiveQualityController.Target target = hot(quality, 15000);
        assertTrue(quality.current().matches(1920, 1080, 120));
        quality.applied(target, 18000);
        assertTrue(quality.current().matches(target.width, target.height, target.fps));
        assertNull(hot(quality, 25000));
        assertNotNull(hot(quality, 33000));
    }

    @Test public void sustainedPressureNeverCrossesFloorsOrRaisesAnInitiallyLowerStream() {
        for (AdaptiveQualityController.Mode mode : AdaptiveQualityController.Mode.values()) {
            AdaptiveQualityController quality = controller(mode);
            for (long now = 20000; now < 700000; now += 20000) {
                AdaptiveQualityController.Target target = hot(quality, now);
                if (target != null) quality.applied(target, now + 3000);
                AdaptiveQualityController.Target current = quality.current();
                assertTrue(current.width <= 1920 && current.height <= 1080 && current.fps <= 120);
                assertTrue(Math.min(current.width, current.height) >= 720 && current.fps >= 30);
            }
        }
        AdaptiveQualityController low = new AdaptiveQualityController(AdaptiveQualityController.Mode.MOTION,
                640, 360, 24, 720, 60, 2, 2, 0);
        assertNull(hot(low, 20000));
        assertTrue(low.current().matches(640, 360, 24));
    }

    @Test public void recoveryRequiresThirtyHealthySecondsAndAdvancesOnlyOneStep() {
        AdaptiveQualityController quality = controller(AdaptiveQualityController.Mode.MOTION);
        quality.applied(hot(quality, 15000), 18000);
        for (long now = 19000; now < 49000; now += 1000) {
            assertNull(quality.sample(now, true, false, 0, 2, 0, 0, 0, true));
        }
        AdaptiveQualityController.Target recover = quality.sample(49000, true, false, 0, 2, 0, 0, 0, true);
        assertNotNull(recover);
        assertTrue(recover.matches(1920, 1080, 120));
        quality.applied(recover, 49000);
        assertNull(quality.sample(80000, true, false, 0, 2, 0, 0, 0, true));
    }

    @Test public void unknownMetricsOrWarmthCannotRecoverQuality() {
        AdaptiveQualityController quality = controller(AdaptiveQualityController.Mode.MOTION);
        quality.applied(hot(quality, 15000), 18000);
        for (long now = 20000; now < 100000; now += 1000) {
            assertNull(quality.sample(now, true, false, -1, 2, 0, 0, 0, true));
            assertNull(quality.sample(now, true, false, 0, 2, 0, 0, 1, true));
        }
    }

    @Test public void batteryModeCapsHealthyWorkButRespectsUserMinimums() {
        AdaptiveQualityController quality = controller(AdaptiveQualityController.Mode.BATTERY);
        for (long now = 20000; now <= 200000; now += 20000) {
            AdaptiveQualityController.Target target = quality.sample(now, true, false, 0, 2, 0, 0, 0, true);
            if (target != null) quality.applied(target, now);
        }
        assertEquals(60, quality.current().fps);
        assertTrue(quality.current().height >= 720);
        AdaptiveQualityController fixed = new AdaptiveQualityController(AdaptiveQualityController.Mode.BATTERY,
                1920, 1080, 120, 1080, 120, 2, 2, 0);
        assertNull(fixed.sample(30000, true, false, 0, 2, 0, 0, 0, true));
    }

    @Test public void foldChangePreservesQualityLevelAndAspectWithinAlignment() {
        AdaptiveQualityController quality = controller(AdaptiveQualityController.Mode.MOTION);
        quality.applied(hot(quality, 15000), 18000);
        AdaptiveQualityController.Target folded = quality.resizeCeiling(2316, 904, 120, 30000);
        assertNotNull(folded);
        assertTrue(folded.width < 2316 && folded.height < 904);
        assertTrue(Math.min(folded.width, folded.height) >= 720);
        assertEquals(0, folded.width % 2);
        assertEquals(0, folded.height % 2);
        assertEquals(2316d / 904, (double) folded.width / folded.height, 0.01);
        assertNull(quality.resizeCeiling(2316, 904, 120, 31000));
    }
}
