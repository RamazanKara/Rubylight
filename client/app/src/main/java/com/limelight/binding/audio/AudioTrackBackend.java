package com.limelight.binding.audio;

import android.content.Context;
import android.media.AudioTrack;
import android.os.Build;
import android.os.SystemClock;

import com.limelight.LimeLog;
import com.limelight.utils.ProblemReport;

/**
 * PCM {@link AudioTrack} output backend for {@link AndroidAudioRenderer}.
 * Manages track construction, adaptive buffer tuning, dead-track recovery, equalizer sessions,
 * and AudioTrack diagnostics reporting.
 */
final class AudioTrackBackend {
    // A dead track is rebuilt at most this often; every other failure only drops one buffer.
    static final long TRACK_RECREATE_INTERVAL_MS = 1000;
    // Before Android 7.0, write() returns the native status for a dead track (DEAD_OBJECT is -32)
    // instead of ERROR_DEAD_OBJECT. A run of dropped buffers rebuilds the track there too.
    private static final int FAILED_WRITES_BEFORE_RECREATE = 5;

    private final AndroidAudioRenderer renderer;
    private final Context context;
    private final boolean enableAudioFx;

    private AudioTrack track;
    // Samples the current AudioTrack accepted; its playback head gives what it still holds.
    private long samplesWritten;
    private boolean audioFxSessionOpen;
    // The stream wants equalizers on its track: from start() until stop(), across track rebuilds.
    private boolean audioFxWanted;
    private long nextRecreateAt, nextWriteWarningAt;
    private int failedWrites;
    // What the AudioTrack really got, for the problem report: granted mode, output burst, smallest buffer we allow.
    private int perfMode = -1, outputBurst, floorFrames, minBufferFrames;

    AudioTrackBackend(AndroidAudioRenderer renderer, Context context, boolean enableAudioFx) {
        this.renderer = renderer;
        this.context = context;
        this.enableAudioFx = enableAudioFx;
    }

    AudioTrack current() {
        return track;
    }

    void delayNextRecreate(long now) {
        nextRecreateAt = now + TRACK_RECREATE_INTERVAL_MS;
    }

