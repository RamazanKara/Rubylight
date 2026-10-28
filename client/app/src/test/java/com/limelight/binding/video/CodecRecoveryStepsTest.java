package com.limelight.binding.video;

import static org.junit.Assert.*;

import org.junit.Test;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;

public class CodecRecoveryStepsTest {
    /** Records the steps; the named ones throw, and recreate() fails when created is false. */
    private static final class FakeCodec implements CodecRecoverySteps.Codec {
        final List<String> steps = new ArrayList<>();
        boolean created = true;
        RuntimeException stopFails, resetFails, rebuildFails;
        final List<RuntimeException> configureFails = new ArrayList<>();

        @Override public void stop() {
            steps.add("stop");
            if (stopFails != null) throw stopFails;
        }

        @Override public boolean recreate() {
            steps.add("recreate");
            return created;
        }

        @Override public void reset() {
            steps.add("reset");
            if (resetFails != null) throw resetFails;
        }

        @Override public void configure() {
            steps.add("configure");
            if (!configureFails.isEmpty()) {
                RuntimeException e = configureFails.remove(0);
                if (e != null) throw e;
            }
        }

        @Override public void rebuild(boolean codecFailed) {
            steps.add("rebuild" + (codecFailed ? "" : " (released)"));
            if (rebuildFails != null) throw rebuildFails;
        }
    }

    @Test
    public void aRestartStopsAndConfiguresTheSameCodec() {
        FakeCodec codec = new FakeCodec();
        assertTrue(CodecRecoverySteps.run(true, false, "size change", codec));
        assertEquals(Arrays.asList("stop", "configure"), codec.steps);
    }

    @Test
    public void aRecreatingRestartConfiguresANewCodecWithoutStop() {
        FakeCodec codec = new FakeCodec();
        assertTrue(CodecRecoverySteps.run(true, true, "HDR on", codec));
        assertEquals(Arrays.asList("recreate", "configure"), codec.steps);
    }

    @Test
    public void noNewCodecGoesStraightToASetupThatKeepsDolbyVision() {
        // The released codec is not reset, and its failure to come back is not the decoder's fault.
        FakeCodec codec = new FakeCodec();
        codec.created = false;
        assertTrue(CodecRecoverySteps.run(true, true, "HDR on", codec));
        assertEquals(Arrays.asList("recreate", "rebuild (released)"), codec.steps);
    }

    @Test
    public void aNewCodecThatRefusesItsFormatIsResetAndThenRebuilt() {
        FakeCodec codec = new FakeCodec();
        codec.configureFails.add(new IllegalStateException("configure"));
        assertTrue(CodecRecoverySteps.run(true, true, "HDR on", codec));
        assertEquals(Arrays.asList("recreate", "configure", "reset", "configure"), codec.steps);

        codec = new FakeCodec();
        codec.configureFails.add(new IllegalStateException("configure"));
        codec.resetFails = new IllegalStateException("reset");
        assertTrue(CodecRecoverySteps.run(true, true, "HDR on", codec));
        assertEquals(Arrays.asList("recreate", "configure", "reset", "rebuild"), codec.steps);
    }

    @Test
    public void aPendingResetResetsEvenOnADecoderWhoseRestartsRecreate() {
        // A restart requested while a reset is pending leaves the reset; it is not turned into a recreate.
        for (boolean recreate : new boolean[] {true, false}) {
            FakeCodec codec = new FakeCodec();
            assertTrue(CodecRecoverySteps.run(false, recreate, "HDR metadata", codec));
            assertEquals(Arrays.asList("reset", "configure"), codec.steps);
        }
    }

    @Test
    public void aFailedStopFallsBackToResetThenToARebuildThatDropsDolbyVision() {
        FakeCodec codec = new FakeCodec();
        codec.stopFails = new IllegalStateException("stop");
        assertTrue(CodecRecoverySteps.run(true, false, "CodecException", codec));
        assertEquals(Arrays.asList("stop", "reset", "configure"), codec.steps);

        codec = new FakeCodec();
        codec.stopFails = new IllegalStateException("stop");
        codec.resetFails = new IllegalStateException("reset");
        assertTrue(CodecRecoverySteps.run(true, false, "CodecException", codec));
        assertEquals(Arrays.asList("stop", "reset", "rebuild"), codec.steps);
    }

    @Test
    public void anInvalidSurfaceStopsTheStreamAtAnyStep() {
        FakeCodec restart = new FakeCodec();
        restart.configureFails.add(new IllegalArgumentException("surface"));
        assertFalse(CodecRecoverySteps.run(true, true, "HDR on", restart));
        assertEquals(Arrays.asList("recreate", "configure"), restart.steps);

        FakeCodec reset = new FakeCodec();
        reset.configureFails.add(new IllegalArgumentException("surface"));
        assertFalse(CodecRecoverySteps.run(false, false, "CodecException", reset));
        assertEquals(Arrays.asList("reset", "configure"), reset.steps);

        FakeCodec rebuild = new FakeCodec();
        rebuild.created = false;
        rebuild.rebuildFails = new IllegalArgumentException("surface");
        assertFalse(CodecRecoverySteps.run(true, true, "HDR on", rebuild));
        assertEquals(Arrays.asList("recreate", "rebuild (released)"), rebuild.steps);
    }

    @Test
    public void aRebuildThatFailsIsThrownForTheCrashReport() {
        FakeCodec codec = new FakeCodec();
        codec.resetFails = new IllegalStateException("reset");
        codec.rebuildFails = new IllegalStateException("Decoder reset failed: -2");
        try {
            CodecRecoverySteps.run(false, false, "CodecException", codec);
            fail("expected the rebuild's failure");
        } catch (IllegalStateException e) {
            assertSame(codec.rebuildFails, e);
        }
        assertEquals(Arrays.asList("reset", "rebuild"), codec.steps);
    }
}
