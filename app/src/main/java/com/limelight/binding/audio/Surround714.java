package com.limelight.binding.audio;

import android.content.Context;
import android.media.AudioDeviceInfo;
import android.media.AudioFormat;
import android.media.AudioManager;
import android.media.AudioTrack;
import android.os.Build;

/**
 * Whether this device plays 7.1.4 (12 channels), so the audio setting offers it: Android 12L or
 * later takes the 7.1.4 channel mask, and an output reports 12 or more channels. The stream asks
 * for 7.1 otherwise. The request on the wire is Moonlight V+'s
 * (MoonBridge.AUDIO_CONFIGURATION_714_SURROUND).
 */
public final class Surround714 {
    public static final int CHANNELS = 12;

    private Surround714() {
    }

    /** Android's 7.1.4 mask (FL FR FC LFE BL BR SL SR TFL TFR TBL TBR), 0 before Android 12L. */
    static int channelMask() {
        return Build.VERSION.SDK_INT >= Build.VERSION_CODES.S_V2 ? AudioFormat.CHANNEL_OUT_7POINT1POINT4 : 0;
    }

    public static boolean playable(Context context) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.S_V2) {
            return false;
        }
        AudioManager manager = context.getSystemService(AudioManager.class);
        if (manager == null) {
            return false;
        }
        boolean trackAccepts;
        try {
            trackAccepts = AudioTrack.getMinBufferSize(48000, channelMask(), AudioFormat.ENCODING_PCM_16BIT) > 0;
        } catch (RuntimeException e) {
            trackAccepts = false;
        }
        AudioDeviceInfo[] outputs = manager.getDevices(AudioManager.GET_DEVICES_OUTPUTS);
        int[][] channelCounts = new int[outputs.length][];
        for (int i = 0; i < outputs.length; i++) {
            channelCounts[i] = outputs[i].getChannelCounts();
        }
        return offered(Build.VERSION.SDK_INT, trackAccepts, channelCounts);
    }

    /** The decision {@link #playable} makes from what it read, kept apart so it is testable. */
    static boolean offered(int sdk, boolean trackAccepts, int[][] outputChannelCounts) {
        if (sdk < Build.VERSION_CODES.S_V2 || !trackAccepts) {
            return false;
        }
        for (int[] counts : outputChannelCounts) {
            for (int count : counts) {
                if (count >= CHANNELS) {
                    return true;
                }
            }
        }
        return false;
    }
}
