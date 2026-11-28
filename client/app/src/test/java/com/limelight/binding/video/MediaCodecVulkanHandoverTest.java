package com.limelight.binding.video;

import android.app.Activity;
import android.app.Application;
import android.graphics.SurfaceTexture;
import android.media.MediaCodec;
import android.media.MediaCrypto;
import android.media.MediaFormat;
import android.view.Surface;

import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.preferences.PreferenceConfiguration;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.Implementation;
import org.robolectric.annotation.Implements;
import org.robolectric.shadows.ShadowBuild;
import org.robolectric.shadows.ShadowMediaCodec;
import org.robolectric.util.ReflectionHelpers;
import org.robolectric.util.ReflectionHelpers.ClassParameter;

import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.Assert.*;

/**
 * Regression tests for {@link MediaCodecVulkanHelper#handOverFromVulkan()} (T8-2): when Vulkan
 * fails to stop within the handover timeout, {@link MediaCodecSession#configureAndStartDecoder}
 * must abort cleanly without configuring or starting MediaCodec on the contended surface, and
 * {@link MediaCodecRecovery} must preserve {@code vulkanFailure}.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class,
        shadows = {MediaCodecVulkanHandoverTest.TrackingCodec.class,
                MediaCodecVulkanHandoverTest.StuckVulkanBridge.class,
                com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class},
        instrumentedPackages = {"com.limelight.nvstream.jni", "com.limelight.binding.video"})
public class MediaCodecVulkanHandoverTest {
    @Implements(MediaCodec.class)
    public static class TrackingCodec extends ShadowMediaCodec {
        static final AtomicInteger configureCalls = new AtomicInteger();
        static final AtomicInteger startCalls = new AtomicInteger();

        @Implementation public void configure(MediaFormat format, Surface surface, MediaCrypto crypto, int flags) {
            configureCalls.incrementAndGet();
        }
        @Implementation public void start() {
            startCalls.incrementAndGet();
        }
        @Implementation public void stop() {}
        @Implementation public void reset() {}
        @Implementation public void release() {}
    }

    @Implements(value = VulkanRendererBridge.class, isInAndroidSdk = false)
    public static class StuckVulkanBridge {
        static boolean stopResult = false;
        static VulkanRendererBridge createResult = null;
        static final AtomicInteger stopCalls = new AtomicInteger();

        @Implementation public boolean stop() {
            stopCalls.incrementAndGet();
            return stopResult;
        }

        @Implementation public static VulkanRendererBridge create(
                Surface output, int streamWidth, int streamHeight, int streamFps,
                int framePacing, int jitterBuffer, int ditherMode, int colorspace,
                boolean fullRange, boolean tenBit, boolean hlg, float displayRefreshHz,
                String codec, String decoderName, String traceDirectory,
                boolean frontBuffer, long frameWarpApi) {
            return createResult;
        }

        @Implementation public boolean drawsFrameWarp() { return true; }
        @Implementation public boolean reportsPresentTimes() { return true; }
        @Implementation public void setVsyncOffset(long offsetNs) {}
        @Implementation public void setAdaptiveRefresh(boolean adaptive) {}
        @Implementation public void setHdrMode(boolean enabled, byte[] metadata) {}
        @Implementation public Surface getDecoderSurface() { return null; }
    }

    private ActivityController<Activity> activity;
    private SurfaceTexture texture;
    private Surface surface;
    private MediaCodecDecoderRenderer renderer;

    @Before
    public void setUp() throws Exception {
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
        TrackingCodec.configureCalls.set(0);
        TrackingCodec.startCalls.set(0);
        StuckVulkanBridge.stopCalls.set(0);
        StuckVulkanBridge.stopResult = false;
        StuckVulkanBridge.createResult = null;
        activity = Robolectric.buildActivity(Activity.class).create();
        texture = new SurfaceTexture(0);
        surface = new Surface(texture);
        ShadowBuild.setModel("Pixel 8");
        PreferenceConfiguration prefs = new PreferenceConfiguration();
        prefs.videoFormat = PreferenceConfiguration.FormatOption.FORCE_HEVC;
        prefs.bitrate = 20_000;
        renderer = new MediaCodecDecoderRenderer(activity.get(), prefs, null, 0, false, false, "", null);
        renderer.renderTarget = surface;
        renderer.videoFormat = MoonBridge.VIDEO_FORMAT_H265;
        renderer.configuredFormat = MediaFormat.createVideoFormat("video/hevc", 1920, 1080);
        renderer.videoDecoder = MediaCodec.createByCodecName("c2.android.hevc.decoder");
        renderer.vulkanRenderer = ReflectionHelpers.callConstructor(
                VulkanRendererBridge.class, ClassParameter.from(long.class, 1L));
        ReflectionHelpers.setField(renderer.vulkanHelper, "vulkanHandoverPending", true);
    }

    @After
    public void tearDown() {
        surface.release();
        texture.release();
        activity.close();
    }

    @Test
    public void timedOutVulkanStopAbortsConfigureAndStartAndSetsStopping() {
        assertFalse(renderer.session.configureAndStartDecoder(renderer.configuredFormat));
        assertEquals(1, StuckVulkanBridge.stopCalls.get());
        assertEquals("MediaCodec.configure must not be called when Vulkan stop timed out",
                0, TrackingCodec.configureCalls.get());
        assertEquals("MediaCodec.start must not be called when Vulkan stop timed out",
                0, TrackingCodec.startCalls.get());
        assertTrue("renderer must enter stopping state", renderer.stopping);
        assertFalse("handover flag must be cleared",
                ReflectionHelpers.<Boolean>getField(renderer.vulkanHelper, "vulkanHandoverPending"));
        assertNotNull("vulkanFailure must be recorded", renderer.vulkanHelper.getVulkanFailure());
    }

    @Test
    public void recoveryAbortsCleanlyAndPreservesVulkanFailureOnHandoverTimeout() {
        renderer.codecRecoveryType.set(MediaCodecDecoderRenderer.CR_RECOVERY_TYPE_RESTART);
        renderer.restartReason = "HDR shown as SDR";
        renderer.codecRecoveryThreadQuiescedFlags =
                MediaCodecDecoderRenderer.CR_FLAG_ALL & ~MediaCodecDecoderRenderer.CR_FLAG_INPUT_THREAD;

        assertTrue(renderer.recovery.doCodecRecoveryIfRequired(MediaCodecDecoderRenderer.CR_FLAG_INPUT_THREAD));
        assertEquals(0, TrackingCodec.configureCalls.get());
        assertEquals(0, TrackingCodec.startCalls.get());
        assertTrue(renderer.stopping);
        assertNotNull("vulkanFailure preserved", renderer.vulkanHelper.getVulkanFailure());
        assertNull("decoderFailure must not overwrite vulkanFailure", renderer.getDecoderFailure());
    }

    @Test
    public void alreadyStoppingRendererAbortsHandoverWithoutSpuriousVulkanFailure() {
        renderer.stopping = true;
        assertFalse(renderer.vulkanHelper.handOverFromVulkan());
        assertEquals(0, StuckVulkanBridge.stopCalls.get());
        assertFalse(ReflectionHelpers.<Boolean>getField(renderer.vulkanHelper, "vulkanHandoverPending"));
        assertNull(renderer.vulkanHelper.getVulkanFailure());
    }

    @Test
    public void midStreamSwitchToVulkanQuitsChoreographerThreadAndGuardsOnVsync() throws Exception {
        ReflectionHelpers.setField(renderer.vulkanHelper, "vulkanHandoverPending", false);
        renderer.vulkanRenderer = null;
        renderer.wantVulkan = true;
        ReflectionHelpers.setField(renderer, "outputVsyncAvailable", true);
        renderer.vsyncPacer.startChoreographerThread();
        android.os.HandlerThread choreoThread = renderer.vsyncPacer.choreographerHandlerThread;
        assertNotNull(choreoThread);
        assertTrue(choreoThread.isAlive());

        VulkanRendererBridge nextVulkan = ReflectionHelpers.callConstructor(
                VulkanRendererBridge.class, ClassParameter.from(long.class, 2L));
        StuckVulkanBridge.createResult = nextVulkan;

        assertTrue(renderer.session.configureAndStartDecoder(renderer.configuredFormat));
        assertSame(nextVulkan, renderer.vulkanRenderer);
        assertNull("choreographerHandlerThread must be cleared on Vulkan switch",
                renderer.vsyncPacer.choreographerHandlerThread);
        renderer.vsyncPacer.joinChoreographerThread();
        assertFalse("choreographer thread must exit after Vulkan switch", choreoThread.isAlive());

        // A stray vsync callback arriving while vulkanRenderer != null must be ignored cleanly.
        renderer.vsyncTimeNs = 0;
        renderer.vsyncPacer.doFrame(123_456_789L);
        assertEquals("onVsync must return early when vulkanRenderer is active", 0L, renderer.vsyncTimeNs);
    }

    @Test
    public void closeUpscalerForVulkanHandoverPreservesFrameWarp() {
        renderer.upscalingPolicy = new UpscalingPolicy(60);
        renderer.vulkanHelper.frameWarp = true;
        GlesUpscaler upscaler = org.robolectric.shadow.api.Shadow.newInstanceOf(GlesUpscaler.class);
        android.os.HandlerThread fakeThread = new android.os.HandlerThread("StubUpscalerHandover");
        fakeThread.start();
        ReflectionHelpers.setField(upscaler, "frameWarp", true);
        ReflectionHelpers.setField(upscaler, "thread", fakeThread);
        ReflectionHelpers.setField(upscaler, "handler", new android.os.Handler(fakeThread.getLooper()));
        ReflectionHelpers.setField(upscaler, "display", android.opengl.EGL14.EGL_NO_DISPLAY);
        renderer.upscaler = upscaler;

        renderer.vulkanHelper.closeUpscaler(true);

        assertNull(renderer.upscaler);
        assertTrue("frameWarp must stay enabled for Vulkan handover", renderer.vulkanHelper.frameWarp);
        assertFalse("frameWarp notice must not fire on Vulkan handover",
                ReflectionHelpers.<Boolean>getField(renderer.vulkanHelper, "frameWarpNoticeShown"));
    }
}
