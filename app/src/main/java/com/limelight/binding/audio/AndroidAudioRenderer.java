package com.limelight.binding.audio;

import android.content.Context;
import android.content.Intent;
import android.media.AudioAttributes;
import android.media.AudioFormat;
import android.media.AudioManager;
import android.media.AudioTrack;
import android.media.audiofx.AudioEffect;
import android.os.Build;
import android.os.SystemClock;

import com.limelight.LimeLog;
import com.limelight.nvstream.av.audio.AudioRenderer;
import com.limelight.nvstream.jni.MoonBridge;

public class AndroidAudioRenderer implements AudioRenderer {

    private final Context context;
    private final boolean enableAudioFx;

    private AudioTrack track;
    private long nativeAudio;
    private AudioBufferPolicy bufferPolicy;
    private long lastBufferCheckMs;
    private int channelConfig, sampleRate, samplesPerFrame, channels;
    private boolean stopped;
    private boolean audioFxSessionOpen;

    private static native long nativeOpenAudio(int sampleRate);
    private static native boolean nativeWriteAudio(long handle, short[] samples, int length);
    private static native int[] nativeAudioStats(long handle);
    private static native int nativeAudioBuffer(long handle, int frames);
    private static native void nativeCloseAudio(long handle);

    public AndroidAudioRenderer(Context context, boolean enableAudioFx) {
        this.context = context;
        this.enableAudioFx = enableAudioFx;
    }

    private AudioTrack createAudioTrack(int channelConfig, int sampleRate, int bufferSize, boolean lowLatency) {
        AudioAttributes.Builder attributesBuilder = new AudioAttributes.Builder()
                .setUsage(AudioAttributes.USAGE_GAME);
        AudioFormat format = new AudioFormat.Builder()
                .setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                .setSampleRate(sampleRate)
                .setChannelMask(channelConfig)
                .build();

        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) {
            // Use FLAG_LOW_LATENCY on L through N
            if (lowLatency) {
                attributesBuilder.setFlags(AudioAttributes.FLAG_LOW_LATENCY);
            }
        }

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            AudioTrack.Builder trackBuilder = new AudioTrack.Builder()
                    .setAudioFormat(format)
                    .setAudioAttributes(attributesBuilder.build())
                    .setTransferMode(AudioTrack.MODE_STREAM)
                    .setBufferSizeInBytes(bufferSize);

            // Use PERFORMANCE_MODE_LOW_LATENCY on O and later
            if (lowLatency) {
                trackBuilder.setPerformanceMode(AudioTrack.PERFORMANCE_MODE_LOW_LATENCY);
            }

