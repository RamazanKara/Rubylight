package com.limelight.preferences;

import android.app.Application;
import android.content.Intent;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.annotation.Config;

import static org.junit.Assert.*;

/**
 * A stream keeps the screen it started on in its intent, as it keeps its network, so a restart after a lost
 * connection and the dual-screen second stream pick the profile it started with (nightjar#163).
 */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class)
public class StreamScreenIntentTest {
    @Test
    public void theScreenRidesInTheIntentAndACopyOfIt() {
        Intent started = new Intent();
        assertNull(StreamScreen.from(started));
        StreamScreen.external("HDMI Screen").putInto(started);
        StreamScreen restarted = StreamScreen.from(new Intent(started));
        assertEquals("external:HDMI Screen", restarted.id);
        assertEquals("HDMI Screen", restarted.name);

        Intent own = new Intent();
        StreamScreen.builtIn().putInto(own);
        assertEquals(StreamScreen.BUILT_IN, StreamScreen.from(own).id);
        assertEquals("", StreamScreen.from(own).name);
    }
}
