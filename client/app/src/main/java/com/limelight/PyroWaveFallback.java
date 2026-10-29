package com.limelight;

import com.limelight.nvstream.jni.MoonBridge;

/**
 * PyroWave chosen in the settings while the stream runs another codec: this device cannot run it,
 * or the PC did not offer it. Either way the player is told once which codec is used instead.
 */
final class PyroWaveFallback {
    private PyroWaveFallback() {}

    /** True when PyroWave was chosen and the stream that started carries video in another codec. */
    static boolean fellBack(boolean pyroWaveChosen, int activeFormat) {
        return pyroWaveChosen && activeFormat != 0 && (activeFormat & MoonBridge.VIDEO_FORMAT_MASK_PYROWAVE) == 0;
    }

    /** The codec of {@code format} as the settings name it. */
    static String codecName(int format) {
        if ((format & MoonBridge.VIDEO_FORMAT_MASK_AV1) != 0) return "AV1";
        if ((format & MoonBridge.VIDEO_FORMAT_MASK_H265) != 0) return "HEVC";
        return "H.264";
    }
}
