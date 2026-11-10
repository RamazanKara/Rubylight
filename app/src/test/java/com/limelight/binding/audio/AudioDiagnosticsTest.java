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

    @Test public void aRoutedDeviceIsNamedAloneAndAnUnknownOneListsTheAttachedOutputsAsSuch() {
        int[] attached = {AudioDeviceInfo.TYPE_BUILTIN_SPEAKER, AudioDeviceInfo.TYPE_BLUETOOTH_A2DP};
        assertEquals("bluetooth-a2dp", AudioDiagnostics.route(AudioDeviceInfo.TYPE_BLUETOOTH_A2DP, attached));
        assertEquals("unknown (attached: speaker+bluetooth-a2dp)", AudioDiagnostics.route(-1, attached));
        assertEquals("unknown (attached: none)", AudioDiagnostics.route(-1, null));
    }

    @Test public void dropsSayWhetherTheQueueOrTheOutputLostTheBuffers() {
        assertEquals("lateDropped=3, failedWrites=0", AudioDiagnostics.drops(3, 0));
    }

    @Test public void aPacedLineComesOnceASecondAndSaysHowManyItStandsFor() {
        AudioDiagnostics.Paced paced = new AudioDiagnostics.Paced();
        // The first is said at once, whatever the clock reads.
        assertEquals(1, paced.count(0));
        // 5 ms packets for the rest of that second: none of them is a line.
        for (long now = 5; now < AudioDiagnostics.Paced.EVERY_MS; now += 5) {
            assertEquals(0, paced.count(now));
        }
        // The next line stands for all of them and the one that brought it.
        assertEquals(200, paced.count(AudioDiagnostics.Paced.EVERY_MS));
        assertEquals(0, paced.count(AudioDiagnostics.Paced.EVERY_MS + 999));
        // Long after, a single one is again said at once, with the one still held.
        assertEquals(2, paced.count(60_000));
        assertEquals(1, paced.count(120_000));
    }

    @Test public void eachKindKeepsItsOwnPace() {
        AudioDiagnostics.Paced trims = new AudioDiagnostics.Paced();
        AudioDiagnostics.Paced drops = new AudioDiagnostics.Paced();
        assertEquals(1, trims.count(1000));
        assertEquals(1, drops.count(1001));
        assertEquals(0, trims.count(1500));
        assertEquals(2, trims.count(2000));
        assertEquals(0, drops.count(2000));
    }
}
