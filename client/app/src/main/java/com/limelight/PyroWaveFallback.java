package com.limelight;

import com.limelight.binding.video.MediaCodecDecoderRenderer;
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

    enum Notice { NONE, DEVICE, PC, PC_WITHOUT_HDR, FULL_CHROMA }

    /**
     * Which notice a fallback gets. dropReason: the renderer's getPyroWaveDropReason().
     * hdrLossSaid: PyroWave HDR failed on this device and HDR streams in another codec, which the
     * connection has said already. The PC is blamed only when its codecs lack PyroWave (HDR).
     */
    static Notice notice(int dropReason, boolean hdrLossSaid) {
        switch (dropReason) {
            case MediaCodecDecoderRenderer.PYROWAVE_NOT_OFFERED: return Notice.PC;
            case MediaCodecDecoderRenderer.PYROWAVE_NO_HDR_ON_PC: return Notice.PC_WITHOUT_HDR;
            case MediaCodecDecoderRenderer.PYROWAVE_FULL_CHROMA: return Notice.FULL_CHROMA;
            default: return dropReason <= 0 || hdrLossSaid ? Notice.NONE : Notice.DEVICE;
        }
    }

    /** The codec of {@code format} as the settings name it. */
    static String codecName(int format) {
        if ((format & MoonBridge.VIDEO_FORMAT_MASK_AV1) != 0) return "AV1";
        if ((format & MoonBridge.VIDEO_FORMAT_MASK_H265) != 0) return "HEVC";
        return "H.264";
    }
}
