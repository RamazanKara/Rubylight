package com.limelight.binding.audio;

import android.content.Context;
import android.media.AudioDeviceCallback;
import android.media.AudioDeviceInfo;
import android.media.AudioManager;
import android.os.Handler;
import android.os.Looper;

import com.limelight.mic.MicOnlySession;

/**
 * Whether game audio plays into the user's ears rather than out of this device's speaker. Automatic
 * echo cancellation uses it: a headset or headphones keep the speaker out of the microphone, and
 * cancelling an echo that is not there only dulls the voice. It also picks the microphone while
 * classic Bluetooth headphones play, so turning the microphone on keeps them in stereo.
 */
public class MicRoute {
    // AudioDeviceInfo.TYPE_BLE_HEADSET (API 31) and TYPE_HEARING_AID (API 28), by value for older SDKs.
    static final int TYPE_BLE_HEADSET = 26, TYPE_HEARING_AID = 23;
    // TYPE_BLE_SPEAKER (API 31) and TYPE_BLE_BROADCAST (API 33): the other LE Audio device types.
    static final int TYPE_BLE_SPEAKER = 27, TYPE_BLE_BROADCAST = 30;

    /**
     * The input type the microphone should prefer, or 0 to leave the choice to Android. Classic
     * Bluetooth headphones (A2DP) and no LE Audio device: the built-in microphone, because opening the
     * headset's own microphone drops the headphones to the hands-free profile (8 or 16 kHz mono)
     * for game audio too. Android keeps its choice while a call or voice app owns the audio route
     * ({@code inCall}), when a wired or USB microphone is attached, and when an LE Audio headset,
     * whose microphone works without that drop, is present.
     */
    static int preferredInput(int[] outputTypes, int[] inputTypes, boolean inCall) {
        if (inCall || !contains(outputTypes, AudioDeviceInfo.TYPE_BLUETOOTH_A2DP)) return 0;
        if (leAudio(outputTypes) || leAudio(inputTypes)) return 0;
        boolean builtIn = false;
        for (int type : inputTypes) {
            switch (type) {
                case AudioDeviceInfo.TYPE_BUILTIN_MIC:
                    builtIn = true;
                    break;
                case AudioDeviceInfo.TYPE_BLUETOOTH_SCO:
                case AudioDeviceInfo.TYPE_BLUETOOTH_A2DP:
                case AudioDeviceInfo.TYPE_TELEPHONY:
                case AudioDeviceInfo.TYPE_FM_TUNER:
                case AudioDeviceInfo.TYPE_TV_TUNER:
                case AudioDeviceInfo.TYPE_REMOTE_SUBMIX:
                    break;
                default:
                    // A wired, USB or other microphone the user plugged in.
                    return 0;
            }
        }
        return builtIn ? AudioDeviceInfo.TYPE_BUILTIN_MIC : 0;
    }

    private static boolean leAudio(int[] types) {
        return contains(types, TYPE_BLE_HEADSET) || contains(types, TYPE_BLE_SPEAKER)
                || contains(types, TYPE_BLE_BROADCAST);
    }

    private static boolean contains(int[] types, int wanted) {
        for (int type : types) {
            if (type == wanted) return true;
        }
        return false;
    }

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

    /** Whether game audio plays privately now. The route reads as the speaker when no output device is attached. */
    public boolean privateOutput() {
        if (audio == null) return false;
        return privateOutput(types(audio.getDevices(AudioManager.GET_DEVICES_OUTPUTS)));
    }

    /**
     * The microphone {@link #preferredInput(int[], int[], boolean)} picks for the current devices, or
     * null to leave the choice to Android.
     */
    public AudioDeviceInfo preferredInput() {
        if (audio == null) return null;
        AudioDeviceInfo[] inputs = audio.getDevices(AudioManager.GET_DEVICES_INPUTS);
        int wanted = preferredInput(types(audio.getDevices(AudioManager.GET_DEVICES_OUTPUTS)), types(inputs),
                MicOnlySession.isCall(audio.getMode()));
        if (wanted == 0) return null;
        for (AudioDeviceInfo device : inputs) {
            if (device.getType() == wanted) return device;
        }
        return null;
    }

    private static int[] types(AudioDeviceInfo[] devices) {
        int[] types = new int[devices.length];
        for (int i = 0; i < devices.length; i++) types[i] = devices[i].getType();
        return types;
    }

    /** Runs {@code changed} when an output device comes or goes; null stops watching. */
    public void watch(Runnable changed) {
        if (audio == null) return;
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
