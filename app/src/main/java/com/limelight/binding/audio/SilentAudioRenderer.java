package com.limelight.binding.audio;

import com.limelight.nvstream.av.audio.AudioRenderer;
import com.limelight.nvstream.jni.MoonBridge;

/** Audio received for the host's sake and never played: a small-stream Input-only session, or Mute audio. */
public class SilentAudioRenderer implements AudioRenderer {
    @Override
    public int setup(MoonBridge.AudioConfiguration audioConfiguration, int sampleRate, int samplesPerFrame) {
        return 0;
    }

    /** Accepts passthrough frames too, so a stream that negotiated AC3 or E-AC3 still starts. */
    @Override
    public int setupEncoded(MoonBridge.AudioConfiguration audioConfiguration, int codec) {
        return 0;
    }

    @Override
    public void start() {
    }

    @Override
    public void stop() {
    }

    @Override
    public void playDecodedAudio(short[] audioData, int length) {
    }

    @Override
    public void cleanup() {
    }
}
