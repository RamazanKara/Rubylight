package com.limelight.computers;

import android.app.Application;
import android.content.Context;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class HostPackedModesTest {
    private final Context context = RuntimeEnvironment.getApplication();

    @Test
    public void theLastDescribeIsKeptPerPc() {
        assertEquals(-1, HostPackedModes.cached(context, "pc"));
        HostPackedModes.store(context, "pc", 0b0101);
        HostPackedModes.store(context, "other", 0b1111);
        assertEquals(0b0101, HostPackedModes.cached(context, "pc"));
        assertEquals(0b1111, HostPackedModes.cached(context, "other"));
        HostPackedModes.store(context, "pc", 0b0111);
        assertEquals(0b0111, HostPackedModes.cached(context, "pc"));
        // Kept across instances of the preferences (another stream, another app start).
        assertEquals(0b0111, context.getSharedPreferences(HostPackedModes.PREFERENCES, Context.MODE_PRIVATE)
                .getInt("pc", -1));
    }

    @Test
    public void aDescribeWithoutModesForgetsThePc() {
        HostPackedModes.store(context, "pc", 0b0011);
        HostPackedModes.store(context, "pc", 0);
        assertEquals(-1, HostPackedModes.cached(context, "pc"));
        // No DESCRIBE read (the C engine): what was known stays.
        HostPackedModes.store(context, "pc", 0b0011);
        HostPackedModes.store(context, "pc", -1);
        assertEquals(0b0011, HostPackedModes.cached(context, "pc"));
        // Only modes a newer host added: none this client knows.
        HostPackedModes.store(context, "pc", 0x10);
        assertEquals(-1, HostPackedModes.cached(context, "pc"));
        context.getSharedPreferences(HostPackedModes.PREFERENCES, Context.MODE_PRIVATE).edit()
                .putInt("pc", 0x30).commit();
        assertEquals(-1, HostPackedModes.cached(context, "pc"));
    }

    @Test
    public void modesANewerHostAddsAreLeftOut() {
        HostPackedModes.store(context, "pc", 0x15);
        assertEquals(0b0101, HostPackedModes.cached(context, "pc"));
        context.getSharedPreferences(HostPackedModes.PREFERENCES, Context.MODE_PRIVATE).edit()
                .putInt("pc", 0x3A).commit();
        assertEquals(0b1010, HostPackedModes.cached(context, "pc"));
        // A negative value is not a mask, not every mode.
        context.getSharedPreferences(HostPackedModes.PREFERENCES, Context.MODE_PRIVATE).edit()
                .putInt("pc", -1).commit();
        assertEquals(-1, HostPackedModes.cached(context, "pc"));
    }

    @Test
    public void noPcOrContextIsUnknown() {
        HostPackedModes.store(context, null, 0b0011);
        HostPackedModes.store(null, "pc", 0b0011);
        assertEquals(-1, HostPackedModes.cached(context, null));
        assertEquals(-1, HostPackedModes.cached(null, "pc"));
        assertEquals(-1, HostPackedModes.cached(context, "pc"));
    }

    @Test
    public void serverinfoWinsOverTheCache() {
        assertEquals(0b0001, HostPackedModes.known(0b0001, 0b1111));
        assertEquals(0, HostPackedModes.known(0, 0b1111));
        assertEquals(0b1111, HostPackedModes.known(-1, 0b1111));
        assertEquals(-1, HostPackedModes.known(-1, -1));
    }
}