    int build() {
        renderer.bufferPolicy = null;
        renderer.reservePolicy = null;
        renderer.stallWatch = null;
        renderer.reserveFrames = 0;
        samplesWritten = 0;
        renderer.lastBufferCheckMs = 0;
        int bytesPerFrame = renderer.channels * renderer.samplesPerFrame * 2;

        // We're not supposed to request less than the minimum
        // buffer size for our buffer, but it appears that we can
        // do this on many devices and it lowers audio latency.
        // We'll try the small buffer size first and if it fails,
        // use the recommended larger buffer size.

        for (int i = 0; i < 4; i++) {
            // 0/1: low-latency small/large buffer; 2/3: standard small/large buffer.
            boolean lowLatency = i < 2;
            int bufferSize = (i % 2 == 0)
                    ? bytesPerFrame * 2
                    : (((Math.max(renderer.minBufferBytes(), bytesPerFrame * 2) + (bytesPerFrame - 1)) / bytesPerFrame) * bytesPerFrame);

            // Skip low latency options if hardware sample rate doesn't match or audio effects are enabled.
            if (lowLatency && (renderer.nativeRate != renderer.sampleRate || enableAudioFx)) continue;

            try {
                int minFrames = Math.max(0, renderer.minBufferBytes()) / (renderer.channels * 2);
                perfMode = -1;
                minBufferFrames = minFrames;
                outputBurst = renderer.outputFramesPerBuffer();
                floorFrames = 0;
                // The granted path is unknown until the track exists, so size for the normal mixer.
                int capacityBytes = Math.max(bufferSize, AudioBufferPolicy.trackCapacityFrames(
                        renderer.samplesPerFrame, minFrames, outputBurst) * renderer.channels * 2);
                track = renderer.createAudioTrack(renderer.channelConfig, renderer.sampleRate, capacityBytes, lowLatency);
                renderer.setReserve(capacityBytes / (renderer.channels * 2));
                // The framework may refuse the fast path. Such a track runs on the normal mixer, whose
                // period is far above two packets, so the buffer must not go below its own minimum.
                perfMode = renderer.trackPerformanceMode();
                floorFrames = AudioBufferPolicy.trackFloorFrames(Build.VERSION.SDK_INT, perfMode, lowLatency,
                        renderer.samplesPerFrame, minFrames, outputBurst);
                boolean fast = AudioBufferPolicy.trackIsFast(Build.VERSION.SDK_INT, perfMode, lowLatency);
                int actual = track.setBufferSizeInFrames(Math.max(bufferSize / (renderer.channels * 2), floorFrames));
                int granted = actual > 0 ? actual : track.getBufferSizeInFrames();
                renderer.bufferPolicy = AudioBufferPolicy.forTrack(
                        AudioBufferPolicy.trackStepFrames(fast, renderer.samplesPerFrame, outputBurst),
                        track.getBufferCapacityInFrames(), floorFrames,
                        granted, track.getUnderrunCount(), SystemClock.uptimeMillis(),
                        AudioBufferPolicy.maxReserveFrames(renderer.sampleRate));
                renderer.setReserve(granted);
                track.play();
                renderer.refreshRoute(-1);

                // Successfully created working AudioTrack. We're done here.
                LimeLog.info("Audio track configuration: " + bufferSize + " " + lowLatency + ", buffer " + renderer.reserveFrames +
                        " frames (limit " + (renderer.bufferPolicy != null ? renderer.bufferPolicy.limit() : renderer.reserveFrames) + ")" +
                        pathDetails());
                ProblemReport.setAudioDiagnostics(AudioOutputSupport.trackSetupDiagnostics(
                        renderer.sampleRate, renderer.channels, renderer.samplesPerFrame,
                        bufferSize, lowLatency, pathDetails(), renderer.outputDetails()));
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
            renderer.reserveFrames = 0;
            return -2;
        }

        return 0;
    }

    int write(short[] audioData, int offset, int length) {
        AudioTrack current = track;
        // A failed recreation leaves no track. Treat that like a dead one so recovery is retried.
        return current == null ? AudioTrack.ERROR_DEAD_OBJECT : current.write(audioData, offset, length);
    }

    // Returns false when the rest of this buffer was dropped. The caller keeps playing the next one.
    boolean writeFully(short[] audioData, int length) {
        int offset = 0;
        boolean recreated = false;
        while (offset < length) {
            int written = renderer.writeAudioTrack(audioData, offset, length - offset);
            if (written > 0 && written <= length - offset) {
                offset += written;
                samplesWritten += written;
            }
            else if ((written == AudioTrack.ERROR_DEAD_OBJECT || failedWrites + 1 >= FAILED_WRITES_BEFORE_RECREATE) &&
                    !recreated && renderer.uptimeMillis() >= nextRecreateAt) {
                // A route change can invalidate AudioTrack. Retry only the unwritten samples,
                // at most once per buffer and once a second so a broken output cannot spin on this thread.
                recreated = true;
                failedWrites = 0;
                nextRecreateAt = renderer.uptimeMillis() + TRACK_RECREATE_INTERVAL_MS;
                LimeLog.warning(written == AudioTrack.ERROR_DEAD_OBJECT ?
                        "AudioTrack disconnected; recreating audio output" :
                        "AudioTrack writes keep failing: " + written + "; recreating audio output");
                if (!renderer.recreateAudioTrack()) {
                    LimeLog.warning("Unable to recreate AudioTrack");
                    return false;
                }
            }
            else {
                failedWrites++;
                long now = renderer.uptimeMillis();
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

    boolean recreate() {
        release();
        if (renderer.setupAudioTrack() != 0) {
            return false;
        }
        // Also after an earlier attempt failed and closed the session with the old track.
        if (audioFxWanted) {
            setAudioFxSession(true);
        }
        return true;
    }

    int queuedFrames(int channels) {
        AudioTrack current = track;
        if (current == null || channels <= 0) return 0;
        try {
            // Both counters wrap at 32 bits alike; the difference stays right.
            return Math.max(0, (int)(samplesWritten / channels) - current.getPlaybackHeadPosition());
        } catch (RuntimeException e) {
            return 0;
        }
    }

    void tune(long now) {
        int underruns = track.getUnderrunCount();
        int target = renderer.bufferPolicy.sample(underruns, now);
        if (target != track.getBufferSizeInFrames()) {
            int applied = track.setBufferSizeInFrames(target);
            renderer.bufferPolicy.applied(applied);
            renderer.setReserve(applied > 0 ? applied : track.getBufferSizeInFrames());
            LimeLog.info("AudioTrack buffer " + renderer.reserveFrames + " frames, underruns " + underruns);
        }
        report(underruns);
    }

    void reportCurrent() {
        if (track != null) {
            report(track.getUnderrunCount());
        }
    }

    private void report(int underruns) {
        renderer.resolveRoute(-1);
        ProblemReport.setAudioDiagnostics(AudioOutputSupport.trackDiagnostics(
                renderer.sampleRate, renderer.channels, renderer.samplesPerFrame,
                track.getBufferSizeInFrames(), track.getBufferCapacityInFrames(), underruns,
                pathDetails(), renderer.outputDetails()));
    }

    String summary() {
        return track != null ? "AudioTrack, underruns " + track.getUnderrunCount() : null;
    }

    void setAudioFxWanted(boolean wanted) {
        audioFxWanted = wanted;
        setAudioFxSession(wanted);
    }

    private void setAudioFxSession(boolean open) {
        if (!enableAudioFx || track == null || audioFxSessionOpen == open) return;
        AudioOutputSupport.sendAudioFxSession(context, track, open);
        audioFxSessionOpen = open;
    }

    void release() {
        setAudioFxSession(false);
        AudioTrack oldTrack = track;
        track = null;
        renderer.bufferPolicy = null;
        AudioOutputSupport.releaseTrack(oldTrack, "AudioTrack was already stopped during release");
    }

    private String pathDetails() {
        return ", perfMode=" + AudioBufferPolicy.perfModeName(perfMode) + ", outputBurst=" + outputBurst +
                ", minBufferFrames=" + minBufferFrames + ", floorFrames=" + floorFrames;
    }
}
