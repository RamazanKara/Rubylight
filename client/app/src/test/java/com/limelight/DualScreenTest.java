package com.limelight;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import android.content.Intent;

import com.limelight.preferences.PreferenceConfiguration;

import org.junit.Test;

public class DualScreenTest {
    @Test
    public void theSecondScreenIsTheFirstUsableDisplayBesideTheMainOne() {
        assertEquals(2, DualScreen.pickSecondDisplayId(new int[] {0, 1, 2}, new boolean[] {true, false, true}, 0));
        assertEquals(1, DualScreen.pickSecondDisplayId(new int[] {0, 1, 2}, new boolean[] {true, true, true}, 0));
    }

    @Test
    public void aDeviceWithOneScreenHasNoSecondScreen() {
        assertEquals(-1, DualScreen.pickSecondDisplayId(new int[] {0}, new boolean[] {true}, 0));
        assertEquals(-1, DualScreen.pickSecondDisplayId(new int[] {0, 3}, new boolean[] {true, false}, 0));
        assertEquals(-1, DualScreen.pickSecondDisplayId(new int[0], new boolean[0], 0));
    }

    @Test
    public void aStreamNotOnTheDefaultDisplayLeavesThatOneAsTheOther() {
        assertEquals(0, DualScreen.pickSecondDisplayId(new int[] {0, 1}, new boolean[] {true, true}, 1));
    }

    @Test
    public void theSecondScreenTakesAFixedSizeOrItsOwn() {
        PreferenceConfiguration config = new PreferenceConfiguration();
        assertTrue(config.useResolution("1280x720"));
        assertEquals(1280, config.width);
        assertEquals(720, config.height);
        assertFalse(config.resolutionFollowsScreen);
        assertFalse(config.useResolution(PreferenceConfiguration.SECOND_SCREEN_NATIVE));
        assertFalse(config.useResolution("0x720"));
        assertFalse(config.useResolution(null));
        assertEquals(1280, config.width);
    }

    @Test
    public void onlyAnIntentMarkedAsTheSecondStreamIsOne() {
        assertFalse(DualScreen.isSecond(null));
        assertFalse(DualScreen.isSecond(new Intent()));
        assertTrue(DualScreen.isSecond(new Intent().putExtra(DualScreen.EXTRA_SECOND_SCREEN, true)));
    }
}
