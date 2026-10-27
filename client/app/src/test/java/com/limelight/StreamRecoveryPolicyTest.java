package com.limelight;

import org.junit.Test;
import static org.junit.Assert.*;

public class StreamRecoveryPolicyTest {
    @Test public void increasingDelaysAndBudgetSurviveActivityReplacement() {
        StreamRecoveryPolicy policy = new StreamRecoveryPolicy(0, 0);
        long now = 10000;
        for (long delay : new long[] {1000, 2000, 4000, 8000, 12000}) {
            assertEquals(delay, policy.nextDelay(now, -1));
            now += delay;
            policy = new StreamRecoveryPolicy(policy.attempts(), policy.startedAt());
        }
        assertEquals(-1, policy.nextDelay(now, -1));
        assertEquals(10000, policy.startedAt());
    }

    @Test public void slowStartupAndFlappingConnectionsCannotExtendTheWindow() {
        StreamRecoveryPolicy policy = new StreamRecoveryPolicy(3, 1000);
        assertEquals(-1, policy.nextDelay(59000, 58000));
        assertEquals(-1, new StreamRecoveryPolicy(5, 1000).nextDelay(25000, 20000));
    }

    @Test public void stableConnectionReceivesANewBudget() {
        StreamRecoveryPolicy policy = new StreamRecoveryPolicy(5, 1000);
        assertEquals(1000, policy.nextDelay(70000, 40000));
        assertEquals(1, policy.attempts());
        assertEquals(70000, policy.startedAt());
    }

    @Test public void aStableSessionResetsAtStopWhileTimeLockedDoesNotCountAsStability() {
        StreamRecoveryPolicy stable = new StreamRecoveryPolicy(5, 1000);
        stable.resetAfterStableConnection(40000, 10000);
        assertEquals(1000, stable.nextDelay(200000, -1));
        assertEquals(200000, stable.startedAt());

        StreamRecoveryPolicy flapping = new StreamRecoveryPolicy(4, 1000);
        flapping.resetAfterStableConnection(25000, 20000);
        assertEquals(-1, flapping.nextDelay(200000, -1));
        assertEquals(4, flapping.attempts());
        assertEquals(1000, flapping.startedAt());
    }

    @Test public void gracefulPermanentAndUnknownErrorsDoNotRetry() {
        for (int code : new int[] {0, -103, -104, -105, 401, 403, 409, 470, 525, -999, 123456}) {
            assertFalse("code " + code, StreamRecoveryPolicy.transientTermination(code));
        }
        for (int code : new int[] {-100, -101, -102, -1, 104, 110, 10054, 10060}) {
            assertTrue("code " + code, StreamRecoveryPolicy.transientTermination(code));
        }
    }

    @Test public void startupRejectsDecoderAndHostErrorsEvenWithPortFlags() {
        assertTrue(StreamRecoveryPolicy.transientStartup("Control stream", 4, -1));
        assertTrue(StreamRecoveryPolicy.transientStartup("Game", 3, 0));
        assertFalse(StreamRecoveryPolicy.transientStartup("Video stream", 256, -1));
        assertFalse(StreamRecoveryPolicy.transientStartup("Game", 3, 403));
        assertFalse(StreamRecoveryPolicy.transientStartup("Game", 0, 0));
    }

    @Test public void nativeStartupTransportErrorsRetryWithoutTreatingHostStatusAsTransport() {
        String[] stages = {"Control stream establishment", "RTSP handshake"};
        int[] portFlags = {0x0200, 0x0c04};
        for (int i = 0; i < stages.length; i++) {
            for (int error : new int[] {32, 101, 104, 110, 111, 113}) {
                assertTrue(stages[i] + " errno " + error,
                        StreamRecoveryPolicy.transientStartup(stages[i], portFlags[i], error));
            }
            for (int error : new int[] {12, 22, 401, 403, 408, 409, 470, 500, 503, 525}) {
                assertFalse(stages[i] + " error " + error,
                        StreamRecoveryPolicy.transientStartup(stages[i], portFlags[i], error));
            }
        }
    }

    @Test public void javaTransportFailuresDoNotInterpretTheGameTitleAsADecoderStage() {
        assertTrue(StreamRecoveryPolicy.transientStartup("Video Game", 0x0003, 0));
        assertTrue(StreamRecoveryPolicy.transientStartup("VIDEO", 0x0003, 0));
        assertFalse(StreamRecoveryPolicy.transientStartup("Video stream", 0x0100, -1));
        assertFalse(StreamRecoveryPolicy.transientStartup("Video Game", 0, 0));
        assertFalse(StreamRecoveryPolicy.transientStartup("Video Game", 0x0003, 403));
    }

    @Test public void invalidPersistedStateCannotOverflowOrRunAfterAClockReset() {
        assertEquals(-1, new StreamRecoveryPolicy(Integer.MAX_VALUE, 1000).nextDelay(2000, -1));
        assertEquals(-1, new StreamRecoveryPolicy(1, 90000).nextDelay(1000, -1));
    }
}
