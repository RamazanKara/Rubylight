package com.limelight.binding.audio;

import android.content.Context;
import android.media.AudioFormat;
import android.media.AudioManager;
import android.media.AudioTrack;
import android.os.Build;
import android.os.SystemClock;

import androidx.annotation.RequiresApi;

import com.limelight.LimeLog;
import com.limelight.nvstream.av.audio.AudioRenderer;
import com.limelight.nvstream.jni.MoonBridge;
import com.limelight.utils.ProblemReport;

/**
 * Android audio renderer supporting low-latency native AAudio, PCM {@link AudioTrack} fallback,
 * controller headphone routing, and AC3/E-AC3 passthrough.
 */
public class AndroidAudioRenderer implements AudioRenderer {

    private final Context context;
    private final boolean enableAudioFx;
    public interface HeadphoneSink {
        boolean write(short[] pcm, int length, float volume);
        // Lets the audio thread skip AudioService volume reads while no headset route exists.
        default boolean active() { return true; }
    }
    private final HeadphoneSink headphoneSink;
    private final AudioTrackBackend audioTrack;
    private final AudioEncodedOutput encodedOutput;
    private long volumeReadAt;
    private float streamVolume = 1;
    private short[] stereo;

    private volatile long nativeAudio;
    // Guards the native output's lifetime against abortNativeAudio, which stop() and cleanup() call
    // without the renderer monitor while a write that holds it waits on a stalled output. Never held
    // across a write.
    private final Object nativeLock = new Object();
    // stop() or cleanup() was called: a write the abort cut short is no disconnect to reopen from.
    private volatile boolean stopRequested;
    // The AudioTrack buffer, or the AAudio device buffer (hardware xruns; its starvation means fallback).
    AudioBufferPolicy bufferPolicy;
    // AAudio only: the jitter reserve in the ring ahead of the stream (network starvation, never fallback).
    AudioBufferPolicy reservePolicy;
    AudioBufferPolicy.StallWatch stallWatch;
    // What the output holds at most for jitter, in frames: the ring's reserve or the track's buffer.
    int reserveFrames;
    private boolean reserveStarvationShown;
    private int largestReserveFrames;
    // The AAudio underruns the log last showed: each new one is logged, a stall too, which leaves the reserve as it is.
    private int loggedUnderruns;
    long lastBufferCheckMs;
    int channelConfig, sampleRate, samplesPerFrame, channels;
    private boolean stopped;
    // Buffers that never played, for the problem report: over the 40 ms queue bound, or refused by the output.
    long lateDropped, droppedWrites, nextDropReportAt;
    // Cached at setup: the native rate is a cross-process call and the route changes only with the output.
    int nativeRate;
    private String routes = "unknown";
    private boolean routeResolved;
    private int routeRetries;
    // Clock drift: packets trimmed, and the last one's start, which the next written packet fades in from.
    private final AudioBufferPolicy.DriftTrim driftTrim = new AudioBufferPolicy.DriftTrim();
    private long driftTrimmed;
    // A stall or a drifting clock drops or trims packet after packet; each kind logs one line a second.
    private final AudioDiagnostics.Paced trimLog = new AudioDiagnostics.Paced();
    private final AudioDiagnostics.Paced lateLog = new AudioDiagnostics.Paced();
    private short[] trimHead = new short[0];
    private int trimHeadSamples;
    // AAudio reopens after a disconnect: when the last one was, and how many came right after the one before.
    private long lastReopenAt = Long.MIN_VALUE / 2;
    private int quickReopens;
    // Native writes in a row that stalled. A callback that stops for good without an error callback
    // (AAudio still reporting the stream started) stalls every write: after MAX_NATIVE_STALLS
    // (about 300 ms of buffers dropped) the stream is taken for stopped and reopened.
    private int nativeStalls;
    static final int MAX_NATIVE_STALLS = 3;

