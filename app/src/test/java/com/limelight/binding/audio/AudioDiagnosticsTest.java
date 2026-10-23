package com.limelight.binding.audio;

import android.media.AudioDeviceInfo;

import org.junit.Test;

import static org.junit.Assert.assertEquals;

public class AudioDiagnosticsTest {
    @Test public void routesNameTheDeviceKindsThatMatterForCrackling() {
        assertEquals("speaker", AudioDiagnostics.routes(new int[] {AudioDeviceInfo.TYPE_BUILTIN_SPEAKER}));
        assertEquals("speaker+bluetooth-a2dp+usb-headset", AudioDiagnostics.routes(new int[] {
                AudioDeviceInfo.TYPE_BUILTIN_SPEAKER, AudioDeviceInfo.TYPE_BLUETOOTH_A2DP,
                AudioDeviceInfo.TYPE_USB_HEADSET}));
        assertEquals("ble-headset", AudioDiagnostics.routes(new int[] {26}));
    }

    @Test public void unknownAndMissingRoutesAreStillReported() {
        assertEquals("none", AudioDiagnostics.routes(new int[0]));
        assertEquals("none", AudioDiagnostics.routes(null));
        assertEquals("type-999", AudioDiagnostics.typeName(999));
    }

    @Test public void dropsSayWhetherTheQueueOrTheOutputLostTheBuffers() {
        assertEquals("lateDropped=3, failedWrites=0", AudioDiagnostics.drops(3, 0));
    }
}
