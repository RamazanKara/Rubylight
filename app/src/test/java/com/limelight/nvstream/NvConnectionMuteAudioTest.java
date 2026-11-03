package com.limelight.nvstream;

import android.app.Application;

import com.limelight.PcViewLifecycleTest;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class NvConnectionMuteAudioTest {
    @Test public void aMutedStreamAsksForNoPassthroughWithUncompressedAudioOn() {
        for (int channels : new int[] {2, 6, 8}) {
            assertEquals(0, NvConnection.audioPassthroughCodecs(true, true, channels));
            assertEquals(0, NvConnection.audioPassthroughCodecs(false, true, channels));
        }
    }
}
