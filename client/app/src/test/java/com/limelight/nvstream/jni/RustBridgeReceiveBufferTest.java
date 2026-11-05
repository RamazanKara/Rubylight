package com.limelight.nvstream.jni;

import android.app.Application;

import org.junit.After;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.Implementation;
import org.robolectric.annotation.Implements;
import org.robolectric.util.ReflectionHelpers;

import static org.junit.Assert.assertEquals;

/** The report's receive buffer comes through MoonBridge from the new engine; the classic one has none. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class,
        shadows = {com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class, RustBridgeReceiveBufferTest.Engine.class},
        instrumentedPackages = "com.limelight.nvstream.jni")
public class RustBridgeReceiveBufferTest {
    private static final long INFO = (456L * 1024 << 32) | 77 * 1024;

    /** The export's place, with a socket that was granted something. */
    @Implements(value = RustBridge.class, isInAndroidSdk = false)
    public static class Engine {
        @Implementation protected static void __staticInitializer__() { }
        @Implementation protected static long getVideoReceiveBufferInfo() {
            return INFO;
        }
    }

    @After
    public void tearDown() {
        ReflectionHelpers.setStaticField(MoonBridge.class, "rustStream", false);
    }

    @Test
    public void aNewEngineStreamAsksTheEngine() {
        ReflectionHelpers.setStaticField(MoonBridge.class, "rustStream", true);
        assertEquals(INFO, MoonBridge.getVideoReceiveBufferInfo());
    }

    @Test
    public void aClassicStreamHasNoAnswer() {
        ReflectionHelpers.setStaticField(MoonBridge.class, "rustStream", false);
        assertEquals(-1, MoonBridge.getVideoReceiveBufferInfo());
    }
}
