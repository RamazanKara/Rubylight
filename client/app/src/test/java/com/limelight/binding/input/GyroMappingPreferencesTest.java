package com.limelight.binding.input;

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertThrows;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.content.Context;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

/** After jvanderv42's Nightjar fork (GPL-3.0), GyroMappingPreferencesTest. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class GyroMappingPreferencesTest {
    private static final float[] SAMPLE = {1f, 2f, 3f};
    private Application app;

    @Before public void setUp() {
        app = RuntimeEnvironment.getApplication();
        clear();
    }

    @After public void tearDown() {
        clear();
    }

    private void clear() {
        app.getSharedPreferences(GyroMappingPreferences.STORE, Context.MODE_PRIVATE).edit().clear().commit();
    }

    private static float[] map(GyroAxisTransformer transformer) {
        return transformer.transform(SAMPLE[0], SAMPLE[1], SAMPLE[2]);
    }

    @Test public void everyControllerIsStandardUntilChosen() {
        assertEquals(GyroMappingPreferences.STANDARD, GyroMappingPreferences.getChoice(app, "unknown"));
        assertTrue(GyroMappingPreferences.transformer(app, "unknown").isIdentity());
        assertTrue(GyroMappingPreferences.transformer(app, (String) null).isIdentity());
        assertTrue(GyroMappingPreferences.transformerForModel(app, "usb:057e:2009").isIdentity());
        // The fork matched its author's own two pads by descriptor; those carry each pad's unique ID.
        assertTrue(GyroMappingPreferences.transformer(app, "874157d9fb2e682d4c2eab8f623109d67ce11434").isIdentity());
    }

    @Test public void theFlydigiPresetAppliesToThePadAndToItsModelOverUsb() {
        GyroMappingPreferences.setChoice(app, "pad-a", "usb:057e:2009", GyroMappingPreferences.FLYDIGI);
        assertEquals(GyroMappingPreferences.FLYDIGI, GyroMappingPreferences.getChoice(app, "pad-a"));
        assertArrayEquals(new float[] {-2f, 3f, 1f}, map(GyroMappingPreferences.transformer(app, "pad-a")), 0f);
        assertArrayEquals(new float[] {-2f, 3f, 1f},
                map(GyroMappingPreferences.transformerForModel(app, "usb:057e:2009")), 0f);
        assertTrue("another pad keeps its own choice", GyroMappingPreferences.transformer(app, "pad-b").isIdentity());

        GyroMappingPreferences.setChoice(app, "pad-a", "usb:057e:2009", GyroMappingPreferences.STANDARD);
        assertTrue(GyroMappingPreferences.transformer(app, "pad-a").isIdentity());
        assertTrue(GyroMappingPreferences.transformerForModel(app, "usb:057e:2009").isIdentity());
    }

    @Test public void anUnknownChoiceIsNotStored() {
        GyroMappingPreferences.setChoice(app, "pad", "usb:1:2", "custom:missing");
        assertEquals(GyroMappingPreferences.STANDARD, GyroMappingPreferences.getChoice(app, "pad"));
        assertTrue(app.getSharedPreferences(GyroMappingPreferences.STORE, Context.MODE_PRIVATE).getAll().isEmpty());
    }

    @Test public void aCustomProfileSurvivesStorageAndItsDeletionReturnsControllersToStandard() {
        GyroMappingPreferences.Profile profile = new GyroMappingPreferences.Profile(null, "My gyro",
                new int[] {2, 0, 1}, new int[] {-1, 1, -1});
        GyroMappingPreferences.saveProfile(app, profile);
        GyroMappingPreferences.setChoice(app, "custom-controller", "usb:1234:5678",
                GyroMappingPreferences.CUSTOM_PREFIX + profile.id);
        assertEquals("My gyro", GyroMappingPreferences.profiles(app).get(0).name);
        assertArrayEquals(new float[] {-3f, 1f, -2f},
                map(GyroMappingPreferences.transformer(app, "custom-controller")), 0f);
        assertArrayEquals(new float[] {-3f, 1f, -2f},
                map(GyroMappingPreferences.transformerForModel(app, "usb:1234:5678")), 0f);

        GyroMappingPreferences.deleteProfile(app, profile.id);
        assertTrue(GyroMappingPreferences.profiles(app).isEmpty());
        assertEquals(GyroMappingPreferences.STANDARD, GyroMappingPreferences.getChoice(app, "custom-controller"));
        assertTrue(GyroMappingPreferences.transformerForModel(app, "usb:1234:5678").isIdentity());
        assertEquals("the assignments go with the profile", 1,
                app.getSharedPreferences(GyroMappingPreferences.STORE, Context.MODE_PRIVATE).getAll().size());
    }

    @Test public void savingAProfileAgainReplacesIt() {
        GyroMappingPreferences.Profile profile = new GyroMappingPreferences.Profile(null, "One",
                new int[] {0, 1, 2}, new int[] {1, 1, 1});
        GyroMappingPreferences.saveProfile(app, profile);
        GyroMappingPreferences.saveProfile(app, new GyroMappingPreferences.Profile(profile.id, "Renamed",
                new int[] {0, 1, 2}, new int[] {1, -1, 1}));
        assertEquals(1, GyroMappingPreferences.profiles(app).size());
        assertEquals("Renamed", GyroMappingPreferences.profiles(app).get(0).name);
    }

    @Test public void sensitivityPersistsAndScalesYawAndPitchButNotRoll() {
        GyroMappingPreferences.Profile profile = new GyroMappingPreferences.Profile(
                null, "Sensitive", new int[] {2, 0, 1}, new int[] {-1, 1, -1}, 1.5f, 0.5f);
        GyroMappingPreferences.saveProfile(app, profile);
        GyroMappingPreferences.Profile restored = GyroMappingPreferences.profiles(app).get(0);
        assertEquals(1.5f, restored.horizontalSensitivity, 0f);
        assertEquals(0.5f, restored.verticalSensitivity, 0f);
        GyroMappingPreferences.setChoice(app, "test", null, GyroMappingPreferences.CUSTOM_PREFIX + profile.id);
        assertArrayEquals(new float[] {-1.5f, 1.5f, -2f},
                map(GyroMappingPreferences.transformer(app, "test")), 0f);
    }

    @Test public void profilesWithoutSensitivityReadAsOneTimes() {
        app.getSharedPreferences(GyroMappingPreferences.STORE, Context.MODE_PRIVATE).edit().putString("custom_profiles",
                "[{\"id\":\"old\",\"name\":\"Old\",\"axes\":[0,1,2],\"signs\":[1,1,1]}]").commit();
        GyroMappingPreferences.Profile restored = GyroMappingPreferences.profiles(app).get(0);
        assertEquals(1f, restored.horizontalSensitivity, 0f);
        assertEquals(1f, restored.verticalSensitivity, 0f);
    }

    @Test public void aDamagedRecordDropsOnlyItself() {
        app.getSharedPreferences(GyroMappingPreferences.STORE, Context.MODE_PRIVATE).edit().putString("custom_profiles",
                "[{\"id\":\"bad\",\"name\":\"Bad\",\"axes\":[0,0,2],\"signs\":[1,1,1]},"
                        + "{\"id\":\"good\",\"name\":\"Good\",\"axes\":[1,0,2],\"signs\":[1,1,1]}]").commit();
        assertEquals(1, GyroMappingPreferences.profiles(app).size());
        assertEquals("good", GyroMappingPreferences.profiles(app).get(0).id);
        app.getSharedPreferences(GyroMappingPreferences.STORE, Context.MODE_PRIVATE).edit()
                .putString("custom_profiles", "not json").commit();
        assertTrue(GyroMappingPreferences.profiles(app).isEmpty());
    }

    @Test public void invalidProfilesCannotBeMade() {
        assertThrows(IllegalArgumentException.class, () -> new GyroMappingPreferences.Profile(
                null, "Invalid", new int[] {0, 1, 2}, new int[] {1, 1, 1}, Float.NaN, 1f));
        assertThrows(IllegalArgumentException.class, () -> new GyroMappingPreferences.Profile(
                null, "Invalid", new int[] {0, 1, 2}, new int[] {1, 1, 1}, 1f, 3.05f));
        assertThrows(IllegalArgumentException.class, () -> new GyroMappingPreferences.Profile(
                null, "Invalid", new int[] {0, 0, 2}, new int[] {1, 1, 1}));
        assertThrows(IllegalArgumentException.class, () -> new GyroMappingPreferences.Profile(
                null, "  ", new int[] {0, 1, 2}, new int[] {1, 1, 1}));
    }
}
