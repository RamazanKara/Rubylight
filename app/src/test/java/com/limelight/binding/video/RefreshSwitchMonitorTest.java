package com.limelight.binding.video;

import static org.junit.Assert.*;
import static org.robolectric.Shadows.shadowOf;

import android.app.Application;
import android.content.Context;
import android.hardware.display.DisplayManager;
import android.os.PowerManager;
import android.provider.Settings;
import android.view.Display;

import com.limelight.R;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.Implementation;
import org.robolectric.annotation.Implements;
import org.robolectric.annotation.RealObject;
import org.robolectric.shadow.api.Shadow;
import org.robolectric.shadows.ShadowDisplay;

import java.lang.reflect.Constructor;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;

/**
 * The monitor against a made-up panel. The Display and its modes are fakes: no phone with several
 * seamless rates was available, which is why docs/expected-refresh-switch.md calls this Expected.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class, shadows = RefreshSwitchMonitorTest.FakePanel.class)
public class RefreshSwitchMonitorTest {
    private static final long SETTLE = RefreshSwitchDiagnosis.Watcher.SETTLE_MS;
    private final Context context = RuntimeEnvironment.getApplication();
    private Display display;

    @Before public void panel() {
        display = ((DisplayManager) context.getSystemService(Context.DISPLAY_SERVICE)).getDisplay(0);
        // A 60/90/120 phone in one config group, currently at 60.
        FakePanel.set(mode(1, 60f, 90f, 120f), mode(1, 60f, 90f, 120f), mode(2, 90f, 60f, 120f),
                mode(3, 120f, 60f, 90f));
    }

    @After public void reset() {
        FakePanel.clear();
    }

    @Test public void aScreenThatStaysIsExplainedOnlyAfterTheSettleTime() {
        RefreshSwitchMonitor monitor = new RefreshSwitchMonitor(context);
        assertNull(monitor.poll(1_000, display, 120f, true));
        assertNull(monitor.poll(1_000 + SETTLE - 1, display, 120f, true));
        assertEquals(context.getString(R.string.refresh_diag_unexplained, "120", "60", "60, 90, 120"),
                monitor.poll(1_000 + SETTLE, display, 120f, true));
        // Still the same text on the next poll: the overlay keeps it until the screen follows.
        assertNotNull(monitor.poll(1_000 + SETTLE + 1_000, display, 120f, true));
    }

    @Test public void theTextGoesWhenTheScreenFollowsOrTheVoteIsWithdrawn() {
        RefreshSwitchMonitor monitor = new RefreshSwitchMonitor(context);
        monitor.poll(0, display, 120f, true);
        assertNotNull(monitor.poll(SETTLE, display, 120f, true));
        FakePanel.setActive(mode(3, 120f, 60f, 90f));
        assertNull(monitor.poll(SETTLE + 1_000, display, 120f, true));

        FakePanel.setActive(mode(1, 60f, 90f, 120f));
        monitor.poll(10_000, display, 120f, true);
        assertNotNull(monitor.poll(10_000 + SETTLE, display, 120f, true));
        assertNull("No vote, nothing to follow", monitor.poll(20_000, display, null, true));
        assertNull("The zero vote leaves the rate to ARR", monitor.poll(21_000, display, 0f, true));
    }

    @Test public void batterySaverIsNamedWhenItIsOn() {
        shadowOf((PowerManager) context.getSystemService(Context.POWER_SERVICE)).setIsPowerSaveMode(true);
        assertEquals(context.getString(R.string.refresh_diag_battery_saver, "120", "60"), settled(120f));
    }

    @Test public void aCriticalThermalStatusIsNamed() {
        shadowOf((PowerManager) context.getSystemService(Context.POWER_SERVICE))
                .setCurrentThermalStatus(PowerManager.THERMAL_STATUS_CRITICAL);
        assertEquals(context.getString(R.string.refresh_diag_thermal, "120", "60"), settled(120f));
    }

    @Test public void theUsersPeakRefreshRateSettingIsNamedWithItsValue() {
        Settings.System.putFloat(context.getContentResolver(), "peak_refresh_rate", 60f);
        assertEquals(context.getString(R.string.refresh_diag_user_limit, "120", "60", "60"), settled(120f));
    }

    @Test public void anUnreadableSettingOnlyMeansNoUserLimitIsKnown() {
        // Absent (the usual case on a phone that does not store it) and zero both read as unknown.
        assertEquals(0f, RefreshSwitchMonitor.conditions(context).peakSettingHz, 0f);
        Settings.System.putFloat(context.getContentResolver(), "peak_refresh_rate", 0f);
        assertEquals(0f, RefreshSwitchMonitor.conditions(context).peakSettingHz, 0f);
        assertFalse(RefreshSwitchMonitor.conditions(context).batterySaver);
        assertFalse(RefreshSwitchMonitor.conditions(context).thermalCritical);
    }

    @Test public void aRateNoModeOffersListsTheRatesTheScreenHas() {
        assertEquals(context.getString(R.string.refresh_diag_no_mode, "144", "60, 90, 120", "60"), settled(144f));
    }

    @Test public void modesThatWouldBlankTheScreenAreNamedWhenTheyAreAllThatIsLeft() {
        FakePanel.set(mode(1, 60f), mode(1, 60f), mode(2, 120f));
        assertEquals(context.getString(R.string.refresh_diag_not_seamless, "120", "60", "60, 120"), settled(120f));
    }

    @Test public void theMonitorReadsTheAlternativesFromTheModes() {
        RefreshSwitchDiagnosis.PanelMode active = RefreshSwitchMonitor.activeMode(display);
        assertEquals(60f, active.hz, 0f);
        assertArrayEquals(new float[] {90f, 120f}, active.alternatives, 0f);
        assertEquals(3, RefreshSwitchMonitor.modes(display).size());
    }

    @Test @Config(sdk = 30)
    public void onAndroid11AlternativesAreUnknownSoSeamlessnessIsNeverBlamed() {
        FakePanel.set(mode(1, 60f), mode(1, 60f), mode(2, 120f));
        assertTrue(RefreshSwitchMonitor.activeMode(display).alternatives.length == 0);
        assertEquals(context.getString(R.string.refresh_diag_unexplained, "120", "60", "60, 120"), settled(120f));
    }

    @Test @Config(sdk = 29)
    public void beforeAndroid11ThereIsNoVoteToFollow() {
        RefreshSwitchMonitor monitor = new RefreshSwitchMonitor(context);
        assertNull(monitor.poll(0, display, 120f, true));
        assertNull(monitor.poll(SETTLE * 2, display, 120f, true));
    }

    private String settled(float vote) {
        RefreshSwitchMonitor monitor = new RefreshSwitchMonitor(context);
        assertNull(monitor.poll(0, display, vote, true));
        return monitor.poll(SETTLE, display, vote, true);
    }

    /** Builds a Display.Mode whatever this API level's hidden constructor looks like. */
    static Display.Mode mode(int id, float hz, float... alternatives) {
        Constructor<?> best = null;
        int bestScore = -1;
        for (Constructor<?> candidate : Display.Mode.class.getDeclaredConstructors()) {
            boolean known = true;
            boolean hasAlternatives = false;
            for (Class<?> type : candidate.getParameterTypes()) {
                known &= type == int.class || type == float.class || type == boolean.class ||
                        type == float[].class || type == int[].class;
                hasAlternatives |= type == float[].class;
            }
            int score = candidate.getParameterCount() + (hasAlternatives && alternatives.length > 0 ? 100 : 0);
            if (known && score > bestScore) {
                best = candidate;
                bestScore = score;
            }
        }
        assertNotNull("No usable Display.Mode constructor on this API level", best);
        Class<?>[] types = best.getParameterTypes();
        Object[] args = new Object[types.length];
        int[] ints = {id, 1080, 2400};
        int nextInt = 0;
        for (int i = 0; i < types.length; i++) {
            if (types[i] == int.class) args[i] = ints[nextInt++];
            else if (types[i] == float.class) args[i] = hz;
            else if (types[i] == boolean.class) args[i] = false;
            else if (types[i] == float[].class) args[i] = alternatives;
            else if (types[i] == int[].class) args[i] = new int[] {Display.HdrCapabilities.HDR_TYPE_HDR10};
            else throw new AssertionError("Unexpected Display.Mode constructor parameter " + types[i]);
        }
        try {
            best.setAccessible(true);
            return (Display.Mode) best.newInstance(args);
        } catch (ReflectiveOperationException e) {
            throw new AssertionError(e);
        }
    }

    @Implements(Display.class)
    public static class FakePanel extends ShadowDisplay {
        private static Display.Mode active;
        private static final List<Display.Mode> supported = new ArrayList<>();
        @RealObject private Display display;

        static void set(Display.Mode activeMode, Display.Mode... modes) {
            active = activeMode;
            supported.clear();
            supported.addAll(Arrays.asList(modes));
        }

        static void setActive(Display.Mode mode) {
            active = mode;
        }

        static void clear() {
            active = null;
            supported.clear();
        }

        @Implementation protected Display.Mode getMode() {
            return active != null ? active : Shadow.directlyOn(display, Display.class, "getMode");
        }

        @Implementation protected Display.Mode[] getSupportedModes() {
            return active != null ? supported.toArray(new Display.Mode[0]) :
                    Shadow.directlyOn(display, Display.class, "getSupportedModes");
        }
    }
}
