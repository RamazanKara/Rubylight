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
import android.os.SystemClock;

import androidx.annotation.RequiresApi;

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
    // The AudioTrack buffer, or the AAudio device buffer (hardware xruns; its starvation means fallback).
    private AudioBufferPolicy bufferPolicy;
    // AAudio only: the jitter reserve in the ring ahead of the stream (network starvation, never fallback).
    private AudioBufferPolicy reservePolicy;
    private AudioBufferPolicy.StallWatch stallWatch;
    // What the output holds at most for jitter, in frames: the ring's reserve or the track's buffer.
    private int reserveFrames;
    // Samples the current AudioTrack accepted; its playback head gives what it still holds.
    private long trackSamplesWritten;
    private boolean reserveStarvationShown;
    private int largestReserveFrames;
    // The AAudio underruns the log last showed: each new one is logged, a stall too, which leaves the reserve as it is.
    private int loggedUnderruns;
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
    // Buffers that never played, for the problem report: over the 40 ms queue bound, or refused by the output.
    private long lateDropped, droppedWrites, nextDropReportAt;
    // Cached at setup: the native rate is a cross-process call and the route changes only with the output.
    private int nativeRate;
    private String routes = "unknown";
    private boolean routeResolved;
    private int routeRetries;
    // What the AudioTrack really got, for the problem report: granted mode, output burst, smallest buffer we allow.
    private int trackPerfMode = -1, outputBurst, floorFrames, trackMinBufferFrames;
    // Passthrough: whole AC3 or E-AC3 frames to a compressed track the route decodes (Android 10 and later).
    private AudioTrack encodedTrack;
    private int encodedCodec, encodedBufferBytes;
    // A passthrough output that cannot be rebuilt says so once per stream, not once a second.
    private boolean encodedFailureShown;

    private static native long nativeOpenAudio(int sampleRate, long concealer);
    private static native boolean nativeWriteAudio(long handle, short[] samples, int length);
    private static native int[] nativeAudioStats(long handle);
    private static native int nativeAudioBuffer(long handle, int frames);
    private static native int nativeAudioReserve(long handle, int frames);
    private static native void nativeAudioReserveLimit(long handle, int frames);
    private static native int nativeAudioQueued(long handle);
    private static native boolean nativeAudioSkipLate(long handle, int frames, int pendingFrames);
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
        reserveStarvationShown = false;
        largestReserveFrames = 0;
        loggedUnderruns = 0;
        nativeRate = nativeOutputRate();

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
            case Surround714.CHANNELS:
                // 7.1.4 in the decoder's order FL FR C LFE RL RR SL SR TFL TFR TBL TBR; Android 12L+.
                channelConfig = Surround714.channelMask();
                if (channelConfig == 0) {
                    LimeLog.severe("7.1.4 needs Android 12L or later");
                    return -1;
                }
                break;
            default:
                LimeLog.severe("Decoder returned unhandled channel count");
                return -1;
        }

        LimeLog.info("Audio channel config: "+String.format("0x%X", channelConfig));

        if (AudioBufferPolicy.useAAudio(Build.VERSION.SDK_INT, channels, enableAudioFx, sampleRate, nativeRate)) {
            nativeAudio = nativeOpenAudio(sampleRate, MoonBridge.audioConcealer());
            if (nativeAudio != 0) {
                int[] stats = nativeAudioStats(nativeAudio);
                long now = SystemClock.uptimeMillis();
                bufferPolicy = new AudioBufferPolicy(stats[0], stats[8], stats[4], stats[5], now);
                // Before the first packet: the ring has not primed, so the larger start only delays that.
                int start = nativeAudioReserve(nativeAudio, AudioBufferPolicy.reserveStartFrames(stats[0], sampleRate));
                reservePolicy = AudioBufferPolicy.forReserve(stats[0], stats[1], start,
                        reserveStarvations(stats), now, sampleRate);
                nativeAudioReserveLimit(nativeAudio, reservePolicy.limit());
                stallWatch = new AudioBufferPolicy.StallWatch(stats[STAT_STALLS]);
                stats = nativeAudioStats(nativeAudio);
                setReserve(stats[2]);
                LimeLog.info("Audio output: AAudio low latency, burst " + stats[0] + ", reserve " + stats[2] +
                        " frames (limit " + reservePolicy.limit() + "), device buffer " + stats[4] + " of " + stats[8]);
                refreshRoute(stats[7]);
                reportNativeAudio(stats);
                return 0;
            }
        }
        return buildAudioTrack();
    }

    // For recreation and fallback: the output may have changed since setup(), so read the native rate again.
    private int setupAudioTrack() {
        nativeRate = nativeOutputRate();
        return buildAudioTrack();
    }

    private int buildAudioTrack() {
        bufferPolicy = null;
        reservePolicy = null;
        stallWatch = null;
        reserveFrames = 0;
        trackSamplesWritten = 0;
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
                    bufferSize = Math.max(minBufferBytes(), bytesPerFrame * 2);

                    // Round to next frame
                    bufferSize = (((bufferSize + (bytesPerFrame - 1)) / bytesPerFrame) * bytesPerFrame);
                    break;
                default:
                    // Unreachable
                    throw new IllegalStateException();
            }

            // Skip low latency options if hardware sample rate doesn't match the content
            if (nativeRate != sampleRate && lowLatency) {
                continue;
            }

            // Skip low latency options when using audio effects, since low latency mode
            // precludes the use of the audio effect pipeline (as of Android 13).
            if (enableAudioFx && lowLatency) {
                continue;
            }

            try {
                int minBufferFrames = Math.max(0, minBufferBytes()) / (channels * 2);
                trackPerfMode = -1;
                trackMinBufferFrames = minBufferFrames;
                outputBurst = outputFramesPerBuffer();
                floorFrames = 0;
                // The granted path is unknown until the track exists, so size for the normal mixer.
                int capacityBytes = Build.VERSION.SDK_INT >= Build.VERSION_CODES.N ?
                        Math.max(bufferSize, AudioBufferPolicy.trackCapacityFrames(samplesPerFrame, minBufferFrames,
                                outputBurst) * channels * 2) : bufferSize;
                track = createAudioTrack(channelConfig, sampleRate, capacityBytes, lowLatency);
                setReserve(capacityBytes / (channels * 2));
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
                    // The framework may refuse the fast path. Such a track runs on the normal mixer, whose
                    // period is far above two packets, so the buffer must not go below its own minimum.
                    trackPerfMode = trackPerformanceMode();
                    floorFrames = AudioBufferPolicy.trackFloorFrames(Build.VERSION.SDK_INT, trackPerfMode, lowLatency,
                            samplesPerFrame, minBufferFrames, outputBurst);
                    boolean fast = AudioBufferPolicy.trackIsFast(Build.VERSION.SDK_INT, trackPerfMode, lowLatency);
                    int actual = track.setBufferSizeInFrames(Math.max(bufferSize / (channels * 2), floorFrames));
                    int granted = actual > 0 ? actual : track.getBufferSizeInFrames();
                    bufferPolicy = AudioBufferPolicy.forTrack(
                            AudioBufferPolicy.trackStepFrames(fast, samplesPerFrame, outputBurst),
                            track.getBufferCapacityInFrames(), floorFrames,
                            granted, track.getUnderrunCount(), SystemClock.uptimeMillis(),
                            AudioBufferPolicy.maxReserveFrames(sampleRate));
                    setReserve(granted);
                }
                track.play();
                refreshRoute(-1);

                // Successfully created working AudioTrack. We're done here.
                LimeLog.info("Audio track configuration: "+bufferSize+" "+lowLatency+", buffer "+reserveFrames+
                        " frames (limit "+(bufferPolicy != null ? bufferPolicy.limit() : reserveFrames)+")"+pathDetails());
                ProblemReport.setAudioDiagnostics("AudioTrack: rate=" + sampleRate + ", channels=" + channels +
                        ", packetFrames=" + samplesPerFrame + ", bufferBytes=" + bufferSize + ", lowLatency=" + lowLatency +
                        pathDetails() + outputDetails());
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
            reserveFrames = 0;
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
        // Only queue up to 40 ms of audio beyond the output's reserve. The output's own level counts: after a
        // network stall drained it, the burst that follows refills it instead of being dropped.
        // A stall's packets whose time Opus already concealed: playing them would keep latency up by the stall.
        // The packets waiting behind this one refill the reserve, so only the excess over it is skipped.
        int pendingMs = pendingAudioMs();
        if (nativeAudio != 0 && channels > 0 &&
                skipLateNativeAudio(length / channels, (int)((long)Math.max(0, pendingMs) * sampleRate / 1000))) return;
        int queuedFrames = outputQueuedFrames();
        if (!AudioBufferPolicy.dropLate(pendingMs, queuedFrames, reserveFrames, sampleRate)) {
            if (nativeAudio != 0 && bufferPolicy.needsFallback() &&
                    !fallbackToAudioTrack("AAudio hardware keeps underrunning at its largest device buffer")) {
                droppedWrites++;
                return;
            }
            if (nativeAudio != 0) {
                if (writeNativeAudio(audioData, length)) {
                    echoReference(audioData, length, queuedFrames);
                    tuneBuffer();
                    return;
                }
                if (!fallbackToAudioTrack("AAudio disconnected")) {
                    // Later buffers retry the AudioTrack like any other dead output.
                    droppedWrites++;
                    return;
                }
            }
            // This will block until the write is completed. That can cause a backlog
            // of pending audio data, so we do the above check to be able to bound
            // latency at 40 ms in that situation.
            // A failed write drops only this buffer; the session's audio keeps playing.
            if (writeAudioTrackFully(audioData, length)) {
                echoReference(audioData, length, queuedFrames);
                tuneBuffer();
            } else {
                droppedWrites++;
                reportDrops();
            }
        }
        else {
            lateDropped++;
            LimeLog.info("Too much pending audio data: " + pendingMs + " ms, output " +
                    framesToMs(queuedFrames) + " of " + framesToMs(reserveFrames) + " ms");
            reportDrops();
        }
    }

    // What the speaker is about to play, for the microphone's echo canceller (a volatile read while none runs).
    private void echoReference(short[] audioData, int length, int queuedFrames) {
        if (sampleRate == EchoCanceller.RATE && channels > 0) {
            EchoCanceller.reference(audioData, length, channels, framesToMs(queuedFrames));
        }
    }

    private int framesToMs(int frames) {
        return sampleRate > 0 ? (int)(frames * 1000L / sampleRate) : 0;
    }

    // One line per stream for comparing outputs: what was dropped, what starved, how large the reserve got.
    private void logSummary() {
        String output;
        if (nativeAudio != 0) {
            int[] stats = nativeAudioStats(nativeAudio);
            output = "AAudio, underruns " + stats[6] + " (stalls " + stats[STAT_STALLS] + "), xruns " + stats[5] +
                    ", device buffer " + stats[4] +
                    ", concealed " + framesToMs(stats[9]) + " ms, skipped late " + framesToMs(stats[10]) + " ms";
        } else if (track != null && Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
            output = "AudioTrack, underruns " + track.getUnderrunCount();
        } else if (track != null) {
            output = "AudioTrack";
        } else {
            return;
        }
        LimeLog.info("Audio summary: " + output + ", reserve " + framesToMs(reserveFrames) + " ms (largest " +
                framesToMs(largestReserveFrames) + " ms), " + AudioDiagnostics.drops(lateDropped, droppedWrites));
    }

    private void setReserve(int frames) {
        reserveFrames = Math.max(0, frames);
        largestReserveFrames = Math.max(largestReserveFrames, reserveFrames);
    }

    /**
     * Whether this packet's time already went to Opus concealment during a stall and the pendingFrames
     * waiting behind it still fill the reserve without it (aaudio_output.c).
     */
    boolean skipLateNativeAudio(int frames, int pendingFrames) {
        return nativeAudioSkipLate(nativeAudio, frames, pendingFrames);
    }

    /** Frames the output holds that are not played yet: the AAudio ring, or the track past its playback head. */
    int outputQueuedFrames() {
        if (nativeAudio != 0) return nativeAudioQueued(nativeAudio);
        AudioTrack current = track;
        if (current == null || channels <= 0) return 0;
        try {
            // Both counters wrap at 32 bits alike; the difference stays right.
            return Math.max(0, (int)(trackSamplesWritten / channels) - current.getPlaybackHeadPosition());
        } catch (RuntimeException e) {
            return 0;
        }
    }

    // The periodic report in tuneBuffer() only runs after a played buffer, so drops would not show.
    private void reportDrops() {
        long now = uptimeMillis();
        if (now < nextDropReportAt) return;
        nextDropReportAt = now + 1000;
        if (nativeAudio != 0) {
            reportNativeAudio(nativeAudioStats(nativeAudio));
        } else if (track != null && Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
            reportAudioTrack(track.getUnderrunCount());
        }
    }

    /** AudioTrack.getPerformanceMode(), or -1 where the platform cannot say (before Android 8.0). */
    int trackPerformanceMode() {
        AudioTrack current = track;
        if (current == null || Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return -1;
        try {
            return current.getPerformanceMode();
        } catch (RuntimeException e) {
            return -1;
        }
    }

    /** The output's native burst in frames, 0 when the property is missing or invalid. */
    int outputFramesPerBuffer() {
        if (context == null) return 0;
        try {
            AudioManager manager = (AudioManager)context.getSystemService(Context.AUDIO_SERVICE);
            return AudioBufferPolicy.parseFrames(manager.getProperty(AudioManager.PROPERTY_OUTPUT_FRAMES_PER_BUFFER));
        } catch (RuntimeException e) {
            return 0;
        }
    }

    private String pathDetails() {
        return ", perfMode=" + AudioBufferPolicy.perfModeName(trackPerfMode) + ", outputBurst=" + outputBurst +
                ", minBufferFrames=" + trackMinBufferFrames + ", floorFrames=" + floorFrames;
    }

    /** The framework's minimum buffer for this track's format; a surround track's is far above a stereo one's. */
    int minBufferBytes() {
        return AudioTrack.getMinBufferSize(sampleRate, channelConfig, AudioFormat.ENCODING_PCM_16BIT);
    }

    int nativeOutputRate() {
        return AudioTrack.getNativeOutputSampleRate(AudioManager.STREAM_MUSIC);
    }

    /** Device type the AudioTrack is routed to, or -1 while it is not routed yet. */
    int routedTrackType() {
        AudioTrack current = track;
        if (current == null || Build.VERSION.SDK_INT < Build.VERSION_CODES.N) return -1;
        try {
            AudioDeviceInfo device = current.getRoutedDevice();
            return device == null ? -1 : device.getType();
        } catch (RuntimeException e) {
            return -1;
        }
    }

    /** {id, type} of every attached output, the fallback when the routed device is unknown. */
    int[][] attachedOutputs() {
        if (context == null || Build.VERSION.SDK_INT < Build.VERSION_CODES.M) return new int[0][];
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

    // aaudioDeviceId is the AAudio stream's device, 0 or less when unknown. The AudioTrack path asks the track.
    void refreshRoute(int aaudioDeviceId) {
        int[][] attached = attachedOutputs();
        int routed = -1;
        if (nativeAudio != 0) {
            if (aaudioDeviceId > 0) {
                for (int[] device : attached) if (device[0] == aaudioDeviceId) routed = device[1];
            }
        } else {
            routed = routedTrackType();
        }
        int[] types = new int[attached.length];
        for (int i = 0; i < attached.length; i++) types[i] = attached[i][1];
        routes = AudioDiagnostics.route(routed, types);
        routeResolved = routed >= 0;
        routeRetries = 0;
    }

    // A track is often not routed yet right after play(); look again in a few reports before giving up.
    private void resolveRoute(int aaudioDeviceId) {
        if (!routeResolved && routeRetries < 5) {
            int retries = routeRetries + 1;
            refreshRoute(aaudioDeviceId);
            routeRetries = retries;
        }
    }

    String outputDetails() {
        return ", nativeRate=" + nativeRate + ", route=" + routes + ", " + AudioDiagnostics.drops(lateDropped, droppedWrites);
    }

    boolean fallbackToAudioTrack(String reason) {
        LimeLog.warning(reason + "; falling back to AudioTrack");
        nativeCloseAudio(nativeAudio);
        nativeAudio = 0;
        nextTrackRecreateAt = uptimeMillis() + TRACK_RECREATE_INTERVAL_MS;
        return setupAudioTrack() == 0;
    }

    // nativeAudioStats: gaps the reserve could still grow to cover (aaudio_output.c), and longer ones.
    static final int STAT_JITTER_GAPS = 11, STAT_STALLS = 12;

    /**
     * What starves the AAudio reserve for its growth: the jitter gaps only. A stall it could not grow
     * to cover is concealed; counting every underrun (stats[6]) took the Retroid's reserve from 14 to
     * 32 ms on 37 to 56 ms Wi-Fi stalls without covering the next.
     */
    static int reserveStarvations(int[] stats) {
        return stats[STAT_JITTER_GAPS];
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

    boolean writeNativeAudio(short[] audioData, int length) {
        return nativeWriteAudio(nativeAudio, audioData, length);
    }

    int pendingAudioMs() {
        return MoonBridge.getPendingAudioDuration();
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
                trackSamplesWritten += written;
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
            // Starvation of the ring is the network's or the decoder's: only the reserve grows. Device
            // xruns grow only the device buffer, the callback keeps that one full of whatever it has. Only
            // jitter grows the reserve (reserveStarvations).
            int reserve = reservePolicy.sample(reserveStarvations(stats), now);
            int device = bufferPolicy.sample(stats[5], now);
            boolean resized = reserve != stats[2] || device != stats[4];
            if (resized) {
                if (reserve != stats[2]) {
                    int applied = nativeAudioReserve(nativeAudio, reserve);
                    reservePolicy.applied(applied);
                    setReserve(applied);
                }
                if (device != stats[4]) bufferPolicy.applied(nativeAudioBuffer(nativeAudio, device));
                stats = nativeAudioStats(nativeAudio);
            }
            if (resized || stats[6] != loggedUnderruns) {
                loggedUnderruns = stats[6];
                LimeLog.info("AAudio reserve " + stats[2] + " frames, device buffer " + stats[4] + ", underruns " +
                        stats[6] + ", stalls " + stats[STAT_STALLS] + ", xruns " + stats[5]);
            }
            if (stallWatch.repeating(stats[STAT_STALLS], now) && !reserveStarvationShown) {
                // Loud, but no switch: AudioTrack would starve on the same late audio.
                reserveStarvationShown = true;
                LimeLog.warning("AAudio reserve starves at its " + framesToMs(reservePolicy.limit()) +
                        " ms limit: audio arrives too late from the network (" +
                        AudioBufferPolicy.StallWatch.STALL_WARN_COUNT + " stalls within a minute), its gaps are concealed");
            }
            reportNativeAudio(stats);
        } else if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
            int underruns = track.getUnderrunCount();
            int target = bufferPolicy.sample(underruns, now);
            if (target != track.getBufferSizeInFrames()) {
                int applied = track.setBufferSizeInFrames(target);
                bufferPolicy.applied(applied);
                setReserve(applied > 0 ? applied : track.getBufferSizeInFrames());
                LimeLog.info("AudioTrack buffer " + reserveFrames + " frames, underruns " + underruns);
            }
            reportAudioTrack(underruns);
        }
    }

    @RequiresApi(Build.VERSION_CODES.N)
    private void reportAudioTrack(int underruns) {
        resolveRoute(-1);
        ProblemReport.setAudioDiagnostics("AudioTrack: rate=" + sampleRate + ", channels=" + channels +
                ", packetFrames=" + samplesPerFrame + ", bufferFrames=" + track.getBufferSizeInFrames() +
                ", capacityFrames=" + track.getBufferCapacityInFrames() + ", underruns=" + underruns +
                pathDetails() + outputDetails());
    }

    private void reportNativeAudio(int[] stats) {
        resolveRoute(stats[7]);
        ProblemReport.setAudioDiagnostics("AAudio: rate=" + sampleRate + ", channels=" + channels +
                ", packetFrames=" + samplesPerFrame + ", burstFrames=" + stats[0] +
                ", reserveFrames=" + stats[2] + ", capacityFrames=" + stats[1] +
                ", hardwareFrames=" + stats[4] + ", hardwareUnderruns=" + stats[5] + ", softwareUnderruns=" + stats[6] +
                outputDetails());
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
    public synchronized int setupEncoded(MoonBridge.AudioConfiguration audioConfiguration, int codec) {
        stopped = false;
        encodedFailureShown = false;
        channels = audioConfiguration.channelCount;
        sampleRate = AudioPassthrough.SAMPLE_RATE;
        encodedCodec = codec;
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) {
            return encodedFailed("needs Android 10 or later");
        }
        if (AudioPassthrough.encoding(codec) == 0 || AudioPassthrough.channelMask(channels) == 0) {
            return encodedFailed(AudioPassthrough.names(codec) + " with " + channels + " channels is not a passthrough format");
        }
        return buildEncodedTrack();
    }

    private int buildEncodedTrack() {
        // setupEncoded checked already; this keeps the route-change rebuild on Android 10 too.
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) {
            return encodedFailed("needs Android 10 or later");
        }
        int encoding = AudioPassthrough.encoding(encodedCodec);
        int mask = AudioPassthrough.channelMask(channels);
        try {
            encodedBufferBytes = Math.max(AudioTrack.getMinBufferSize(sampleRate, mask, encoding),
                    2 * AudioPassthrough.MAX_FRAME_BYTES);
            encodedTrack = createEncodedTrack(encoding, mask, encodedBufferBytes);
        } catch (RuntimeException e) {
            encodedTrack = null;
            return encodedFailed("the audio output refused " + AudioPassthrough.names(encodedCodec) + ": " + e.getMessage());
        }
        LimeLog.info("Audio output: " + AudioPassthrough.names(encodedCodec) + " passthrough, " + channels + " channels");
        reportEncoded();
        return 0;
    }

    // Loud: the log, the problem report and the stream's message line. Returns setup's error.
    private int encodedFailed(String reason) {
        LimeLog.severe("Audio passthrough: " + reason);
        ProblemReport.setAudioDiagnostics("AudioTrack passthrough failed: codec=" + AudioPassthrough.names(encodedCodec) +
                ", channels=" + channels + ", " + reason);
        if (!encodedFailureShown) {
            encodedFailureShown = true;
            MoonBridge.bridgeClDisplayMessage("Audio passthrough failed: " + reason + ". Turn off Audio passthrough to hear Opus.");
        }
        return -1;
    }

    private void reportEncoded() {
        AudioTrack current = encodedTrack;
        int routed = -1;
        if (current != null && Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
            try {
                AudioDeviceInfo device = current.getRoutedDevice();
                if (device != null) routed = device.getType();
            } catch (RuntimeException ignored) {}
        }
        ProblemReport.setAudioDiagnostics("AudioTrack passthrough: codec=" + AudioPassthrough.names(encodedCodec) +
                ", channels=" + channels + ", bufferBytes=" + encodedBufferBytes +
                ", route=" + AudioDiagnostics.route(routed, new int[0]) + ", " + AudioDiagnostics.drops(lateDropped, droppedWrites));
    }

    @RequiresApi(Build.VERSION_CODES.Q)
    AudioTrack createEncodedTrack(int encoding, int channelMask, int bufferBytes) {
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
    @Override
    public synchronized void playEncodedAudio(byte[] frame, int length) {
        if (stopped) return;
        // The same 40 ms bound on pending audio as decoded packets.
        if (pendingAudioMs() >= 40) {
            lateDropped++;
            return;
        }
        int written = writeEncoded(frame, length);
        if (written == length) return;
        droppedWrites++;
        long now = uptimeMillis();
        if (written == AudioTrack.ERROR_DEAD_OBJECT && now >= nextTrackRecreateAt) {
            // A route change ends a compressed track; the new route may not take the format.
            nextTrackRecreateAt = now + TRACK_RECREATE_INTERVAL_MS;
            LimeLog.warning("Passthrough AudioTrack disconnected; recreating it");
            releaseEncodedTrack();
            buildEncodedTrack();
        } else if (now >= nextWriteWarningAt) {
            nextWriteWarningAt = now + TRACK_RECREATE_INTERVAL_MS;
            LimeLog.warning("Passthrough AudioTrack write failed: " + written + " of " + length + " bytes; dropping the frame");
            reportEncoded();
        }
    }

    // Blocking and whole: a compressed track takes only complete frames.
    int writeEncoded(byte[] frame, int length) {
        AudioTrack current = encodedTrack;
        if (current == null || Build.VERSION.SDK_INT < Build.VERSION_CODES.M) return AudioTrack.ERROR_DEAD_OBJECT;
        return current.write(frame, 0, length, AudioTrack.WRITE_BLOCKING);
    }

    private void releaseEncodedTrack() {
        AudioTrack oldTrack = encodedTrack;
        encodedTrack = null;
        if (oldTrack == null) return;
        try {
            oldTrack.pause();
            oldTrack.flush();
        } catch (IllegalStateException e) {
            LimeLog.warning("Passthrough AudioTrack was already stopped during release");
        } finally {
            oldTrack.release();
        }
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
        logSummary();
        if (nativeAudio != 0) {
            nativeCloseAudio(nativeAudio);
            nativeAudio = 0;
        }
        reservePolicy = null;
        stallWatch = null;
        reserveFrames = 0;
        releaseAudioTrack();
        releaseEncodedTrack();
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
