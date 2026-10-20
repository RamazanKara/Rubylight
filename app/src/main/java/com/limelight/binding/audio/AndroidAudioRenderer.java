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
import com.limelight.utils.ProblemReport;

public class AndroidAudioRenderer implements AudioRenderer {

    private final Context context;
    private final boolean enableAudioFx;
    public interface HeadphoneSink {
        boolean write(short[] pcm, int length, float volume);
        // Lets the audio thread skip AudioService volume reads while no headset route exists.
        default boolean active() { return true; }
    }
    private final HeadphoneSink headphoneSink;
    private long volumeReadAt;
    private float streamVolume = 1;
    private short[] stereo;

    private AudioTrack track;
    private long nativeAudio;
    private AudioBufferPolicy bufferPolicy;
    private long lastBufferCheckMs;
    private int channelConfig, sampleRate, samplesPerFrame, channels;
    private boolean stopped;
    private boolean audioFxSessionOpen;
    // The stream wants equalizers on its track: from start() until stop(), across track rebuilds.
    private boolean audioFxWanted;
    // A dead track is rebuilt at most this often; every other failure only drops one buffer.
    private static final long TRACK_RECREATE_INTERVAL_MS = 1000;
    // Before Android 7.0, write() returns the native status for a dead track (DEAD_OBJECT is -32)
    // instead of ERROR_DEAD_OBJECT. A run of dropped buffers rebuilds the track there too.
    private static final int FAILED_WRITES_BEFORE_RECREATE = 5;
    private long nextTrackRecreateAt, nextWriteWarningAt;
    private int failedWrites;

    private static native long nativeOpenAudio(int sampleRate);
    private static native boolean nativeWriteAudio(long handle, short[] samples, int length);
    private static native int[] nativeAudioStats(long handle);
    private static native int nativeAudioBuffer(long handle, int frames);
    private static native void nativeCloseAudio(long handle);

    public AndroidAudioRenderer(Context context, boolean enableAudioFx) {
        this(context, enableAudioFx, null);
    }
    public AndroidAudioRenderer(Context context, boolean enableAudioFx, HeadphoneSink headphoneSink) {
        this.context = context;
        this.enableAudioFx = enableAudioFx;
        this.headphoneSink = headphoneSink;
    }

