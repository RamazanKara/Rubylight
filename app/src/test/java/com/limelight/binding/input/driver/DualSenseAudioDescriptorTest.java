package com.limelight.binding.input.driver;

import org.junit.Test;
import java.util.Arrays;
import static org.junit.Assert.*;

public class DualSenseAudioDescriptorTest {
    // Audio descriptors captured from a physical DualSense by nondebug/dualsense:
    // https://github.com/nondebug/dualsense/blob/main/lsusb-descriptor-info.txt
    private static byte[] descriptors() {
        return new byte[] {
                9, 4, 0, 0, 0, 1, 1, 0, 0,
                12, 0x24, 6, 2, 1, 1, 3, 0, 0, 0, 0, 0,
                9, 4, 1, 0, 0, 1, 2, 0, 0,
                9, 4, 1, 1, 1, 1, 2, 0, 0,
                7, 0x24, 1, 1, 1, 1, 0,
                11, 0x24, 2, 1, 4, 2, 16, 1, (byte)0x80, (byte)0xbb, 0,
                9, 5, 1, 9, (byte)0x88, 1, 4, 0, 0,
                7, 0x25, 1, 0, 0, 0, 0};
    }

    @Test
    public void selectsStereoMicrophoneAndItsFeatureUnit() {
        byte[] descriptors = {
                9, 4, 0, 0, 0, 1, 1, 0, 0,
                10, 0x24, 6, 5, 4, 1, 3, 0, 0, 0,
                9, 0x24, 3, 6, 1, 1, 0, 5, 0,
                9, 4, 2, 1, 1, 1, 2, 0, 0,
                7, 0x24, 1, 6, 1, 1, 0,
                11, 0x24, 2, 1, 2, 2, 16, 1, (byte)0x80, (byte)0xbb, 0,
                9, 5, (byte)0x82, 5, (byte)196, 0, 4, 0, 0 };
        DualSenseAudioDescriptor result = DualSenseAudioDescriptor.findMicrophone(descriptors);
        assertNotNull(result);
        assertEquals(2, result.streamingInterface);
        assertEquals(0x82, result.endpoint);
        assertEquals(5, result.featureUnit);
        assertNull(DualSenseAudioDescriptor.find(descriptors));
    }

    @Test
    public void selectsOnlyTheFourChannelPlaybackAlternate() {
        DualSenseAudioDescriptor result = DualSenseAudioDescriptor.find(descriptors());
        assertNotNull(result);
        assertEquals(0, result.controlInterface);
        assertEquals(1, result.streamingInterface);
        assertEquals(1, result.alternateSetting);
        assertEquals(1, result.endpoint);
        assertEquals(4, result.interval);
        assertEquals(2, result.featureUnit);
    }

    @Test
    public void rejectsDifferentFormatsInputEndpointsAndBrokenDescriptorLengths() {
        for (int[] mutation : new int[][] {{50, 2}, {52, 24}, {55, 0}, {44, 3},
                {59, 0x81}, {60, 1}, {61, 0}, {62, 0}, {0, 0}, {66, 100}, {15, 0}, {13, 4}}) {
            byte[] data = descriptors();
            data[mutation[0]] = (byte)mutation[1];
            assertNull("offset " + mutation[0], DualSenseAudioDescriptor.find(data));
        }
        byte[] full = descriptors();
        assertNull(DualSenseAudioDescriptor.find(Arrays.copyOf(full, full.length - 1)));
        assertNull(DualSenseAudioDescriptor.find(null));
    }
}
