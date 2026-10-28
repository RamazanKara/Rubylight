package com.limelight.binding.video;

import android.app.Application;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import static org.junit.Assert.*;

/**
 * The TCL C6K's MediaTek decoder stops taking input after a few frames when its tuned output goes
 * to the Vulkan renderer, so the vendor keys stay off there (MediaCodecHelper.usesVendorLowLatency).
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, application = Application.class)
public class MediaCodecVulkanVendorKeysTest {
    @Before
    public void setUp() {
        ReflectionHelpers.setStaticField(MediaCodecHelper.class, "initialized", true);
    }

    @Test
    public void mediaTekDecodersGetNoVendorKeysWhenTheVulkanRendererDraws() {
        for (String name : new String[] {"c2.mtk.hevc.decoder", "c2.mtk.av1.decoder", "C2.MTK.avc.decoder",
                "omx.mtk.video.decoder.hevc"}) {
            assertFalse(name, MediaCodecHelper.usesVendorLowLatency(true, name, true));
            assertTrue(name, MediaCodecHelper.usesVendorLowLatency(true, name, false));
        }
    }

    @Test
    public void otherDecodersKeepTheirVendorKeysWithTheVulkanRenderer() {
        for (String name : new String[] {"c2.qti.hevc.decoder", "c2.exynos.hevc.decoder", "c2.amlogic.hevc.decoder",
                "c2.android.hevc.decoder"}) {
            assertTrue(name, MediaCodecHelper.usesVendorLowLatency(true, name, true));
        }
    }

    @Test
    public void theSettingStillTurnsThemOffEverywhere() {
        for (boolean vulkan : new boolean[] {false, true}) {
            assertFalse(MediaCodecHelper.usesVendorLowLatency(false, "c2.mtk.hevc.decoder", vulkan));
            assertFalse(MediaCodecHelper.usesVendorLowLatency(false, "c2.qti.hevc.decoder", vulkan));
        }
    }
}