    private static native long nativeOpenAudio(int sampleRate, long concealer);
    /**
     * nativeWriteAudio's answers: the buffer is in the ring; the AAudio callback took nothing for
     * 100 ms while the stream still runs (a short stall: the rest of the buffer is dropped, the
     * stream and its tuned reserve stay); the stream stopped or disconnected and must be reopened.
     */
    static final int NATIVE_WRITE_DONE = 0, NATIVE_WRITE_STALLED = 1, NATIVE_WRITE_STOPPED = 2;

    private static native int nativeWriteAudio(long handle, short[] samples, int length);
    private static native int nativeWriteAudioDirect(long handle, long pcmPtr, int frames);
    private static native void nativeCopyShorts(long pcmPtr, short[] dst, int count);
    private static native int[] nativeAudioStats(long handle);
    private static native int nativeAudioBuffer(long handle, int frames);
    private static native int nativeAudioReserve(long handle, int frames);
    private static native void nativeAudioReserveLimit(long handle, int frames);
    private static native int nativeAudioQueued(long handle);
    private static native boolean nativeAudioSkipLate(long handle, int frames, int pendingFrames);
    private static native int nativeAudioCheckAndQueued(long handle, int frames, int pendingFrames);
    private static native void nativeCloseAudio(long handle);
    private static native void nativeAbortAudio(long handle);

    public AndroidAudioRenderer(Context context, boolean enableAudioFx) {
        this(context, enableAudioFx, null);
    }

    public AndroidAudioRenderer(Context context, boolean enableAudioFx, HeadphoneSink headphoneSink) {
        this.context = context;
        this.enableAudioFx = enableAudioFx;
        this.headphoneSink = headphoneSink;
        this.audioTrack = new AudioTrackBackend(this, context, enableAudioFx);
        this.encodedOutput = new AudioEncodedOutput(this);
    }

    AudioTrack createAudioTrack(int channelConfig, int sampleRate, int bufferSize, boolean lowLatency) {
        return AudioOutputSupport.createPcmTrack(channelConfig, sampleRate, bufferSize, lowLatency);
    }

    @Override
    public synchronized int setup(MoonBridge.AudioConfiguration audioConfiguration, int sampleRate, int samplesPerFrame) {
        this.sampleRate = sampleRate;
        this.samplesPerFrame = samplesPerFrame;
        this.channels = audioConfiguration.channelCount;
        stopped = false;
        stopRequested = false;
        lastBufferCheckMs = 0;
        reserveStarvationShown = false;
        largestReserveFrames = 0;
        loggedUnderruns = 0;
        nativeRate = nativeOutputRate();

        channelConfig = AudioOutputSupport.channelConfig(audioConfiguration.channelCount);
        if (channelConfig == 0) return -1;

        LimeLog.info("Audio channel config: " + String.format("0x%X", channelConfig));

        driftTrim.reset();
        trimHeadSamples = 0;
        quickReopens = 0;
        lastReopenAt = Long.MIN_VALUE / 2;
        if (openAAudio()) return 0;
        return audioTrack.build();
    }

    // AAudio at the stream's rate where useAAudio allows it, with fresh buffer policies; false leaves none open.
    private boolean openAAudio() {
        if (!AudioBufferPolicy.useAAudio(Build.VERSION.SDK_INT, channels, enableAudioFx, sampleRate, nativeRate)) {
            return false;
        }
        long opened = nativeOpenAudio(sampleRate, MoonBridge.audioConcealer());
        synchronized (nativeLock) {
            nativeAudio = opened;
        }
        if (nativeAudio == 0) return false;
        int[] stats = nativeAudioStats(nativeAudio);
        long now = SystemClock.uptimeMillis();
        lastBufferCheckMs = 0;
        loggedUnderruns = 0;
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
        return true;
    }

    /**
     * AAudio again on the route the output moved to, re-reading its native rate. Package-visible so the
     * reopen decision is testable without an audio device.
     */
    boolean openNativeAudio() {
        nativeRate = nativeOutputRate();
        return openAAudio();
    }

    void closeNativeAudio() {
        synchronized (nativeLock) {
            if (nativeAudio != 0) nativeCloseAudio(nativeAudio);
            nativeAudio = 0;
        }
    }

