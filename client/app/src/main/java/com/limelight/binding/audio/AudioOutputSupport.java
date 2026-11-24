package com.limelight.binding.audio;

import android.content.Context;
import android.content.Intent;
import android.media.AudioAttributes;
import android.media.AudioDeviceInfo;
import android.media.AudioFormat;
import android.media.AudioManager;
import android.media.AudioTrack;
import android.media.audiofx.AudioEffect;
import android.os.Build;

import com.limelight.LimeLog;

/**
 * Stateless PCM format, track factory, routing query, mixing, and diagnostics formatting helpers
 * shared by {@link AndroidAudioRenderer} and its output backends.
 */
final class AudioOutputSupport {
    private AudioOutputSupport() {
    }

    static int channelConfig(int channelCount) {
        switch (channelCount) {
            case 2: return AudioFormat.CHANNEL_OUT_STEREO;
            case 4: return AudioFormat.CHANNEL_OUT_QUAD;
            case 6: return AudioFormat.CHANNEL_OUT_5POINT1;
            case 8:
                // AudioFormat.CHANNEL_OUT_7POINT1_SURROUND isn't available until Android 6.0,
                // yet the CHANNEL_OUT_SIDE_LEFT and CHANNEL_OUT_SIDE_RIGHT constants were added
                // in 5.0, so just hardcode the constant so we can work on Lollipop.
                return 0x000018fc; // AudioFormat.CHANNEL_OUT_7POINT1_SURROUND
            case Surround714.CHANNELS:
                // 7.1.4 in the decoder's order FL FR C LFE RL RR SL SR TFL TFR TBL TBR; Android 12L+.
                int mask = Surround714.channelMask();
                if (mask == 0) {
                    LimeLog.severe("7.1.4 needs Android 12L or later");
                }
                return mask;
            default:
                LimeLog.severe("Decoder returned unhandled channel count");
                return 0;
        }
    }

