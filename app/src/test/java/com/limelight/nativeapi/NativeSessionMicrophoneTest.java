package com.limelight.nativeapi;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import android.app.Application;

import org.json.JSONObject;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

/** GET /session tells Microphone only when another device's microphone plays on the PC. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class NativeSessionMicrophoneTest {
    @Test
    public void thePcsSessionSaysWhenTheMicrophoneIsBusy() throws Exception {
        String session = "{\"revision\":1,\"state\":\"streaming\",\"owner\":\"you\",\"app\":null,"
                + "\"game_running\":false,\"actions\":[]";
        assertTrue(NativeSession.parse(
                new JSONObject(session + ",\"microphone_busy\":true}")).microphoneBusy);
        // Absent from older hosts and whenever it plays.
        assertFalse(NativeSession.parse(new JSONObject(session + "}")).microphoneBusy);
    }
}
