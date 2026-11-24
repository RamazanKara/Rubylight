package com.limelight.binding.audio;

import android.media.AudioDeviceInfo;
import android.media.AudioTrack;
import android.os.Build;

import androidx.annotation.RequiresApi;

import com.limelight.LimeLog;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.utils.ProblemReport;

/**
 * Compressed AC3/E-AC3 passthrough {@link AudioTrack} output for {@link AndroidAudioRenderer}.
 * Handles format validation, route-change rebuilds, late-frame dropping, and diagnostics on Android 10+.
 */
final class AudioEncodedOutput {
    private final AndroidAudioRenderer renderer;
    private AudioTrack track;
    private int codec, bufferBytes;
    // A passthrough output that cannot be rebuilt says so once per stream, not once a second.
    private boolean failureShown;
    private long nextRecreateAt, nextWriteWarningAt;

    AudioEncodedOutput(AndroidAudioRenderer renderer) {
        this.renderer = renderer;
    }

    int setup(MoonBridge.AudioConfiguration audioConfiguration, int codec) {
        failureShown = false;
        renderer.channels = audioConfiguration.channelCount;
        renderer.sampleRate = AudioPassthrough.SAMPLE_RATE;
        this.codec = codec;
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) return failed("needs Android 10 or later");
        if (AudioPassthrough.encoding(codec) == 0 || AudioPassthrough.channelMask(renderer.channels) == 0) {
            return failed(AudioPassthrough.names(codec) + " with " + renderer.channels + " channels is not a passthrough format");
        }
        return buildTrack();
    }

    private int buildTrack() {
        // setup checked already; this keeps the route-change rebuild on Android 10 too.
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) return failed("needs Android 10 or later");
        int encoding = AudioPassthrough.encoding(codec);
        int mask = AudioPassthrough.channelMask(renderer.channels);
        try {
            bufferBytes = Math.max(AudioTrack.getMinBufferSize(renderer.sampleRate, mask, encoding),
                    2 * AudioPassthrough.MAX_FRAME_BYTES);
            track = renderer.createEncodedTrack(encoding, mask, bufferBytes);
        } catch (RuntimeException e) {
            track = null;
            return failed("the audio output refused " + AudioPassthrough.names(codec) + ": " + e.getMessage());
        }
        LimeLog.info("Audio output: " + AudioPassthrough.names(codec) + " passthrough, " + renderer.channels + " channels");
        report();
        return 0;
    }

    // Loud: the log, the problem report and the stream's message line. Returns setup's error.
    private int failed(String reason) {
        LimeLog.severe("Audio passthrough: " + reason);
        ProblemReport.setAudioDiagnostics("AudioTrack passthrough failed: codec=" + AudioPassthrough.names(codec) +
                ", channels=" + renderer.channels + ", " + reason);
        if (!failureShown) {
            failureShown = true;
            MoonBridge.bridgeClDisplayMessage("Audio passthrough failed: " + reason + ". Turn off Audio passthrough to hear Opus.");
        }
        return -1;
    }

    private void report() {
        AudioTrack current = track;
        int routed = -1;
        if (current != null) {
            try {
                AudioDeviceInfo device = current.getRoutedDevice();
                if (device != null) routed = device.getType();
            } catch (RuntimeException ignored) {}
        }
        ProblemReport.setAudioDiagnostics("AudioTrack passthrough: codec=" + AudioPassthrough.names(codec) +
                ", channels=" + renderer.channels + ", bufferBytes=" + bufferBytes +
                ", route=" + AudioDiagnostics.route(routed, new int[0]) + ", " +
                AudioDiagnostics.drops(renderer.lateDropped, renderer.droppedWrites));
    }

    @RequiresApi(Build.VERSION_CODES.Q)
    static AudioTrack createTrack(int encoding, int channelMask, int bufferBytes) {
        AudioTrack created = new AudioTrack.Builder()
                .setAudioFormat(AudioPassthrough.format(encoding, channelMask))
                .setAudioAttributes(AudioPassthrough.attributes())
                .setTransferMode(AudioTrack.MODE_STREAM)
                .setBufferSizeInBytes(bufferBytes)
                .build();
        created.play();
        return created;
    }

    // A lost frame never reaches here: the sink hears a gap, never a broken frame.
    void play(byte[] frame, int length) {
        // The same 40 ms bound on pending audio as decoded packets.
        if (renderer.pendingAudioMs() >= 40) {
            renderer.lateDropped++;
            return;
        }
        int written = renderer.writeEncoded(frame, length);
        if (written == length) return;
        renderer.droppedWrites++;
        long now = renderer.uptimeMillis();
        if (written == AudioTrack.ERROR_DEAD_OBJECT && now >= nextRecreateAt) {
            // A route change ends a compressed track; the new route may not take the format.
            nextRecreateAt = now + AudioTrackBackend.TRACK_RECREATE_INTERVAL_MS;
            LimeLog.warning("Passthrough AudioTrack disconnected; recreating it");
            release();
            buildTrack();
        } else if (now >= nextWriteWarningAt) {
            nextWriteWarningAt = now + AudioTrackBackend.TRACK_RECREATE_INTERVAL_MS;
            LimeLog.warning("Passthrough AudioTrack write failed: " + written + " of " + length + " bytes; dropping the frame");
            report();
        }
    }

    // Blocking and whole: a compressed track takes only complete frames.
    int write(byte[] frame, int length) {
        AudioTrack current = track;
        if (current == null) return AudioTrack.ERROR_DEAD_OBJECT;
        return current.write(frame, 0, length, AudioTrack.WRITE_BLOCKING);
    }

    void release() {
        AudioTrack oldTrack = track;
        track = null;
        AudioOutputSupport.releaseTrack(oldTrack, "Passthrough AudioTrack was already stopped during release");
    }
}