    static AudioTrack createPcmTrack(int channelConfig, int sampleRate, int bufferSize, boolean lowLatency) {
        AudioAttributes.Builder attributesBuilder = new AudioAttributes.Builder()
                .setUsage(AudioAttributes.USAGE_GAME);
        AudioFormat format = new AudioFormat.Builder()
                .setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                .setSampleRate(sampleRate)
                .setChannelMask(channelConfig)
                .build();

        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) {
            if (lowLatency) attributesBuilder.setFlags(AudioAttributes.FLAG_LOW_LATENCY);
            return new AudioTrack(attributesBuilder.build(), format, bufferSize,
                    AudioTrack.MODE_STREAM, AudioManager.AUDIO_SESSION_ID_GENERATE);
        }
        AudioTrack.Builder trackBuilder = new AudioTrack.Builder()
                .setAudioFormat(format)
                .setAudioAttributes(attributesBuilder.build())
                .setTransferMode(AudioTrack.MODE_STREAM)
                .setBufferSizeInBytes(bufferSize);
        if (lowLatency) trackBuilder.setPerformanceMode(AudioTrack.PERFORMANCE_MODE_LOW_LATENCY);
        return trackBuilder.build();
    }

    /** Fades pcm in from head over the samples both have, whole frames, as the AAudio stall fade-in does. */
    static void crossfade(short[] head, int headSamples, short[] pcm, int length, int channels) {
        if (channels <= 0) return;
        int frames = Math.min(headSamples, length) / channels;
        for (int i = 0; i < frames * channels; i++) {
            int step = i / channels;
            pcm[i] = (short)((pcm[i] * step + head[i] * (frames - step)) / frames);
        }
    }

    static void downmixStereo(short[] input, int length, int channels, short[] stereo) {
        for (int frame = 0; frame < length / channels; frame++) {
            for (int side = 0; side < 2; side++) {
                // The decoder supplies FL FR C LFE RL RR [SL SR]; four channels are quad, FL FR RL RR,
                // with neither C nor LFE. Mixing their index 2 as a centre gave both ears the left rear
                // and lost the right one.
                int base = frame * channels;
                float mix = input[base + side];
                int firstSurround;
                if (channels == 4) {
                    firstSurround = 2 + side;
                } else {
                    if (channels >= 3) mix += 0.7071f * input[base + 2];
                    firstSurround = 4 + side;
                }
                for (int c = firstSurround; c < channels; c += 2) mix += 0.7071f * input[base + c];
                stereo[frame * 2 + side] = (short)Math.max(-32768, Math.min(32767, Math.round(mix)));
            }
        }
    }

    static int framesToMs(int frames, int sampleRate) {
        return sampleRate > 0 ? (int)(frames * 1000L / sampleRate) : 0;
    }

    static float readMusicVolume(Context context) {
        AudioManager manager = (AudioManager)context.getSystemService(Context.AUDIO_SERVICE);
        return manager.getStreamVolume(AudioManager.STREAM_MUSIC) /
                (float)Math.max(1, manager.getStreamMaxVolume(AudioManager.STREAM_MUSIC));
    }

    /** AudioTrack.getPerformanceMode(), or -1 where the platform cannot say (before Android 8.0). */
    static int trackPerformanceMode(AudioTrack current) {
        if (current == null || Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return -1;
        try {
            return current.getPerformanceMode();
        } catch (RuntimeException e) {
            return -1;
        }
    }

    /** The output's native burst in frames, 0 when the property is missing or invalid. */
    static int outputFramesPerBuffer(Context context) {
        if (context == null) return 0;
        try {
            AudioManager manager = (AudioManager)context.getSystemService(Context.AUDIO_SERVICE);
            return AudioBufferPolicy.parseFrames(manager.getProperty(AudioManager.PROPERTY_OUTPUT_FRAMES_PER_BUFFER));
        } catch (RuntimeException e) {
            return 0;
        }
    }

    /** Device type the AudioTrack is routed to, or -1 while it is not routed yet. */
    static int routedTrackType(AudioTrack current) {
        if (current == null) return -1;
        try {
            AudioDeviceInfo device = current.getRoutedDevice();
            return device == null ? -1 : device.getType();
        } catch (RuntimeException e) {
            return -1;
        }
    }

    /** {id, type} of every attached output, the fallback when the routed device is unknown. */
    static int[][] attachedOutputs(Context context) {
        if (context == null) return new int[0][];
        try {
            AudioManager manager = (AudioManager)context.getSystemService(Context.AUDIO_SERVICE);
            AudioDeviceInfo[] devices = manager.getDevices(AudioManager.GET_DEVICES_OUTPUTS);
            int[][] attached = new int[devices.length][];
            for (int i = 0; i < devices.length; i++) attached[i] = new int[] {devices[i].getId(), devices[i].getType()};
            return attached;
        } catch (RuntimeException e) {
            return new int[0][];
        }
    }

    static void sendAudioFxSession(Context context, AudioTrack track, boolean open) {
        Intent i = new Intent(open ? AudioEffect.ACTION_OPEN_AUDIO_EFFECT_CONTROL_SESSION :
                AudioEffect.ACTION_CLOSE_AUDIO_EFFECT_CONTROL_SESSION);
        i.putExtra(AudioEffect.EXTRA_AUDIO_SESSION, track.getAudioSessionId());
        i.putExtra(AudioEffect.EXTRA_PACKAGE_NAME, context.getPackageName());
        if (open) i.putExtra(AudioEffect.EXTRA_CONTENT_TYPE, AudioEffect.CONTENT_TYPE_GAME);
        context.sendBroadcast(i);
    }

    static void releaseTrack(AudioTrack oldTrack, String stoppedWarning) {
        if (oldTrack == null) return;
        try {
            // Immediately drop pending data, but still release an already invalidated track.
            oldTrack.pause();
            oldTrack.flush();
        } catch (IllegalStateException e) {
            LimeLog.warning(stoppedWarning);
        } finally {
            oldTrack.release();
        }
    }

    static String trackSetupDiagnostics(int sampleRate, int channels, int samplesPerFrame,
                                        int bufferSize, boolean lowLatency,
                                        String pathDetails, String outputDetails) {
        return "AudioTrack: rate=" + sampleRate + ", channels=" + channels +
                ", packetFrames=" + samplesPerFrame + ", bufferBytes=" + bufferSize + ", lowLatency=" + lowLatency +
                pathDetails + outputDetails;
    }

    static String trackDiagnostics(int sampleRate, int channels, int samplesPerFrame,
                                   int bufferFrames, int capacityFrames, int underruns,
                                   String pathDetails, String outputDetails) {
        return "AudioTrack: rate=" + sampleRate + ", channels=" + channels +
                ", packetFrames=" + samplesPerFrame + ", bufferFrames=" + bufferFrames +
                ", capacityFrames=" + capacityFrames + ", underruns=" + underruns +
                pathDetails + outputDetails;
    }

    static String nativeDiagnostics(int sampleRate, int channels, int samplesPerFrame,
                                    int[] stats, String outputDetails) {
        return "AAudio: rate=" + sampleRate + ", channels=" + channels +
                ", packetFrames=" + samplesPerFrame + ", burstFrames=" + stats[0] +
                ", reserveFrames=" + stats[2] + ", capacityFrames=" + stats[1] +
                ", hardwareFrames=" + stats[4] + ", hardwareUnderruns=" + stats[5] + ", softwareUnderruns=" + stats[6] +
                outputDetails;
    }
}
