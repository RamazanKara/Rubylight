package com.limelight.binding.audio;

import com.limelight.nvstream.av.audio.AudioRenderer;
import com.limelight.nvstream.jni.MoonBridge;

/** The audio of a small-stream Input-only session: received for the host's sake, never played. */
public class SilentAudioRenderer implements AudioRenderer {
    @Override
    public int setup(MoonBridge.AudioConfiguration audioConfiguration, int sampleRate, int samplesPerFrame) {
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
