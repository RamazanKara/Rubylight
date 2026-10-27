package com.limelight.utils;

import android.app.Application;
import android.content.Intent;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertSame;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class)
public class LaunchSourceTest {
    @Test public void anUnlabelledStartIsLoggedAsUnknown() {
        assertEquals("Stream launch from unknown", LaunchSource.describe(new Intent()));
        assertEquals(LaunchSource.UNKNOWN, LaunchSource.of(new Intent().putExtra(LaunchSource.EXTRA, "")));
        assertEquals(LaunchSource.UNKNOWN, LaunchSource.of(null));
    }

    @Test public void aLabelledStartIsLoggedWithItsSource() {
        Intent intent = new Intent();
        assertSame(intent, LaunchSource.put(intent, LaunchSource.LIBRARY_TAP));
        assertEquals("Stream launch from library tap", LaunchSource.describe(intent));
    }

    @Test public void shortcutsAndFrontendsNameTheAppThatSentThem() {
        assertEquals("shortcut", LaunchSource.external(false, null));
        assertEquals("frontend", LaunchSource.external(true, ""));
        assertEquals("frontend from org.es_de.frontend", LaunchSource.external(true, "org.es_de.frontend"));
        assertEquals("shortcut from com.android.launcher3", LaunchSource.external(false, "com.android.launcher3"));
    }

    @Test public void aReconnectKeepsTheStartThatOpenedTheStreamAndDoesNotChain() {
        Intent first = LaunchSource.put(new Intent(), LaunchSource.external(true, "org.es_de.frontend"));
        Intent reconnect = LaunchSource.put(new Intent(first), LaunchSource.again(LaunchSource.RECONNECT, first));
        assertEquals("reconnect after frontend from org.es_de.frontend", LaunchSource.of(reconnect));
        Intent recovery = LaunchSource.put(new Intent(reconnect), LaunchSource.again(LaunchSource.RECOVERY, reconnect));
        assertEquals("recovery after frontend from org.es_de.frontend", LaunchSource.of(recovery));
        assertEquals("return to stream after unknown", LaunchSource.again(LaunchSource.RETURN, new Intent()));
    }
}
