package com.limelight.binding.audio;

import android.media.AudioDeviceInfo;

import org.junit.Test;

import static org.junit.Assert.assertEquals;

/** The microphone choice under Bluetooth headphones: stereo A2DP never drops to hands-free. */
public class MicRouteTest {
    private static final int[] PHONE_MICS = {AudioDeviceInfo.TYPE_BUILTIN_MIC, AudioDeviceInfo.TYPE_BLUETOOTH_SCO};
    private static final int[] A2DP = {AudioDeviceInfo.TYPE_BUILTIN_SPEAKER, AudioDeviceInfo.TYPE_BLUETOOTH_A2DP};

    @Test public void classicBluetoothHeadphonesRecordFromTheBuiltInMicrophone() {
        assertEquals(AudioDeviceInfo.TYPE_BUILTIN_MIC, MicRoute.preferredInput(A2DP, PHONE_MICS, false));
        assertEquals(AudioDeviceInfo.TYPE_BUILTIN_MIC, MicRoute.preferredInput(
                new int[] {AudioDeviceInfo.TYPE_BLUETOOTH_A2DP, AudioDeviceInfo.TYPE_BLUETOOTH_SCO}, PHONE_MICS, false));
    }

    @Test public void anLeAudioHeadsetKeepsItsOwnMicrophone() {
        assertEquals(0, MicRoute.preferredInput(
                new int[] {AudioDeviceInfo.TYPE_BLUETOOTH_A2DP, MicRoute.TYPE_BLE_HEADSET}, PHONE_MICS, false));
        assertEquals(0, MicRoute.preferredInput(A2DP,
                new int[] {AudioDeviceInfo.TYPE_BUILTIN_MIC, MicRoute.TYPE_BLE_HEADSET}, false));
        assertEquals(0, MicRoute.preferredInput(
                new int[] {AudioDeviceInfo.TYPE_BLUETOOTH_A2DP, MicRoute.TYPE_BLE_SPEAKER}, PHONE_MICS, false));
        assertEquals(0, MicRoute.preferredInput(
                new int[] {AudioDeviceInfo.TYPE_BLUETOOTH_A2DP, MicRoute.TYPE_BLE_BROADCAST}, PHONE_MICS, false));
    }

    @Test public void androidKeepsItsChoiceWithoutClassicBluetoothHeadphones() {
        assertEquals(0, MicRoute.preferredInput(new int[] {AudioDeviceInfo.TYPE_BUILTIN_SPEAKER}, PHONE_MICS, false));
        assertEquals(0, MicRoute.preferredInput(new int[] {AudioDeviceInfo.TYPE_WIRED_HEADPHONES}, PHONE_MICS, false));
        assertEquals(0, MicRoute.preferredInput(new int[] {AudioDeviceInfo.TYPE_BLUETOOTH_SCO}, PHONE_MICS, false));
        assertEquals(0, MicRoute.preferredInput(new int[0], new int[0], false));
    }

    @Test public void aPluggedInMicrophoneOrACallWins() {
        assertEquals(0, MicRoute.preferredInput(A2DP,
                new int[] {AudioDeviceInfo.TYPE_BUILTIN_MIC, AudioDeviceInfo.TYPE_USB_HEADSET}, false));
        assertEquals(0, MicRoute.preferredInput(A2DP,
                new int[] {AudioDeviceInfo.TYPE_BUILTIN_MIC, AudioDeviceInfo.TYPE_WIRED_HEADSET}, false));
        assertEquals(0, MicRoute.preferredInput(A2DP, PHONE_MICS, true));
    }

    @Test public void noBuiltInMicrophoneLeavesTheChoiceToAndroid() {
        assertEquals(0, MicRoute.preferredInput(A2DP, new int[] {AudioDeviceInfo.TYPE_BLUETOOTH_SCO}, false));
    }
}
