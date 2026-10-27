package com.limelight.preferences;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

import android.app.Application;

import com.limelight.pin.PinStore;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import java.nio.charset.StandardCharsets;
import java.util.Collections;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class)
public class PinLockResetTest {
    @Test
    public void resetAllSettingsLeavesThePinLockAlone() {
        Application app = RuntimeEnvironment.getApplication();
        byte[] pin = "2468".getBytes(StandardCharsets.US_ASCII);
        PinStore store = new PinStore(app);
        store.setPin(pin);
        store.setMode(PinStore.Mode.STREAMS);
        store.setPcs(Collections.singleton("pc-1"));

        StreamSettings.resetAllSettings(app);

        PinStore after = new PinStore(app);
        assertTrue(after.hasPin());
        assertEquals(PinStore.Mode.STREAMS, after.mode());
        assertEquals(Collections.singleton("pc-1"), after.pcs());
        assertEquals(PinStore.Result.OK, after.tryPin(pin).result);
    }
}
