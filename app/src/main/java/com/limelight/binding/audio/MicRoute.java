package com.limelight.binding.audio;

import android.content.Context;
import android.media.AudioDeviceCallback;
import android.media.AudioDeviceInfo;
import android.media.AudioManager;
import android.os.Build;
import android.os.Handler;
import android.os.Looper;

/**
 * Whether game audio plays into the user's ears rather than out of this device's speaker. Automatic
 * echo cancellation uses it: a headset or headphones keep the speaker out of the microphone, and
 * cancelling an echo that is not there only dulls the voice.
 */
public class MicRoute {
    // AudioDeviceInfo.TYPE_BLE_HEADSET (API 31) and TYPE_HEARING_AID (API 28), by value for older SDKs.
    static final int TYPE_BLE_HEADSET = 26, TYPE_HEARING_AID = 23;

    /** Whether one of these output device types plays privately (headphones, a headset, a hearing aid). */
    public static boolean privateOutput(int[] outputTypes) {
        for (int type : outputTypes) {
            switch (type) {
                case AudioDeviceInfo.TYPE_WIRED_HEADSET:
                case AudioDeviceInfo.TYPE_WIRED_HEADPHONES:
                case AudioDeviceInfo.TYPE_BLUETOOTH_A2DP:
                case AudioDeviceInfo.TYPE_BLUETOOTH_SCO:
                case AudioDeviceInfo.TYPE_USB_HEADSET:
                case TYPE_BLE_HEADSET:
                case TYPE_HEARING_AID:
                    return true;
                default:
                    break;
            }
        }
        return false;
    }

    private final AudioManager audio;
    private final Handler handler = new Handler(Looper.getMainLooper());
    private AudioDeviceCallback callback;

    public MicRoute(Context context) {
        audio = (AudioManager) context.getApplicationContext().getSystemService(Context.AUDIO_SERVICE);
    }

    /** Whether game audio plays privately now. Before Android 6 the route is unknown and reads as the speaker. */
    public boolean privateOutput() {
        if (audio == null || Build.VERSION.SDK_INT < Build.VERSION_CODES.M) return false;
        AudioDeviceInfo[] devices = audio.getDevices(AudioManager.GET_DEVICES_OUTPUTS);
        int[] types = new int[devices.length];
        for (int i = 0; i < devices.length; i++) types[i] = devices[i].getType();
        return privateOutput(types);
    }

    /** Runs {@code changed} when an output device comes or goes; null stops watching. */
    public void watch(Runnable changed) {
        if (audio == null || Build.VERSION.SDK_INT < Build.VERSION_CODES.M) return;
        if (callback != null) {
            audio.unregisterAudioDeviceCallback(callback);
            callback = null;
        }
        if (changed == null) return;
        callback = new AudioDeviceCallback() {
            @Override public void onAudioDevicesAdded(AudioDeviceInfo[] added) { changed.run(); }
            @Override public void onAudioDevicesRemoved(AudioDeviceInfo[] removed) { changed.run(); }
        };
        audio.registerAudioDeviceCallback(callback, handler);
    }
}
