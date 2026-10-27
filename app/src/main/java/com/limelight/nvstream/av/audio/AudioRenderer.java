package com.limelight.nvstream.av.audio;

import com.limelight.nvstream.jni.MoonBridge;

public interface AudioRenderer {
    int setup(MoonBridge.AudioConfiguration audioConfiguration, int sampleRate, int samplesPerFrame);

    void start();

    void stop();
    
    void playDecodedAudio(short[] audioData, int length);

    /**
     * Opens a passthrough output for whole AC3 or E-AC3 frames ({@code codec} is
     * {@code AudioPassthrough.CODEC_AC3} or {@code CODEC_EAC3}). Returns 0, or a negative error after
     * saying why: a renderer without passthrough refuses, so the stream never plays silence.
     */
    default int setupEncoded(MoonBridge.AudioConfiguration audioConfiguration, int codec) {
        return -1;
    }

    /** One whole AC3 or E-AC3 frame, after {@link #setupEncoded}. */
    default void playEncodedAudio(byte[] frame, int length) {
    }
    
    void cleanup();
}