    /**
     * Cuts a native write short that waits for room in a stalled output, so stop() and cleanup() do not
     * queue behind its retries on the monitor. The output stays open until cleanup() or a reopen closes it.
     */
    void abortNativeAudio() {
        synchronized (nativeLock) {
            if (nativeAudio != 0) nativeAbortAudio(nativeAudio);
        }
    }

    /**
     * AAudio disconnected, as a route change (headphones, Bluetooth, a USB DAC) does: AAudio again on the
     * new route, keeping low latency, stall concealment and late skips. AudioTrack only when it does not
     * reopen, or keeps dying right after reopening (AudioBufferPolicy.reopenAAudio).
     */
    boolean reopenAudio(String reason) {
        long now = uptimeMillis();
        quickReopens = now - lastReopenAt < AudioBufferPolicy.REOPEN_SETTLE_MS ? quickReopens + 1 : 0;
        lastReopenAt = now;
        if (!AudioBufferPolicy.reopenAAudio(quickReopens)) {
            return fallbackToAudioTrack(reason + " " + quickReopens + " times right after reopening");
        }
        LimeLog.warning(reason + "; reopening AAudio on the current route");
        closeNativeAudio();
        driftTrim.reset();
        if (openNativeAudio()) return true;
        return fallbackToAudioTrack(reason + " and AAudio did not reopen");
    }

    // For recreation and fallback: the output may have changed since setup(), so read the native rate again.
    int setupAudioTrack() {
        nativeRate = nativeOutputRate();
        return audioTrack.build();
    }

    @Override
    public void playDecodedAudio(short[] audioData, int length) {
        playDecodedAudio(audioData, length, -1);
    }

    /** Plays decoded PCM with the Rust audio queue duration already supplied (-1 reads via JNI). */
    public void playDecodedAudio(short[] audioData, int length, int pendingMs) {
        // AudioService calls run only for a live headset route, and never under the renderer
        // lock that stop() and cleanup() take from other threads.
        boolean headphones = headphoneSink != null && headphoneSink.active();
        if (headphones && uptimeMillis() - volumeReadAt >= 500) {
            streamVolume = AudioOutputSupport.readMusicVolume(context);
            volumeReadAt = uptimeMillis();
        }
        playDecodedAudio(audioData, length, headphones, pendingMs);
    }

    /**
     * Direct native-memory fast path for stereo AAudio without headphones, AEC, or pending crossfade.
     * Returns 1 when handled, 0 for transient fallback to short[], or -1 when AAudio is not active.
     */
    public int playDecodedAudioDirect(long pcmPtr, int length, int pendingMs) {
        if (nativeAudio == 0 || channels != 2) return -1;
        if ((headphoneSink != null && headphoneSink.active()) || trimHeadSamples > 0) return 0;
        return playDecodedAudioDirectLocked(pcmPtr, length, pendingMs);
    }

    private synchronized int playDecodedAudioDirectLocked(long pcmPtr, int length, int pendingMs) {
        if (stopped) return 1;
        if (nativeAudio == 0 || channels != 2) return -1;
        if (trimHeadSamples > 0) return 0;
        int frames = length / 2;
        int pending = pendingMs >= 0 ? pendingMs : pendingAudioMs();
        int pendingFrames = (int)((long)Math.max(0, pending) * sampleRate / 1000);
        int queuedFrames = checkLateAndQueuedNativeAudio(frames, pendingFrames);
        if (queuedFrames < 0) return 1;
        if (AudioBufferPolicy.dropLate(pending, queuedFrames, reserveFrames, sampleRate)) {
            onLateDropped(pending, queuedFrames);
            return 1;
        }
        if (driftTrim.trim(pendingFrames, queuedFrames, reserveFrames, frames, sampleRate)) {
            keepTrimHeadDirect(pcmPtr, length);
            onDriftTrimmed(pending, queuedFrames);
            return 1;
        }
        if (bufferPolicy != null && bufferPolicy.needsFallback()) {
            if (!fallbackToAudioTrack("AAudio hardware keeps underrunning at its largest device buffer")) {
                droppedWrites++;
                return 1;
            }
            return -1;
        }
        int written = writeNativeAudioDirect(pcmPtr, frames);
        return finishNativeWrite(written, null, length, pcmPtr, frames, queuedFrames);
    }

