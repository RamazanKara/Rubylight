package com.limelight;

import com.limelight.nvstream.http.NvApp;
import com.limelight.nvstream.jni.MoonBridge;

import org.junit.Test;

import static org.junit.Assert.*;

public class StreamExitPolicyTest {
    @Test public void leavingQuitsOnlyAGameThePlayerChoseToQuit() {
        assertTrue(StreamExitPolicy.quitsOnLeave(true, NvApp.Role.STREAM, true));
        assertFalse(StreamExitPolicy.quitsOnLeave(false, NvApp.Role.STREAM, true));
    }

    @Test public void aStreamThatIsNoLongerConnectedLeavesWithoutQuitting() {
        assertFalse(StreamExitPolicy.quitsOnLeave(true, NvApp.Role.STREAM, false));
    }

    @Test public void remoteMonitorAndInputSessionsAreNeverQuitOnLeave() {
        assertFalse(StreamExitPolicy.quitsOnLeave(true, NvApp.Role.REMOTE_MONITOR, true));
        assertFalse(StreamExitPolicy.quitsOnLeave(true, NvApp.Role.INPUT_ONLY, true));
    }

    @Test public void theHostEndingTheStreamNeverNeedsADialog() {
        assertFalse(StreamExitPolicy.terminationNeedsAction(MoonBridge.ML_ERROR_GRACEFUL_TERMINATION, false));
        assertFalse(StreamExitPolicy.terminationNeedsAction(MoonBridge.ML_ERROR_GRACEFUL_TERMINATION, true));
    }

    @Test public void aLostConnectionOrAHostThatWentAwayReturnsToTheList() {
        // -1: ENet disconnect or timeout with no reason; the rest are socket errors (ETIMEDOUT,
        // ECONNRESET, WSAETIMEDOUT) and the Java transport's own failures.
        for (int code : new int[] {-1, 32, 54, 104, 110, 111, 113, 10053, 10054, 10060}) {
            assertFalse("code " + code, StreamExitPolicy.terminationNeedsAction(code, false));
        }
    }

    @Test public void aReasonOnlyThePcKnowsReturnsToTheList() {
        // Host termination reasons: the HTTP-like codes some hosts send and NVST_DISCONN_* values.
        for (int code : new int[] {503, 404, 0x80030020, 0xc0000005}) {
            assertFalse("code " + code, StreamExitPolicy.terminationNeedsAction(code, false));
        }
    }

    @Test public void errorsWithAdviceKeepTheDialog() {
        assertTrue(StreamExitPolicy.terminationNeedsAction(MoonBridge.ML_ERROR_NO_VIDEO_TRAFFIC, false));
        assertTrue(StreamExitPolicy.terminationNeedsAction(MoonBridge.ML_ERROR_NO_VIDEO_FRAME, false));
        assertTrue(StreamExitPolicy.terminationNeedsAction(MoonBridge.ML_ERROR_UNEXPECTED_EARLY_TERMINATION, false));
        assertTrue(StreamExitPolicy.terminationNeedsAction(MoonBridge.ML_ERROR_PROTECTED_CONTENT, false));
        assertTrue(StreamExitPolicy.terminationNeedsAction(MoonBridge.ML_ERROR_FRAME_CONVERSION, false));
    }

    @Test public void aBlockedPortTestKeepsTheDialogForAnyError() {
        assertTrue(StreamExitPolicy.terminationNeedsAction(-1, true));
        assertTrue(StreamExitPolicy.terminationNeedsAction(110, true));
        assertTrue(StreamExitPolicy.terminationNeedsAction(503, true));
    }
}
