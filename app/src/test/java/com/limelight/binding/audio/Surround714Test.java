package com.limelight.binding.audio;

import android.media.AudioFormat;

import com.limelight.nvstream.http.HostCapabilities;
import com.limelight.nvstream.http.TestHosts;
import com.limelight.nvstream.jni.MoonBridge;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 33, shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class Surround714Test {
    private static final int S_V2 = 32;

    @Test public void offeredOnlyWhereAnOutputReportsTwelveChannels() {
        int[][] phone = {{1, 2}, {2}};
        int[][] interfaceWith12 = {{2}, {2, 6, 8, 12}};
        assertFalse(Surround714.offered(S_V2, true, phone));
        assertTrue(Surround714.offered(S_V2, true, interfaceWith12));
        assertTrue(Surround714.offered(S_V2, true, new int[][] {{16}}));
        // Android before 12L has no 7.1.4 mask, and a mask AudioTrack refuses is never offered.
        assertFalse(Surround714.offered(S_V2 - 1, true, interfaceWith12));
        assertFalse(Surround714.offered(S_V2, false, interfaceWith12));
        // HDMI's 8 PCM channels are 7.1, not 7.1.4; no outputs at all offer nothing.
        assertFalse(Surround714.offered(S_V2, true, new int[][] {{2, 6, 8}}));
        assertFalse(Surround714.offered(S_V2, true, new int[0][]));
    }

    @Test public void theRequestIsMoonlightVPlusAndTheMaskIsAndroids714() {
        // MoonBridge's constants need its native static initializer, which the shadow skips; this is
        // AUDIO_CONFIGURATION_714_SURROUND's request as V+ sends it.
        assertEquals(0xF63F000C, new MoonBridge.AudioConfiguration(Surround714.CHANNELS, 0xF63F).getSurroundAudioInfo());
        assertEquals(AudioFormat.CHANNEL_OUT_7POINT1POINT4, Surround714.channelMask());
        assertEquals(12, Integer.bitCount(Surround714.channelMask()));
        assertEquals(Surround714.channelMask(), AudioPassthrough.channelMask(12));
    }

    @Test public void onlyAHostThatReports714IsAskedForIt() {
        MoonBridge.AudioConfiguration wanted = new MoonBridge.AudioConfiguration(Surround714.CHANNELS, 0xF63F);
        // Rubylight while it streams 7.1.4 (AudioLayouts 15) is asked for it.
        assertSame(wanted, Surround714.forHost(wanted, TestHosts.fixture("rubylight-30-serverinfo.xml")));
        // Rubylight with 7.1.4 off (AudioLayouts 7), stock Sunshine, and Foundation Sunshine, whose
        // serverinfo reads as Sunshine's, get 7.1; so does a host not heard from yet.
        HostCapabilities[] without = {TestHosts.fixture("rubylight-30-refused-serverinfo.xml"),
                TestHosts.fixture("sunshine-serverinfo.xml"), TestHosts.fixture("apollo-serverinfo.xml"),
                TestHosts.fixture("vibepollo-serverinfo.xml"), TestHosts.fixture("gamestream-serverinfo.xml"),
                HostCapabilities.UNKNOWN};
        for (HostCapabilities caps : without) {
            MoonBridge.AudioConfiguration asked = Surround714.forHost(wanted, caps);
            assertEquals(caps.toString(), 8, asked.channelCount);
            assertEquals(caps.toString(), 0x63F, asked.channelMask);
        }
        // Stereo, 5.1 and 7.1 are asked of every host as picked.
        MoonBridge.AudioConfiguration surround71 = new MoonBridge.AudioConfiguration(8, 0x63F);
        assertSame(surround71, Surround714.forHost(surround71, TestHosts.fixture("sunshine-serverinfo.xml")));
        MoonBridge.AudioConfiguration stereo = new MoonBridge.AudioConfiguration(2, 0x3);
        assertSame(stereo, Surround714.forHost(stereo, HostCapabilities.UNKNOWN));
    }
}
