package com.limelight.binding.audio;

import android.media.AudioAttributes;
import android.media.AudioFormat;
import android.media.AudioTrack;
import android.os.Build;

import androidx.annotation.RequiresApi;

/**
 * Which passthrough formats the current audio route plays without mixing, as the
 * {@code x-rl-audio} codec bits of rubylight_protocol::audio_ext. The Rust engine asks a Rubylight
 * PC for one of them and keeps Opus when there is none, so a stream never picks a format the
 * output would play as silence.
 */
public final class AudioPassthrough {
    public static final int CODEC_PCM = 2;
    public static final int CODEC_AC3 = 4;
    public static final int CODEC_EAC3 = 8;

    /** The host's PCM, AC3 and E-AC3 all run at 48 kHz. */
    static final int SAMPLE_RATE = 48000;
    /** The largest AC3 or E-AC3 frame the Rust engine hands on, in bytes. */
    static final int MAX_FRAME_BYTES = 4096;

    private AudioPassthrough() {
    }

    interface DirectPlayback {
        boolean supports(int encoding, int channelMask);
    }

    /** The codecs the route plays for {@code channels}; 0 before Android 10, which cannot tell. */
    public static int playableCodecs(int channels) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) {
            return playableCodecs(Build.VERSION.SDK_INT, channels, null);
        }
        return playableCodecs(Build.VERSION.SDK_INT, channels, AudioPassthrough::directPlayback);
    }

    static int playableCodecs(int sdk, int channels, DirectPlayback direct) {
        int mask = channelMask(channels);
        if (sdk < Build.VERSION_CODES.Q || mask == 0) return 0;
        int codecs = 0;
        // Multichannel PCM only where the route takes it as is: an HDMI ARC soundbar gets the
        // stream as AC3 or E-AC3 instead of a stereo downmix.
        if (direct.supports(AudioFormat.ENCODING_PCM_16BIT, mask)) codecs |= CODEC_PCM;
        if (direct.supports(AudioFormat.ENCODING_AC3, mask)) codecs |= CODEC_AC3;
        if (direct.supports(AudioFormat.ENCODING_E_AC3, mask)) codecs |= CODEC_EAC3;
        return codecs;
    }

    /** The codec bits as text for logs: "PCM, AC3", or "none". */
    public static String names(int codecs) {
        StringBuilder text = new StringBuilder();
        if ((codecs & CODEC_PCM) != 0) text.append("PCM");
        if ((codecs & CODEC_AC3) != 0) text.append(text.length() > 0 ? ", " : "").append("AC3");
        if ((codecs & CODEC_EAC3) != 0) text.append(text.length() > 0 ? ", " : "").append("E-AC3");
        return text.length() > 0 ? text.toString() : "none";
    }

    /** AudioFormat's encoding for AC3 or E-AC3, 0 for any other codec. */
    static int encoding(int codec) {
        switch (codec) {
            case CODEC_AC3: return AudioFormat.ENCODING_AC3;
            case CODEC_EAC3: return AudioFormat.ENCODING_E_AC3;
            default: return 0;
        }
    }

    /** The output channel mask for the stream's channel count, 0 when passthrough has none. */
    static int channelMask(int channels) {
        switch (channels) {
            case 2: return AudioFormat.CHANNEL_OUT_STEREO;
            case 6: return AudioFormat.CHANNEL_OUT_5POINT1;
            case 8: return AudioFormat.CHANNEL_OUT_7POINT1_SURROUND;
            case Surround714.CHANNELS: return Surround714.channelMask();
            default: return 0;
        }
    }

    /** Media rather than game usage: the route's decoders and passthrough paths are set up for it. */
    static AudioAttributes attributes() {
        return new AudioAttributes.Builder()
                .setUsage(AudioAttributes.USAGE_MEDIA)
                .setContentType(AudioAttributes.CONTENT_TYPE_MOVIE)
                .build();
    }

    static AudioFormat format(int encoding, int channelMask) {
        return new AudioFormat.Builder()
                .setEncoding(encoding)
                .setSampleRate(SAMPLE_RATE)
                .setChannelMask(channelMask)
                .build();
    }

    @RequiresApi(Build.VERSION_CODES.Q)
    private static boolean directPlayback(int encoding, int channelMask) {
        try {
            return AudioTrack.isDirectPlaybackSupported(format(encoding, channelMask), attributes());
        } catch (RuntimeException e) {
            return false;
        }
    }
}
