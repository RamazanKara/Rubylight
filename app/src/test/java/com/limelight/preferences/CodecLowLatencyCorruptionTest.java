package com.limelight.preferences;

import android.app.Application;
import android.content.SharedPreferences;
import android.media.MediaCodecInfo;
import android.media.MediaCodecInfo.CodecCapabilities;
import android.media.MediaCodecInfo.CodecProfileLevel;
import android.media.MediaFormat;
import android.preference.PreferenceManager;

import com.limelight.binding.video.MediaCodecHelper;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.MediaCodecInfoBuilder;
import org.robolectric.shadows.MediaCodecInfoBuilder.CodecCapabilitiesBuilder;
import org.robolectric.shadows.ShadowBuild;
import org.robolectric.shadows.ShadowMediaCodecList;

import static org.junit.Assert.*;

/**
 * Erratum 23: on one exact Fire TV model with a MediaTek decoder, Android's low-latency mode starts
 * off, once, and a later On is kept. Every other device stores nothing and keeps what it has.
 */
@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class CodecLowLatencyCorruptionTest {
    private static final String MODEL = "AFTNE4E43F";
    private static final String MTK = "c2.mtk.hevc.decoder";
    private static final String KEY = "checkbox_codec_low_latency";
    private static final String FLAG = "codec_low_latency_corruption_off";

    private static Application app() {
        return RuntimeEnvironment.getApplication();
    }

    private static SharedPreferences prefs() {
        return PreferenceManager.getDefaultSharedPreferences(app());
    }

    static void device(String model, String decoder) {
        ShadowBuild.setModel(model);
        MediaFormat format = MediaFormat.createVideoFormat("video/hevc", 1920, 1080);
        MediaCodecInfo info = MediaCodecInfoBuilder.newBuilder().setName(decoder).setCapabilities(
                CodecCapabilitiesBuilder.newBuilder().setMediaFormat(format)
                        .setProfileLevels(new CodecProfileLevel[0])
                        .setColorFormats(new int[] {CodecCapabilities.COLOR_FormatYUV420Flexible}).build()).build();
        ShadowMediaCodecList.addCodec(info);
    }

    @Before
    @After
    public void forgetCodecs() {
        MediaCodecHelper.forgetCodecList();
    }

    @Test public void aNewInstallOnTheModelStartsOffAndKeepsTheVendorKeys() {
        device(MODEL, MTK);
        PreferenceConfiguration config = PreferenceConfiguration.readPreferences(app());
        assertFalse(config.codecLowLatency);
        assertTrue(config.vendorLowLatency);
        assertFalse(prefs().getBoolean(KEY, true));
    }

    @Test public void aStoredOnIsSwitchedOffOnceAndALaterOnIsKept() {
        device(MODEL, MTK);
        // The default every earlier install stored, untouched.
        prefs().edit().putBoolean(KEY, true).commit();
        assertFalse(PreferenceConfiguration.readPreferences(app()).codecLowLatency);

        // The user's On afterwards survives every later read.
        prefs().edit().putBoolean(KEY, true).commit();
        assertTrue(PreferenceConfiguration.readPreferences(app()).codecLowLatency);
        assertTrue(PreferenceConfiguration.readPreferences(app()).codecLowLatency);
        assertTrue(prefs().getBoolean(KEY, false));
    }

    @Test public void theMasterSwitchOnIsKeptToo() {
        device(MODEL, MTK);
        PreferenceConfiguration.readPreferences(app());
        PreferenceConfiguration.setLowLatencyDecoding(prefs(), true);
        assertTrue(PreferenceConfiguration.readPreferences(app()).codecLowLatency);
    }

    @Test public void theMigrationReportsOnlyTheSwitchItMade() {
        device(MODEL, MTK);
        prefs().edit().putBoolean(KEY, false).commit();
        assertFalse("it was already off", PreferenceConfiguration.migrateCodecLowLatencyCorruption(prefs()));
        assertTrue(prefs().getBoolean(FLAG, false));
        prefs().edit().remove(FLAG).putBoolean(KEY, true).commit();
        assertTrue(PreferenceConfiguration.migrateCodecLowLatencyCorruption(prefs()));
        assertFalse(PreferenceConfiguration.migrateCodecLowLatencyCorruption(prefs()));
    }

    @Test public void anotherModelWithTheSameDecoderIsUntouched() {
        for (String model : new String[] {"AFTNE4E43", "AFTNE4E43F2", "aftne4e43f", "AFTMM", "Retroid Pocket 5"}) {
            device(model, MTK);
            assertFalse(model, MediaCodecHelper.hasLowLatencyCorruptingDecoder());
            assertTrue(model, PreferenceConfiguration.readPreferences(app()).codecLowLatency);
            assertFalse(model, prefs().contains(FLAG));
            assertTrue(model, prefs().getBoolean(KEY, true));
            MediaCodecHelper.forgetCodecList();
        }
    }

    @Test public void theModelWithAnotherDecoderIsUntouched() {
        device(MODEL, "c2.amlogic.hevc.decoder");
        assertFalse(MediaCodecHelper.hasLowLatencyCorruptingDecoder());
        assertTrue(PreferenceConfiguration.readPreferences(app()).codecLowLatency);
        assertFalse(prefs().contains(FLAG));
        assertTrue(prefs().getBoolean(KEY, true));
    }

    @Test public void aStoredOffStaysAChoiceOnAnotherModel() {
        device("AFTMM", MTK);
        // The first read of a fresh profile stores the defaults; a choice comes after it.
        PreferenceConfiguration.readPreferences(app());
        prefs().edit().putBoolean(KEY, false).commit();
        assertFalse(PreferenceConfiguration.readPreferences(app()).codecLowLatency);
        assertFalse(prefs().contains(FLAG));
    }

    @Test public void theGateIsTheExactModelPlusAMediaTekName() {
        ShadowBuild.setModel(MODEL);
        for (String name : new String[] {MTK, "c2.mtk.av1.decoder", "C2.MTK.avc.decoder", "omx.mtk.video.decoder.hevc"}) {
            assertTrue(name, MediaCodecHelper.corruptsInLowLatency(name));
        }
        for (String name : new String[] {"c2.amlogic.hevc.decoder", "c2.qti.hevc.decoder", "c2.android.hevc.decoder", "mtk"}) {
            assertFalse(name, MediaCodecHelper.corruptsInLowLatency(name));
        }
        ShadowBuild.setModel("AFTMM");
        assertFalse(MediaCodecHelper.corruptsInLowLatency(MTK));
    }
}
