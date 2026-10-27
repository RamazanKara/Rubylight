package com.limelight.binding.input;

import android.app.Application;
import android.preference.PreferenceManager;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class DesktopControlsConfigTest {
    @Test public void defaultsStayCompactAndDoNotEnableHeldModifiers() {
        DesktopControlsConfig config = DesktopControlsConfig.parse("{}");
        assertEquals("keyboard", config.mode);
        assertEquals("typing", config.bank);
        assertEquals(300, config.panelHeightDp);
        assertEquals(40, config.keyHeightDp);
        assertTrue(config.tapToClick);
        assertTrue(config.mouseButtons);
        assertFalse(config.lockModifiers);
        assertFalse(config.middleButton);
        assertFalse(config.extraButtons);
    }

    @Test public void malformedAndFutureDataUseDefaultsAndNumbersAreBounded() {
        for (String source : new String[] {null, "[broken", "{\"version\":2,\"panelHeightDp\":500}"}) {
            assertEquals(300, DesktopControlsConfig.parse(source).panelHeightDp);
        }
        DesktopControlsConfig bounded = DesktopControlsConfig.parse("{\"mode\":\"unknown\",\"bank\":\"hidden\","
                + "\"keyHeightDp\":0,\"panelHeightDp\":99999,\"opacityPercent\":-1,\"pointerSpeedPercent\":99999}");
        assertEquals("keyboard", bounded.mode);
        assertEquals("typing", bounded.bank);
        assertEquals(36, bounded.keyHeightDp);
        assertEquals(560, bounded.panelHeightDp);
        assertEquals(50, bounded.opacityPercent);
        assertEquals(400, bounded.pointerSpeedPercent);
    }

    @Test public void settingsRoundTripAndResetDoesNotChangeOtherInputPreferences() {
        Application context = RuntimeEnvironment.getApplication();
        PreferenceManager.getDefaultSharedPreferences(context).edit()
                .putString("keyboard_shortcuts", "{\"data\":[]}").putInt("trackpad_speed", 175).apply();
        DesktopControlsConfig config = new DesktopControlsConfig();
        config.mode = "combined";
        config.bank = "navigation";
        config.leftHanded = true;
        config.naturalScroll = false;
        config.tapToClick = false;
        config.extraButtons = true;
        config.middleButton = true;
        config.pointerSpeedPercent = 250;
        config.save(context);
        DesktopControlsConfig loaded = DesktopControlsConfig.load(context);
        assertEquals("combined", loaded.mode);
        assertEquals("navigation", loaded.bank);
        assertTrue(loaded.leftHanded);
        assertTrue(loaded.middleButton);
        assertTrue(loaded.extraButtons);
        assertFalse(loaded.naturalScroll);
        assertFalse(loaded.tapToClick);
        assertEquals(250, loaded.pointerSpeedPercent);
        DesktopControlsConfig copy = loaded.copy();
        copy.pointerSpeedPercent = 50;
        assertEquals(250, loaded.pointerSpeedPercent);
        new DesktopControlsConfig().save(context);
        assertEquals(175, PreferenceManager.getDefaultSharedPreferences(context).getInt("trackpad_speed", 0));
        assertEquals("{\"data\":[]}", PreferenceManager.getDefaultSharedPreferences(context).getString("keyboard_shortcuts", null));
    }
}