    // Completes a native AAudio write (direct when audioData == null); returns 1 if handled, -1 if fallen back to AudioTrack.
    private int finishNativeWrite(int written, short[] audioData, int length, long pcmPtr, int frames, int queuedFrames) {
        if (written == NATIVE_WRITE_DONE) {
            nativeStalls = 0;
            if (audioData != null) echoReference(audioData, length, queuedFrames);
            tuneBuffer();
            return 1;
        }
        if (stopRequested || (written == NATIVE_WRITE_STALLED && ++nativeStalls < MAX_NATIVE_STALLS)) {
            droppedWrites++;
            return 1;
        }
        boolean stalled = written == NATIVE_WRITE_STALLED;
        nativeStalls = 0;
        if (!reopenAudio(stalled ? "AAudio stopped taking audio" : "AAudio disconnected")) {
            droppedWrites++;
            return 1;
        }
        if (nativeAudio == 0) return -1;
        int retry = audioData != null ? writeNativeAudio(audioData, length) : writeNativeAudioDirect(pcmPtr, frames);
        if (retry == NATIVE_WRITE_DONE) {
            if (audioData != null) echoReference(audioData, length, outputQueuedFrames());
            tuneBuffer();
        } else if (!stopRequested) {
            droppedWrites++;
        }
        return 1;
    }

