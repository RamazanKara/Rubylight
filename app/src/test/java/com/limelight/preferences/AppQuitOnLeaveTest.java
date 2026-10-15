package com.limelight.preferences;

import android.app.Application;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import java.util.Arrays;
import java.util.Collections;
import java.util.HashSet;
import java.util.Set;

import static org.junit.Assert.*;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class)
public class AppQuitOnLeaveTest {
    @Test public void aGameIsStoredUnderItsUuidAndId() {
        Set<String> stored = AppQuitOnLeave.with(null, "abc", 7, true);
        assertEquals(new HashSet<>(Arrays.asList("uuid:abc", "id:7")), stored);
        assertTrue(AppQuitOnLeave.contains(stored, "abc", 7));
        assertTrue(AppQuitOnLeave.contains(stored, null, 7));
        assertTrue(AppQuitOnLeave.with(stored, "abc", 7, false).isEmpty());
    }

    @Test public void aKnownUuidDecidesAloneBecauseIdsCanBeReused() {
        Set<String> stored = AppQuitOnLeave.with(null, "abc", 7, true);
        assertFalse(AppQuitOnLeave.contains(stored, "other", 7));
        assertFalse(AppQuitOnLeave.contains(Collections.emptySet(), "abc", 7));
        assertFalse(AppQuitOnLeave.contains(null, "abc", 7));
    }

    @Test public void hostsWithoutUuidsMatchByIdAndUnknownIdsMatchNothing() {
        Set<String> stored = AppQuitOnLeave.with(null, "", 12, true);
        assertEquals(Collections.singleton("id:12"), stored);
        assertTrue(AppQuitOnLeave.contains(stored, null, 12));
        assertFalse(AppQuitOnLeave.contains(stored, null, 13));
        assertTrue(AppQuitOnLeave.with(null, null, 0, true).isEmpty());
    }

    @Test public void choicesAreKeptPerPcAndForgottenWithThePc() {
        Application app = RuntimeEnvironment.getApplication();
        assertFalse(AppQuitOnLeave.isEnabled(app, "pc1", "abc", 7));
        AppQuitOnLeave.setEnabled(app, "pc1", "abc", 7, true);
        assertTrue(AppQuitOnLeave.isEnabled(app, "pc1", "abc", 7));
        assertFalse(AppQuitOnLeave.isEnabled(app, "pc2", "abc", 7));
        assertFalse(AppQuitOnLeave.isEnabled(app, null, "abc", 7));
        AppQuitOnLeave.setEnabled(app, "pc1", "abc", 7, false);
        assertFalse(AppQuitOnLeave.isEnabled(app, "pc1", "abc", 7));
        AppQuitOnLeave.setEnabled(app, "pc1", null, 9, true);
        AppQuitOnLeave.forget(app, "pc1");
        assertFalse(AppQuitOnLeave.isEnabled(app, "pc1", null, 9));
    }
}
