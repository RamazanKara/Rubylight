package com.limelight.binding.video;

import android.app.Activity;
import android.app.Application;
import android.content.SharedPreferences;
import android.media.MediaCodecInfo;
import android.media.MediaCodecInfo.CodecCapabilities;
import android.media.MediaCodecInfo.CodecProfileLevel;
import android.media.MediaFormat;
import android.preference.PreferenceManager;

import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.utils.UiHelper;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.MediaCodecInfoBuilder;
import org.robolectric.shadows.MediaCodecInfoBuilder.CodecCapabilitiesBuilder;
import org.robolectric.shadows.ShadowDialog;
import org.robolectric.shadows.ShadowLooper;
import org.robolectric.shadows.ShadowMediaCodecList;
import org.robolectric.util.ReflectionHelpers;

import java.util.Arrays;
import java.util.List;
import java.util.Map;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

/**
 * A decoder given up on (-107) ends its stream with a dialog, and the next stream runs without
 * reference frame invalidation, as after a decoder crash, with no second dialog and no reset.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class DecoderGiveUpRfiTest {
    private static final String AML_HEVC = "c2.amlogic.hevc.decoder";

    private ActivityController<Activity> activity;
    private SharedPreferences tombstone;

    @Before
    public void setUp() {
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "isFireTv", false);
        MediaCodecHelper.forgetCodecList();
        ShadowMediaCodecList.addCodec(lowLatencyHevc());
        activity = Robolectric.buildActivity(Activity.class).create();
        tombstone = activity.get().getSharedPreferences("DecoderTombstone", 0);
        tombstone.edit().clear().commit();
    }

    @After
    public void tearDown() {
        MediaCodecHelper.forgetCodecList();
        activity.close();
    }

    private MediaCodecDecoderRenderer stream(CrashListener listener) {
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.videoFormat = PreferenceConfiguration.FormatOption.FORCE_HEVC;
        prefs.codecLowLatency = prefs.vendorLowLatency = true;
        return new MediaCodecDecoderRenderer(activity.get(), prefs, listener,
                tombstone.getInt("CrashCount", 0), false, false, "", null);
    }

    private static boolean hevcRfi(MediaCodecDecoderRenderer renderer) {
        return (renderer.getCapabilities() & MoonBridge.CAPABILITY_REFERENCE_FRAME_INVALIDATION_HEVC) != 0;
    }

    @Test
    public void aGiveUpTurnsReferenceFrameInvalidationOffForTheNextStream() {
        MediaCodecDecoderRenderer first = stream(e -> CrashAccounting.recordGiveUp(tombstone, "rfi-pc"));
        assertTrue(hevcRfi(first));
        ReflectionHelpers.setField(first, "activeDecoderName", AML_HEVC);
        ReflectionHelpers.callInstanceMethod(first, "failDecoder",
                ReflectionHelpers.ClassParameter.from(String.class, AML_HEVC + " took no input for 5000 ms"),
                ReflectionHelpers.ClassParameter.from(Exception.class, null));
        assertEquals(1, tombstone.getInt("CrashCount", 0));
        assertEquals("rfi-pc", tombstone.getString("CrashHostUuid", null));

        // The next stream: RFI off, as the retry after a decoder crash always was.
        assertFalse(hevcRfi(stream(null)));
    }

    @Test
    public void giveUpsShowNoSecondDialogAndResetNothing() {
        SharedPreferences settings = PreferenceManager.getDefaultSharedPreferences(activity.get());
        settings.edit().putInt("seekbar_bitrate_kbps", 80_000).commit();
        // Three in a row: a crash count of 3 used to reset the streaming settings from the dialog.
        for (int i = 1; i <= 3; i++) {
            assertEquals(i, CrashAccounting.recordGiveUp(tombstone, "rfi-pc"));
            assertEquals(i, tombstone.getInt("LastNotifiedCrashCount", 0));
        }
        ShadowDialog.reset();
        UiHelper.showDecoderCrashDialog(activity.get());
        ShadowLooper.idleMainLooper();
        assertNull(ShadowDialog.getLatestDialog());
        assertEquals(80_000, settings.getInt("seekbar_bitrate_kbps", 0));
    }

    private static MediaCodecInfo lowLatencyHevc() {
        Map<String, List<String>> parameters = ReflectionHelpers.getStaticField(
                MediaCodecHelper.class, "vendorParameterCache");
        parameters.put(AML_HEVC, Arrays.asList("vendor.low-latency.enable"));
        MediaFormat capabilities = MediaFormat.createVideoFormat("video/hevc", 1920, 1080);
        capabilities.setFeatureEnabled(CodecCapabilities.FEATURE_LowLatency, true);
        return MediaCodecInfoBuilder.newBuilder().setName(AML_HEVC).setCapabilities(
                CodecCapabilitiesBuilder.newBuilder().setMediaFormat(capabilities)
                        .setProfileLevels(new CodecProfileLevel[0])
                        .setColorFormats(new int[] {CodecCapabilities.COLOR_FormatYUV420Flexible}).build()).build();
    }
}