    private synchronized void playDecodedAudio(short[] audioData, int length, boolean headphones, int pendingMsIn) {
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
            if (headphoneSink.write(sound, count, streamVolume)) {
                // The output this would fade into is not playing.
                trimHeadSamples = 0;
                return;
            }
        }
        // Only queue up to 40 ms of audio beyond the output's reserve. The output's own level counts: after a
        // network stall drained it, the burst that follows refills it instead of being dropped.
        int pendingMs = pendingMsIn >= 0 ? pendingMsIn : pendingAudioMs();
        int pendingFrames = (int)((long)Math.max(0, pendingMs) * sampleRate / 1000);
        int queuedFrames;
        if (nativeAudio != 0 && channels > 0) {
            queuedFrames = checkLateAndQueuedNativeAudio(length / channels, pendingFrames);
            if (queuedFrames < 0) return;
        } else {
            queuedFrames = outputQueuedFrames();
        }
        if (!AudioBufferPolicy.dropLate(pendingMs, queuedFrames, reserveFrames, sampleRate)) {
            if (channels > 0 && driftTrim.trim(pendingFrames, queuedFrames,
                    reserveFrames, length / channels, sampleRate)) {
                keepTrimHead(audioData, length);
                onDriftTrimmed(pendingMs, queuedFrames);
                return;
            }
            if (trimHeadSamples > 0) {
                crossfade(trimHead, trimHeadSamples, audioData, length, channels);
                trimHeadSamples = 0;
            }
            if (nativeAudio != 0 && bufferPolicy.needsFallback() &&
                    !fallbackToAudioTrack("AAudio hardware keeps underrunning at its largest device buffer")) {
                droppedWrites++;
                return;
            }
            if (nativeAudio != 0) {
                int written = writeNativeAudio(audioData, length);
                if (finishNativeWrite(written, audioData, length, 0, 0, queuedFrames) >= 0) return;
            }
            // Blocking write; a failed write drops only this buffer so the session's audio keeps playing.
            if (writeAudioTrackFully(audioData, length)) {
                echoReference(audioData, length, queuedFrames);
                tuneBuffer();
            } else {
                droppedWrites++;
                reportDrops();
            }
        } else {
            onLateDropped(pendingMs, queuedFrames);
        }
    }

    private void onDriftTrimmed(int pendingMs, int queuedFrames) {
        driftTrimmed++;
        int trims = trimLog.count(uptimeMillis());
        if (trims > 0) {
            LimeLog.info("Audio clock drift: trimmed a packet, pending " + pendingMs + " ms, output " +
                    framesToMs(queuedFrames) + " of " + framesToMs(reserveFrames) + " ms (" + driftTrimmed +
                    " trimmed, " + trims + " since the last line)");
        }
    }

    private void onLateDropped(int pendingMs, int queuedFrames) {
        lateDropped++;
        int drops = lateLog.count(uptimeMillis());
        if (drops > 0) {
            LimeLog.info("Too much pending audio data: " + pendingMs + " ms, output " +
                    framesToMs(queuedFrames) + " of " + framesToMs(reserveFrames) + " ms (" + drops +
                    " dropped since the last line)");
        }
        reportDrops();
    }

    // The trimmed packet's first 5 ms: it continues what already played, so the next packet fades in from it.
    private void keepTrimHead(short[] audioData, int length) {
        int samples = Math.min(length, sampleRate / 200 * channels);
        if (trimHead.length < samples) trimHead = new short[samples];
        System.arraycopy(audioData, 0, trimHead, 0, samples);
        trimHeadSamples = samples;
    }

    private void keepTrimHeadDirect(long pcmPtr, int length) {
        int samples = Math.min(length, sampleRate / 200 * channels);
        if (samples <= 0) return;
        if (trimHead.length < samples) trimHead = new short[samples];
        copyNativeShorts(pcmPtr, trimHead, samples);
        trimHeadSamples = samples;
    }

    /** Fades pcm in from head over the samples both have, whole frames, as the AAudio stall fade-in does. */
    static void crossfade(short[] head, int headSamples, short[] pcm, int length, int channels) {
        AudioOutputSupport.crossfade(head, headSamples, pcm, length, channels);
    }

    // What the speaker is about to play, for the microphone's echo canceller (a volatile read while none runs).
    private void echoReference(short[] audioData, int length, int queuedFrames) {
        if (sampleRate == EchoCanceller.RATE && channels > 0) {
            EchoCanceller.reference(audioData, length, channels, framesToMs(queuedFrames));
        }
    }

    private int framesToMs(int frames) {
        return AudioOutputSupport.framesToMs(frames, sampleRate);
    }

    // One line per stream for comparing outputs: what was dropped, what starved, how large the reserve got.
    private void logSummary() {
        String output;
        if (nativeAudio != 0) {
            int[] stats = nativeAudioStats(nativeAudio);
            output = "AAudio, underruns " + stats[6] + " (stalls " + stats[STAT_STALLS] + "), xruns " + stats[5] +
                    ", device buffer " + stats[4] +
                    ", concealed " + framesToMs(stats[9]) + " ms, skipped late " + framesToMs(stats[10]) + " ms";
        } else {
            output = audioTrack.summary();
            if (output == null) return;
        }
        LimeLog.info("Audio summary: " + output + ", reserve " + framesToMs(reserveFrames) + " ms (largest " +
                framesToMs(largestReserveFrames) + " ms), " + AudioDiagnostics.drops(lateDropped, droppedWrites) +
                ", drift trims " + driftTrimmed);
    }

    void setReserve(int frames) {
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

    int checkLateAndQueuedNativeAudio(int frames, int pendingFrames) {
        return nativeAudioCheckAndQueued(nativeAudio, frames, pendingFrames);
    }

    /** Frames the output holds that are not played yet: the AAudio ring, or the track past its playback head. */
    int outputQueuedFrames() {
        if (nativeAudio != 0) return nativeAudioQueued(nativeAudio);
        return audioTrack.queuedFrames(channels);
    }

    // The periodic report in tuneBuffer() only runs after a played buffer, so drops would not show.
    private void reportDrops() {
        long now = uptimeMillis();
        if (now < nextDropReportAt) return;
        nextDropReportAt = now + 1000;
        if (nativeAudio != 0) {
            reportNativeAudio(nativeAudioStats(nativeAudio));
        } else {
            audioTrack.reportCurrent();
        }
    }

    /** AudioTrack.getPerformanceMode(), or -1 where the platform cannot say (before Android 8.0). */
    int trackPerformanceMode() {
        return AudioOutputSupport.trackPerformanceMode(audioTrack.current());
    }

    /** The output's native burst in frames, 0 when the property is missing or invalid. */
    int outputFramesPerBuffer() {
        return AudioOutputSupport.outputFramesPerBuffer(context);
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
        return AudioOutputSupport.routedTrackType(audioTrack.current());
    }

    /** {id, type} of every attached output, the fallback when the routed device is unknown. */
    int[][] attachedOutputs() {
        return AudioOutputSupport.attachedOutputs(context);
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
    void resolveRoute(int aaudioDeviceId) {
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
        LimeLog.severe(reason + "; falling back to AudioTrack");
        closeNativeAudio();
        driftTrim.reset();
        audioTrack.delayNextRecreate(uptimeMillis());
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
        AudioOutputSupport.downmixStereo(input, length, channels, stereo);
    }

    // Package-visible write/recreation/clock hooks keep recovery testable without an audio device.
    int writeAudioTrack(short[] audioData, int offset, int length) {
        return audioTrack.write(audioData, offset, length);
    }

    int writeNativeAudio(short[] audioData, int length) {
        return nativeWriteAudio(nativeAudio, audioData, length);
    }

    int writeNativeAudioDirect(long pcmPtr, int frames) {
        return nativeWriteAudioDirect(nativeAudio, pcmPtr, frames);
    }

    void copyNativeShorts(long pcmPtr, short[] dst, int count) {
        nativeCopyShorts(pcmPtr, dst, count);
    }

    int pendingAudioMs() {
        return MoonBridge.getPendingAudioDuration();
    }

    long uptimeMillis() {
        return SystemClock.uptimeMillis();
    }

    boolean recreateAudioTrack() {
        return audioTrack.recreate();
    }

    // Returns false when the rest of this buffer was dropped. The caller keeps playing the next one.
    boolean writeAudioTrackFully(short[] audioData, int length) {
        return audioTrack.writeFully(audioData, length);
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
        } else {
            audioTrack.tune(now);
        }
    }

    private void reportNativeAudio(int[] stats) {
        resolveRoute(stats[7]);
        ProblemReport.setAudioDiagnostics(AudioOutputSupport.nativeDiagnostics(
                sampleRate, channels, samplesPerFrame, stats, outputDetails()));
    }

    @Override
    public synchronized int setupEncoded(MoonBridge.AudioConfiguration audioConfiguration, int codec) {
        stopped = false;
        stopRequested = false;
        return encodedOutput.setup(audioConfiguration, codec);
    }

    @RequiresApi(Build.VERSION_CODES.Q)
    AudioTrack createEncodedTrack(int encoding, int channelMask, int bufferBytes) {
        return AudioEncodedOutput.createTrack(encoding, channelMask, bufferBytes);
    }

    // A lost frame never reaches here: the sink hears a gap, never a broken frame.
    @Override
    public synchronized void playEncodedAudio(byte[] frame, int length) {
        if (stopped) return;
        encodedOutput.play(frame, length);
    }

    // Blocking and whole: a compressed track takes only complete frames.
    int writeEncoded(byte[] frame, int length) {
        return encodedOutput.write(frame, length);
    }

    @Override
    public synchronized void start() {
        // Allow equalizers to attach to this track, including a replacement after disconnect.
        audioTrack.setAudioFxWanted(true);
    }

    @Override
    public void stop() {
        // Before the monitor: a write stalled on a full output holds it for up to 100 ms.
        stopRequested = true;
        abortNativeAudio();
        synchronized (this) {
            stopped = true;
            audioTrack.setAudioFxWanted(false);
        }
    }

    @Override
    public void cleanup() {
        stopRequested = true;
        abortNativeAudio();
        cleanupLocked();
    }

    private synchronized void cleanupLocked() {
        stopped = true;
        logSummary();
        closeNativeAudio();
        reservePolicy = null;
        stallWatch = null;
        reserveFrames = 0;
        audioTrack.release();
        encodedOutput.release();
    }
}
