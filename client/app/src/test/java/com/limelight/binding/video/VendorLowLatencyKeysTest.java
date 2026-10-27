package com.limelight.binding.video;

import org.junit.Test;

import java.util.Arrays;
import java.util.Map;

import static org.junit.Assert.*;

public class VendorLowLatencyKeysTest {
    @Test public void snapdragonEliteGetsLowLatencyPictureOrderAndSoftwareFence() {
        Map<String, Integer> keys = VendorLowLatencyKeys.forSupported(Arrays.asList(
                "vendor.qti-ext-dec-low-latency.enable",
                "vendor.qti-ext-dec-picture-order.enable",
                "vendor.qti-ext-output-fence.enable",
                "vendor.qti-ext-output-fence.fence_type",
                "vendor.qti-ext-dec-heif-mode.value"));
        assertEquals(Integer.valueOf(1), keys.get("vendor.qti-ext-dec-low-latency.enable"));
        assertEquals(Integer.valueOf(1), keys.get("vendor.qti-ext-dec-picture-order.enable"));
        assertEquals(Integer.valueOf(1), keys.get("vendor.qti-ext-output-fence.enable"));
        assertEquals(Integer.valueOf(1), keys.get("vendor.qti-ext-output-fence.fence_type"));
        assertFalse(keys.containsKey("vendor.qti-ext-dec-heif-mode.value"));
    }

    @Test public void onlyDeclaredKeysAreSent() {
        Map<String, Integer> keys = VendorLowLatencyKeys.forSupported(Arrays.asList("vendor.rtc-ext-dec-low-latency.enable"));
        assertEquals(1, keys.size());
        assertTrue(VendorLowLatencyKeys.forSupported(Arrays.<String>asList()).isEmpty());
    }

    @Test public void unknownVendorLowLatencySwitchesAreFoundByName() {
        Map<String, Integer> keys = VendorLowLatencyKeys.forSupported(Arrays.asList(
                "vendor.sprd-dec-low-latency.enable",
                "vendor.foo.lowlatency.mode",
                "vendor.foo.low-latency.disable",
                "vendor.foo.ultra-low-latency.enable",
                "vendor.hisi-ext-low-latency-video-dec.video-scene-for-low-latency-rdy",
                "vendor.foo.low-latency.status"));
        assertEquals(2, keys.size());
        assertTrue(keys.containsKey("vendor.sprd-dec-low-latency.enable"));
        assertTrue(keys.containsKey("vendor.foo.lowlatency.mode"));
    }

    @Test public void coreSwitchesSurviveTheLastFallback() {
        assertTrue(VendorLowLatencyKeys.isCoreSwitch("vendor.qti-ext-dec-low-latency.enable"));
        assertTrue(VendorLowLatencyKeys.isCoreSwitch("vendor.mtk.vdec.low-latency.mode"));
        assertFalse(VendorLowLatencyKeys.isCoreSwitch("vendor.qti-ext-output-fence.fence_type"));
        assertFalse(VendorLowLatencyKeys.isCoreSwitch("vendor.qti-ext-dec-picture-order.enable"));
    }

    @Test public void snapdragonGetsItsFenceAndLowLatencyKeysEvenWhenUndeclared() {
        Map<String, Integer> keys = VendorLowLatencyKeys.qualcommKeys();
        assertEquals(Integer.valueOf(1), keys.get("vendor.qti-ext-dec-low-latency.enable"));
        assertEquals(Integer.valueOf(1), keys.get("vendor.qti-ext-dec-picture-order.enable"));
        assertEquals(Integer.valueOf(1), keys.get("vendor.qti-ext-output-sw-fence-enable.value"));
        assertEquals(Integer.valueOf(1), keys.get("vendor.qti-ext-output-fence.enable"));
        // 1 is the software fence.
        assertEquals(Integer.valueOf(1), keys.get("vendor.qti-ext-output-fence.fence_type"));
        assertEquals(5, keys.size());
    }
}