            return trackBuilder.build();
        }
        else {
            return new AudioTrack(attributesBuilder.build(),
                    format,
                    bufferSize,
                    AudioTrack.MODE_STREAM,
                    AudioManager.AUDIO_SESSION_ID_GENERATE);
        }
    }

    @Override
    public synchronized int setup(MoonBridge.AudioConfiguration audioConfiguration, int sampleRate, int samplesPerFrame) {
        this.sampleRate = sampleRate;
        this.samplesPerFrame = samplesPerFrame;
        this.channels = audioConfiguration.channelCount;
        stopped = false;

        switch (audioConfiguration.channelCount)
        {
            case 2:
                channelConfig = AudioFormat.CHANNEL_OUT_STEREO;
                break;
            case 4:
                channelConfig = AudioFormat.CHANNEL_OUT_QUAD;
                break;
            case 6:
                channelConfig = AudioFormat.CHANNEL_OUT_5POINT1;
                break;
            case 8:
                // AudioFormat.CHANNEL_OUT_7POINT1_SURROUND isn't available until Android 6.0,
                // yet the CHANNEL_OUT_SIDE_LEFT and CHANNEL_OUT_SIDE_RIGHT constants were added
                // in 5.0, so just hardcode the constant so we can work on Lollipop.
                channelConfig = 0x000018fc; // AudioFormat.CHANNEL_OUT_7POINT1_SURROUND
                break;
            default:
                LimeLog.severe("Decoder returned unhandled channel count");
                return -1;
        }

        LimeLog.info("Audio channel config: "+String.format("0x%X", channelConfig));

        if (AudioBufferPolicy.useAAudio(Build.VERSION.SDK_INT, channels, enableAudioFx, sampleRate,
                AudioTrack.getNativeOutputSampleRate(AudioManager.STREAM_MUSIC))) {
            nativeAudio = nativeOpenAudio(sampleRate);
            if (nativeAudio != 0) {
                int[] stats = nativeAudioStats(nativeAudio);
                bufferPolicy = new AudioBufferPolicy(stats[0], stats[1], stats[2], stats[3], SystemClock.uptimeMillis());
                LimeLog.info("Audio output: AAudio low latency");
                return 0;
            }
        }
        return setupAudioTrack();
    }

    private int setupAudioTrack() {
        bufferPolicy = null;
        lastBufferCheckMs = 0;
        int bytesPerFrame = channels * samplesPerFrame * 2;

        // We're not supposed to request less than the minimum
        // buffer size for our buffer, but it appears that we can
        // do this on many devices and it lowers audio latency.
        // We'll try the small buffer size first and if it fails,
        // use the recommended larger buffer size.

        for (int i = 0; i < 4; i++) {
            boolean lowLatency;
            int bufferSize;

            // We will try:
            // 1) Small buffer, low latency mode
            // 2) Large buffer, low latency mode
            // 3) Small buffer, standard mode
            // 4) Large buffer, standard mode

            switch (i) {
                case 0:
                case 1:
                    lowLatency = true;
                    break;
                case 2:
                case 3:
                    lowLatency = false;
                    break;
                default:
                    // Unreachable
                    throw new IllegalStateException();
            }

            switch (i) {
                case 0:
                case 2:
                    bufferSize = bytesPerFrame * 2;
                    break;

                case 1:
                case 3:
                    // Try the larger buffer size
                    bufferSize = Math.max(AudioTrack.getMinBufferSize(sampleRate,
                            channelConfig,
                            AudioFormat.ENCODING_PCM_16BIT),
                            bytesPerFrame * 2);

                    // Round to next frame
                    bufferSize = (((bufferSize + (bytesPerFrame - 1)) / bytesPerFrame) * bytesPerFrame);
                    break;
                default:
                    // Unreachable
                    throw new IllegalStateException();
            }

            // Skip low latency options if hardware sample rate doesn't match the content
            if (AudioTrack.getNativeOutputSampleRate(AudioManager.STREAM_MUSIC) != sampleRate && lowLatency) {
                continue;
            }

            // Skip low latency options when using audio effects, since low latency mode
            // precludes the use of the audio effect pipeline (as of Android 13).
            if (enableAudioFx && lowLatency) {
                continue;
            }

            try {
                int capacityBytes = Build.VERSION.SDK_INT >= Build.VERSION_CODES.N ?
                        Math.max(bufferSize, bytesPerFrame * 8) : bufferSize;
                track = createAudioTrack(channelConfig, sampleRate, capacityBytes, lowLatency);
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
                    int actual = track.setBufferSizeInFrames(bufferSize / (channels * 2));
                    bufferPolicy = new AudioBufferPolicy(samplesPerFrame, track.getBufferCapacityInFrames(),
                            actual > 0 ? actual : track.getBufferSizeInFrames(), track.getUnderrunCount(), SystemClock.uptimeMillis());
                }
                track.play();

                // Successfully created working AudioTrack. We're done here.
                LimeLog.info("Audio track configuration: "+bufferSize+" "+lowLatency);
                break;
            } catch (Exception e) {
                // Try to release the AudioTrack if we got far enough
                e.printStackTrace();
                try {
                    if (track != null) {
                        track.release();
                        track = null;
                    }
                } catch (Exception ignored) {}
            }
        }

        if (track == null) {
            // Couldn't create any audio track for playback
            return -2;
        }

        return 0;
    }

    @Override
    public synchronized void playDecodedAudio(short[] audioData, int length) {
        if (stopped) return;
        // Only queue up to 40 ms of pending audio data in addition to what AudioTrack is buffering for us.
        if (MoonBridge.getPendingAudioDuration() < 40) {
            if (nativeAudio != 0) {
                if (nativeWriteAudio(nativeAudio, audioData, length)) {
                    tuneBuffer();
                    return;
                }
                nativeCloseAudio(nativeAudio);
                nativeAudio = 0;
                bufferPolicy = null;
                LimeLog.warning("AAudio disconnected; falling back to AudioTrack");
                if (setupAudioTrack() != 0) {
                    stopped = true;
                    return;
                }
            }
            // This will block until the write is completed. That can cause a backlog
            // of pending audio data, so we do the above check to be able to bound
            // latency at 40 ms in that situation.
            if (!writeAudioTrackFully(audioData, length)) {
                stopped = true;
                releaseAudioTrack();
                return;
            }
            tuneBuffer();
        }
        else {
            LimeLog.info("Too much pending audio data: " + MoonBridge.getPendingAudioDuration() +" ms");
        }
    }

    // Package-visible write/recreation hooks keep recovery testable without an audio device.
    int writeAudioTrack(short[] audioData, int offset, int length) {
        return track.write(audioData, offset, length);
    }

    boolean recreateAudioTrack() {
        boolean reopenAudioFx = audioFxSessionOpen;
        releaseAudioTrack();
        if (setupAudioTrack() != 0) {
            return false;
        }
        if (reopenAudioFx) {
            setAudioFxSession(true);
        }
        return true;
    }

    boolean writeAudioTrackFully(short[] audioData, int length) {
        int offset = 0;
        boolean recreated = false;
        while (offset < length) {
            int written = writeAudioTrack(audioData, offset, length - offset);
            if (written > 0 && written <= length - offset) {
                offset += written;
            }
            else if (written == AudioTrack.ERROR_DEAD_OBJECT && !recreated) {
                // A route change can invalidate AudioTrack. Retry only the unwritten samples,
                // and at most once per buffer so a broken output cannot spin on this thread.
                recreated = true;
                LimeLog.warning("AudioTrack disconnected; recreating audio output");
                if (!recreateAudioTrack()) {
                    LimeLog.warning("Unable to recreate AudioTrack");
                    return false;
                }
            }
            else {
                LimeLog.warning("AudioTrack write made no progress or failed: " + written);
                return false;
            }
        }
        return true;
    }

    private void tuneBuffer() {
        long now = SystemClock.uptimeMillis();
        if (bufferPolicy == null || now - lastBufferCheckMs < 1000) return;
        lastBufferCheckMs = now;
        if (nativeAudio != 0) {
            int[] stats = nativeAudioStats(nativeAudio);
            int target = bufferPolicy.sample(stats[3], now);
            if (target != stats[2]) bufferPolicy.applied(nativeAudioBuffer(nativeAudio, target));
        } else if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
            int target = bufferPolicy.sample(track.getUnderrunCount(), now);
            if (target != track.getBufferSizeInFrames()) bufferPolicy.applied(track.setBufferSizeInFrames(target));
        }
    }

    private void setAudioFxSession(boolean open) {
        if (!enableAudioFx || track == null || audioFxSessionOpen == open) {
            return;
        }
        Intent i = new Intent(open ? AudioEffect.ACTION_OPEN_AUDIO_EFFECT_CONTROL_SESSION :
                AudioEffect.ACTION_CLOSE_AUDIO_EFFECT_CONTROL_SESSION);
        i.putExtra(AudioEffect.EXTRA_AUDIO_SESSION, track.getAudioSessionId());
        i.putExtra(AudioEffect.EXTRA_PACKAGE_NAME, context.getPackageName());
        if (open) {
            i.putExtra(AudioEffect.EXTRA_CONTENT_TYPE, AudioEffect.CONTENT_TYPE_GAME);
        }
        context.sendBroadcast(i);
        audioFxSessionOpen = open;
    }

    @Override
    public synchronized void start() {
        // Allow equalizers to attach to this track, including a replacement after disconnect.
        setAudioFxSession(true);
    }

    @Override
    public synchronized void stop() {
        stopped = true;
        setAudioFxSession(false);
    }

    @Override
    public synchronized void cleanup() {
        stopped = true;
        if (nativeAudio != 0) {
            nativeCloseAudio(nativeAudio);
            nativeAudio = 0;
        }
        releaseAudioTrack();
    }

    private void releaseAudioTrack() {
        setAudioFxSession(false);
        AudioTrack oldTrack = track;
        track = null;
        bufferPolicy = null;
        if (oldTrack == null) return;
        try {
            // Immediately drop pending data, but still release an already invalidated track.
            oldTrack.pause();
            oldTrack.flush();
        } catch (IllegalStateException e) {
            LimeLog.warning("AudioTrack was already stopped during release");
        } finally {
            oldTrack.release();
        }
    }
}
