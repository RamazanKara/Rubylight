package com.limelight.binding.video;

import android.app.Activity;
import android.app.Application;
import android.graphics.SurfaceTexture;
import android.media.MediaCodecInfo;
import android.media.MediaFormat;
import android.view.Surface;

import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.utils.RecentStreams;
import com.limelight.utils.WiredLink;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.MediaCodecInfoBuilder;
import org.robolectric.shadows.ShadowBuild;
import org.robolectric.shadows.ShadowSystemClock;
import org.robolectric.util.ReflectionHelpers;
import org.robolectric.util.ReflectionHelpers.ClassParameter;

import java.time.Duration;

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

/** A finished stream leaves one row in the report, with what the renderer saw and no address. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class,
        shadows = {KeyframeSafetyNetWiringTest.QueueingCodec.class, com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class},
        instrumentedPackages = "com.limelight.nvstream.jni")
public class RecentStreamRowTest {
    private ActivityController<Activity> activity;
    private SurfaceTexture texture;
    private Surface surface;
    private MediaCodecDecoderRenderer renderer;
    private int frame;

    @Before
    public void setUp() {
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
        RecentStreams.clear();
        activity = Robolectric.buildActivity(Activity.class).create();
        texture = new SurfaceTexture(0);
        surface = new Surface(texture);
    }

    @After
    public void tearDown() {
        surface.release();
        texture.release();
        activity.close();
    }

    private void stream(String model, String decoder, int bytes) {
        ShadowBuild.setModel(model);
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.videoFormat = PreferenceConfiguration.FormatOption.FORCE_HEVC;
        prefs.codecLowLatency = true;
        prefs.vendorLowLatency = true;
        prefs.rustEngine = true;
        renderer = new MediaCodecDecoderRenderer(activity.get(), prefs, null, 0, false, false, "", null);
        ReflectionHelpers.setField(renderer, "renderTarget", surface);
        ReflectionHelpers.setField(renderer, "videoFormat", MoonBridge.VIDEO_FORMAT_H265);
        ReflectionHelpers.setField(renderer, "initialWidth", 3840);
        ReflectionHelpers.setField(renderer, "initialHeight", 2160);
        ReflectionHelpers.setField(renderer, "refreshRate", 60);
        ReflectionHelpers.setField(renderer, "streamStartMs", 0L);
        ReflectionHelpers.setField(renderer, "streamStartUptimeMs", android.os.SystemClock.uptimeMillis());
        ReflectionHelpers.callInstanceMethod(renderer, "tryConfigureDecoder",
                ClassParameter.from(MediaCodecInfo.class, MediaCodecInfoBuilder.newBuilder().setName(decoder).build()),
                ClassParameter.from(MediaFormat.class, MediaFormat.createVideoFormat("video/hevc", 3840, 2160)),
                ClassParameter.from(boolean.class, true));
        submit(bytes, MoonBridge.FRAME_TYPE_IDR);
        submit(64, MoonBridge.FRAME_TYPE_PFRAME);
        submit(bytes / 2, MoonBridge.FRAME_TYPE_IDR);
        ShadowSystemClock.advanceBy(Duration.ofMillis(97_000));
    }

    private void submit(int bytes, int frameType) {
        frame++;
        renderer.submitDecodeUnit(new byte[bytes], bytes, MoonBridge.BUFFER_TYPE_PICDATA, frame, frameType, (char) 0,
                frame * 16_000L, frame * 16_000L, frame * 16_000_000L, frame * 16_000L);
    }

    @Test
    public void theRowSaysWhatTheStreamWas() {
        RecentStreams.noteLink(new WiredLink.Hints(1000, false));
        stream("AFTNE4E43F", "c2.mtk.hevc.decoder", 512);
        renderer.cleanup();
        String section = RecentStreams.section();
        for (String part : new String[] {"1970-01-01T00:00:00Z 97 s", "c2.mtk.hevc.decoder video/hevc", "3840x2160 60 fps",
                "HDR off", "engine new", "low latency on", "vendor on", "RFI off", "keyframe net on",
                "frames in 3 ", "keyframes 2, largest 512 B", "net requests 0", "input waits 0", "input buffer 4096 B",
                "link wired 1000 Mbps"}) {
            assertTrue(part + " in " + section, section.contains(part));
        }
    }

    @Test
    public void anotherDecoderShowsItsNetOffAndCleaningUpTwiceAddsOneRow() {
        stream("AFTMM", "c2.mtk.hevc.decoder", 128);
        renderer.cleanup();
        renderer.cleanup();
        String section = RecentStreams.section();
        assertTrue(section, section.contains("keyframe net off"));
        assertTrue(section, section.indexOf("c2.mtk.hevc.decoder") == section.lastIndexOf("c2.mtk.hevc.decoder"));
    }

    @Test
    public void aStreamThatNeverStartedLeavesNoRow() {
        ShadowBuild.setModel("AFTMM");
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.videoFormat = PreferenceConfiguration.FormatOption.FORCE_HEVC;
        new MediaCodecDecoderRenderer(activity.get(), prefs, null, 0, false, false, "", null).cleanup();
        assertTrue(RecentStreams.section(), RecentStreams.section().contains("None finished"));
        assertFalse(RecentStreams.section().contains("engine"));
    }
}
