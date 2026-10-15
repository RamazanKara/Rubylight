package com.limelight;

import com.limelight.nvstream.http.NvApp;

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
}