    AudioTrack createAudioTrack(int channelConfig, int sampleRate, int bufferSize, boolean lowLatency) {
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
        lastBufferCheckMs = 0;

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
                reportNativeAudio(stats);
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
                ProblemReport.setAudioDiagnostics("AudioTrack: rate=" + sampleRate + ", channels=" + channels +
                        ", packetFrames=" + samplesPerFrame + ", bufferBytes=" + bufferSize + ", lowLatency=" + lowLatency);
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
    public void playDecodedAudio(short[] audioData, int length) {
        // AudioService calls run only for a live headset route, and never under the renderer
        // lock that stop() and cleanup() take from other threads.
        boolean headphones = headphoneSink != null && headphoneSink.active();
        if (headphones && uptimeMillis() - volumeReadAt >= 500) {
            AudioManager manager = (AudioManager)context.getSystemService(Context.AUDIO_SERVICE);
            streamVolume = manager.getStreamVolume(AudioManager.STREAM_MUSIC) /
                    (float)Math.max(1, manager.getStreamMaxVolume(AudioManager.STREAM_MUSIC));
            volumeReadAt = uptimeMillis();
        }
        playDecodedAudio(audioData, length, headphones);
    }

    private synchronized void playDecodedAudio(short[] audioData, int length, boolean headphones) {
        if (stopped) return;
        if (headphones && sampleRate == 48000 && channels >= 2) {
            short[] sound = audioData;
            int count = length;
            if (channels != 2) {
                count = length / channels * 2;
                if (stereo == null || stereo.length < count) stereo = new short[count];
                downmixStereo(audioData, length, channels, stereo);
                sound = stereo;
            }
            if (headphoneSink.write(sound, count, streamVolume)) return;
        }
        // Only queue up to 40 ms of pending audio data in addition to what AudioTrack is buffering for us.
        if (MoonBridge.getPendingAudioDuration() < 40) {
            if (nativeAudio != 0 && bufferPolicy.needsFallback() &&
                    !fallbackToAudioTrack("AAudio keeps underrunning at its maximum reserve")) return;
            if (nativeAudio != 0) {
                if (nativeWriteAudio(nativeAudio, audioData, length)) {
                    tuneBuffer();
                    return;
                }
                if (!fallbackToAudioTrack("AAudio disconnected")) {
                    // Later buffers retry the AudioTrack like any other dead output.
                    return;
                }
            }
            // This will block until the write is completed. That can cause a backlog
            // of pending audio data, so we do the above check to be able to bound
            // latency at 40 ms in that situation.
            // A failed write drops only this buffer; the session's audio keeps playing.
            if (writeAudioTrackFully(audioData, length)) {
                tuneBuffer();
            }
        }
        else {
            LimeLog.info("Too much pending audio data: " + MoonBridge.getPendingAudioDuration() +" ms");
        }
    }

    boolean fallbackToAudioTrack(String reason) {
        LimeLog.warning(reason + "; falling back to AudioTrack");
        nativeCloseAudio(nativeAudio);
        nativeAudio = 0;
        nextTrackRecreateAt = uptimeMillis() + TRACK_RECREATE_INTERVAL_MS;
        return setupAudioTrack() == 0;
    }

    static void downmixStereo(short[] input, int length, int channels, short[] stereo) {
        for (int frame = 0; frame < length / channels; frame++) {
            for (int side = 0; side < 2; side++) {
                // The decoder supplies FL FR C LFE RL RR [SL SR].
                int base = frame * channels;
                float mix = input[base + side] + 0.7071f * input[base + 2];
                for (int c = 4 + side; c < channels; c += 2) mix += 0.7071f * input[base + c];
                stereo[frame * 2 + side] = (short)Math.max(-32768, Math.min(32767, Math.round(mix)));
            }
        }
    }

    // Package-visible write/recreation/clock hooks keep recovery testable without an audio device.
    int writeAudioTrack(short[] audioData, int offset, int length) {
        AudioTrack current = track;
        // A failed recreation leaves no track. Treat that like a dead one so recovery is retried.
        return current == null ? AudioTrack.ERROR_DEAD_OBJECT : current.write(audioData, offset, length);
    }

    long uptimeMillis() {
        return SystemClock.uptimeMillis();
    }

    boolean recreateAudioTrack() {
        releaseAudioTrack();
        if (setupAudioTrack() != 0) {
            return false;
        }
        // Also after an earlier attempt failed and closed the session with the old track.
        if (audioFxWanted) {
            setAudioFxSession(true);
        }
        return true;
    }

    // Returns false when the rest of this buffer was dropped. The caller keeps playing the next one.
    boolean writeAudioTrackFully(short[] audioData, int length) {
        int offset = 0;
        boolean recreated = false;
        while (offset < length) {
            int written = writeAudioTrack(audioData, offset, length - offset);
            if (written > 0 && written <= length - offset) {
                offset += written;
            }
            else if ((written == AudioTrack.ERROR_DEAD_OBJECT || failedWrites + 1 >= FAILED_WRITES_BEFORE_RECREATE) &&
                    !recreated && uptimeMillis() >= nextTrackRecreateAt) {
                // A route change can invalidate AudioTrack. Retry only the unwritten samples,
                // at most once per buffer and once a second so a broken output cannot spin on this thread.
                recreated = true;
                failedWrites = 0;
                nextTrackRecreateAt = uptimeMillis() + TRACK_RECREATE_INTERVAL_MS;
                LimeLog.warning(written == AudioTrack.ERROR_DEAD_OBJECT ?
                        "AudioTrack disconnected; recreating audio output" :
                        "AudioTrack writes keep failing: " + written + "; recreating audio output");
                if (!recreateAudioTrack()) {
                    LimeLog.warning("Unable to recreate AudioTrack");
                    return false;
                }
            }
            else {
                failedWrites++;
                long now = uptimeMillis();
                if (now >= nextWriteWarningAt) {
                    nextWriteWarningAt = now + TRACK_RECREATE_INTERVAL_MS;
                    LimeLog.warning("AudioTrack write made no progress or failed: " + written + "; dropping buffer");
                }
                return false;
            }
        }
        failedWrites = 0;
        return true;
    }

    private void tuneBuffer() {
        long now = SystemClock.uptimeMillis();
        if (bufferPolicy == null || now - lastBufferCheckMs < 1000) return;
        lastBufferCheckMs = now;
        if (nativeAudio != 0) {
            int[] stats = nativeAudioStats(nativeAudio);
            int target = bufferPolicy.sample(stats[3], now);
            if (target != stats[2]) {
                bufferPolicy.applied(nativeAudioBuffer(nativeAudio, target));
                stats = nativeAudioStats(nativeAudio);
            }
            reportNativeAudio(stats);
        } else if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
            int underruns = track.getUnderrunCount();
            int target = bufferPolicy.sample(underruns, now);
            if (target != track.getBufferSizeInFrames()) bufferPolicy.applied(track.setBufferSizeInFrames(target));
            ProblemReport.setAudioDiagnostics("AudioTrack: rate=" + sampleRate + ", channels=" + channels +
                    ", packetFrames=" + samplesPerFrame + ", bufferFrames=" + track.getBufferSizeInFrames() +
                    ", capacityFrames=" + track.getBufferCapacityInFrames() + ", underruns=" + underruns);
        }
    }

    private void reportNativeAudio(int[] stats) {
        ProblemReport.setAudioDiagnostics("AAudio: rate=" + sampleRate + ", channels=" + channels +
                ", packetFrames=" + samplesPerFrame + ", burstFrames=" + stats[0] +
                ", reserveFrames=" + stats[2] + ", capacityFrames=" + stats[1] +
                ", hardwareFrames=" + stats[4] + ", hardwareUnderruns=" + stats[5] + ", softwareUnderruns=" + stats[6]);
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
        audioFxWanted = true;
        setAudioFxSession(true);
    }

    @Override
    public synchronized void stop() {
        stopped = true;
        audioFxWanted = false;
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
