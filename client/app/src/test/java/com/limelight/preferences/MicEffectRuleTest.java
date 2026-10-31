package com.limelight.preferences;

import android.app.Application;
import android.content.SharedPreferences;
import android.preference.PreferenceManager;

import com.limelight.binding.audio.MicProcessing;

import org.junit.After;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import static org.junit.Assert.*;

/** The "Not on this device" rule on the microphone processing rows. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class)
public class MicEffectRuleTest {
    private static final MicProcessing.Availability NONE = effect -> false, ALL = new MicProcessing.Availability() {
        @Override public boolean has(MicProcessing.Effect effect) {
            return true;
        }

        @Override public boolean hasOwnEchoCanceller() {
            return true;
        }
    };
    /** Android's effects without Nightjar's echo canceller, and the reverse. */
    private static final MicProcessing.Availability ANDROID = effect -> true, OWN = new MicProcessing.Availability() {
        @Override public boolean has(MicProcessing.Effect effect) {
            return false;
        }

        @Override public boolean hasOwnEchoCanceller() {
            return true;
        }
    };
    private static final String ECHO = PreferenceConfiguration.MIC_ECHO_PREF_STRING,
            NOISE = PreferenceConfiguration.MIC_NOISE_PREF_STRING, GAIN = PreferenceConfiguration.MIC_AUTO_GAIN_PREF_STRING;

    private static SharedPreferences prefs() {
        return PreferenceManager.getDefaultSharedPreferences(RuntimeEnvironment.getApplication());
    }

    @After
    public void clear() {
        prefs().edit().clear().commit();
    }

    @Test public void onlyTheThreeProcessingSettingsHaveAnEffect() {
        assertEquals(MicProcessing.Effect.ECHO_CANCELER, SettingRules.micEffect(ECHO));
        assertEquals(MicProcessing.Effect.NOISE_SUPPRESSOR, SettingRules.micEffect(NOISE));
        assertEquals(MicProcessing.Effect.AUTOMATIC_GAIN, SettingRules.micEffect(GAIN));
        assertNull(SettingRules.micEffect(PreferenceConfiguration.MIC_BUTTON_PREF_STRING));
        assertFalse(SettingRules.micEffectMissing(PreferenceConfiguration.MIC_BUTTON_PREF_STRING, prefs(), NONE));
    }

    @Test public void aDeviceWithEveryEffectNeverSaysNotOnThisDevice() {
        prefs().edit().putString(ECHO, "on").putBoolean(NOISE, true).putBoolean(GAIN, true).commit();
        for (String key : new String[] {ECHO, NOISE, GAIN}) assertFalse(key, SettingRules.micEffectMissing(key, prefs(), ALL));
    }

    @Test public void echoCancellationIsMissingUnlessItIsOff() {
        assertTrue("Automatic by default", SettingRules.micEffectMissing(ECHO, prefs(), NONE));
        prefs().edit().putString(ECHO, "on").commit();
        assertTrue(SettingRules.micEffectMissing(ECHO, prefs(), NONE));
        prefs().edit().putString(ECHO, "off").commit();
        assertFalse("Off asks for nothing", SettingRules.micEffectMissing(ECHO, prefs(), NONE));
    }

    @Test public void echoCancellationFollowsTheCancellerItsEngineUses() {
        // Nightjar's own canceller by default: a device without Android's is not missing it.
        assertFalse(SettingRules.micEffectMissing(ECHO, prefs(), OWN));
        assertTrue(SettingRules.micEffectMissing(ECHO, prefs(), ANDROID));
        prefs().edit().putString(PreferenceConfiguration.MIC_ECHO_ENGINE_PREF_STRING, "platform").commit();
        assertTrue(SettingRules.micEffectMissing(ECHO, prefs(), OWN));
        assertFalse(SettingRules.micEffectMissing(ECHO, prefs(), ANDROID));
        prefs().edit().putString(PreferenceConfiguration.MIC_ECHO_ENGINE_PREF_STRING, "both").commit();
        assertFalse(SettingRules.micEffectMissing(ECHO, prefs(), OWN));
        assertFalse(SettingRules.micEffectMissing(ECHO, prefs(), ANDROID));
        assertTrue(SettingRules.micEffectMissing(ECHO, prefs(), NONE));
    }

    @Test public void theSwitchesAreMissingOnlyWhileOnFromTheirOwnDefaults() {
        assertTrue("Noise suppression is on by default", SettingRules.micEffectMissing(NOISE, prefs(), NONE));
        assertFalse("Automatic gain is off by default", SettingRules.micEffectMissing(GAIN, prefs(), NONE));
        prefs().edit().putBoolean(NOISE, false).putBoolean(GAIN, true).commit();
        assertFalse(SettingRules.micEffectMissing(NOISE, prefs(), NONE));
        assertTrue(SettingRules.micEffectMissing(GAIN, prefs(), NONE));
    }

    @Test public void onlyTheLackingEffectIsFlagged() {
        MicProcessing.Availability noGain = new MicProcessing.Availability() {
            @Override public boolean has(MicProcessing.Effect effect) {
                return effect != MicProcessing.Effect.AUTOMATIC_GAIN;
            }

            @Override public boolean hasOwnEchoCanceller() {
                return true;
            }
        };
        prefs().edit().putString(ECHO, "on").putBoolean(NOISE, true).putBoolean(GAIN, true).commit();
        assertFalse(SettingRules.micEffectMissing(ECHO, prefs(), noGain));
        assertFalse(SettingRules.micEffectMissing(NOISE, prefs(), noGain));
        assertTrue(SettingRules.micEffectMissing(GAIN, prefs(), noGain));
    }
}
