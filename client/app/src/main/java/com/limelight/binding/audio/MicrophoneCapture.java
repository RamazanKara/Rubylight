package com.limelight.binding.audio;

import android.annotation.SuppressLint;
import android.media.AudioFormat;
import android.media.AudioRecord;
import android.media.MediaRecorder;
import android.os.Process;

import com.limelight.LimeLog;

import java.util.ArrayList;
import java.util.List;

// Records this device's microphone on its own thread and hands 20 ms frames of 48 kHz mono PCM
// to a sink (MoonBridge.sendMicrophonePcm encodes them as Opus and sends them to the host).
// While muted, recording stops entirely, so the system's microphone indicator goes out too; muting
// and unmuting never touch the stream itself. Echo cancellation, noise suppression and automatic
// gain follow MicProcessing; the manual gain is applied to each frame before it is sent.
public class MicrophoneCapture {
    public static final int SAMPLE_RATE = 48000;
    public static final int FRAME_MS = 20;
    public static final int FRAME_SAMPLES = SAMPLE_RATE * FRAME_MS / 1000;

    public interface FrameSink {
        // Returns a negative value when the frame was not sent.
        int send(short[] pcm, int samples);
    }

    /** Hears about effects the settings asked for that this device lacks, once per microphone open. */
    public interface EffectsListener {
        void missing(List<MicProcessing.Effect> effects);
    }

    /**
     * Hears, once, that capture failed on its own rather than through {@link #stop}. The microphone
     * and its effects are released by then, so the owner can drop this capture and open a fresh one.
     */
    public interface FailureListener {
        void failed(MicrophoneCapture capture);
    }

    private final FrameSink sink;
    private final MicProcessing processing;
    private final MicRoute route;
    private final EffectsListener effectsListener;
    private final FailureListener failureListener;
    // Set when an output device came or went; the capture thread checks the route at its next frame.
    private volatile boolean routeChanged;
    private final Object lock = new Object();
    private Thread thread;
    // Guarded by lock
    private boolean running;
    private boolean muted;
    private volatile boolean failed;

    public MicrophoneCapture(FrameSink sink, boolean muted) {
        this(sink, muted, MicProcessing.DEFAULT, null, null);
    }

    /** {@code route} may be null: the capture then treats the output as this device's speaker. */
    public MicrophoneCapture(FrameSink sink, boolean muted, MicProcessing processing, MicRoute route,
                             EffectsListener effectsListener) {
        this(sink, muted, processing, route, effectsListener, null);
    }

    public MicrophoneCapture(FrameSink sink, boolean muted, MicProcessing processing, MicRoute route,
                             EffectsListener effectsListener, FailureListener failureListener) {
        this.sink = sink;
        this.muted = muted;
        this.processing = processing == null ? MicProcessing.DEFAULT : processing;
        this.route = route;
        this.effectsListener = effectsListener;
        this.failureListener = failureListener;
    }

    public MicProcessing processing() {
        return processing;
    }

    public synchronized void start() {
        if (thread != null) {
            return;
        }
        synchronized (lock) {
            running = true;
        }
        // Only automatic echo cancellation depends on the route.
        if (route != null && processing.echo == MicProcessing.Echo.AUTO) {
            route.watch(() -> routeChanged = true);
        }
        thread = new Thread(this::run, "Microphone");
        thread.start();
    }

    public void stop() {
        Thread capture;
        synchronized (this) {
            capture = thread;
            thread = null;
        }
        if (capture == null) {
            return;
        }
        if (route != null) {
            route.watch(null);
        }
        synchronized (lock) {
            running = false;
            lock.notifyAll();
        }
        try {
            // A blocking read returns within one frame.
            capture.join(1000);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
        }
    }

    public void setMuted(boolean muted) {
        synchronized (lock) {
            if (this.muted == muted) {
                return;
            }
            this.muted = muted;
            lock.notifyAll();
        }
        LimeLog.info(muted ? "Microphone: muted" : "Microphone: unmuted");
    }

    public boolean isMuted() {
        synchronized (lock) {
            return muted;
        }
    }

    // Recording or waiting out a mute; false once capture has failed or stopped.
    public boolean isCapturing() {
        synchronized (lock) {
            return running && !failed;
        }
    }

    // Waits while muted and returns whether capture should go on.
    private boolean awaitUnmuted(AudioRecord record) throws InterruptedException {
        synchronized (lock) {
            while (running && muted) {
                if (record.getRecordingState() == AudioRecord.RECORDSTATE_RECORDING) {
                    record.stop();
                }
                lock.wait();
            }
            return running;
        }
    }

    private boolean shouldSend() {
        synchronized (lock) {
            return running && !muted;
        }
    }

    // Reads one whole frame; returns the samples read, or a negative AudioRecord error.
    private int readFrame(AudioRecord record, short[] frame) {
        int offset = 0;
        while (offset < frame.length) {
            int read = record.read(frame, offset, frame.length - offset);
            if (read <= 0) {
                return read < 0 ? read : offset;
            }
            offset += read;
        }
        return offset;
    }

    private boolean privateOutput() {
        return route != null && route.privateOutput();
    }

    /** The effects that should be on but are not: missing from the device, or failed to attach. */
    /**
     * What the settings ask for that is not on: the plan's missing effects and the ones Android
     * refused, where echo cancellation counts as on while either canceller the plan wanted runs.
     */
    static List<MicProcessing.Effect> notOn(MicProcessing.Plan plan, List<MicProcessing.Effect> failed, boolean ownOpened) {
        List<MicProcessing.Effect> missing = new ArrayList<>(plan.missing);
        List<MicProcessing.Effect> refused = new ArrayList<>(failed);
        if (plan.ownEchoCanceller) {
            if (ownOpened) refused.remove(MicProcessing.Effect.ECHO_CANCELER);
            else if (!plan.echoCanceler) refused.add(MicProcessing.Effect.ECHO_CANCELER);
        }
        for (MicProcessing.Effect effect : refused) {
            if (!missing.contains(effect)) {
                missing.add(effect);
            }
        }
        return missing;
    }

    // The caller checks RECORD_AUDIO before starting capture.
    private void run() {
        Process.setThreadPriority(Process.THREAD_PRIORITY_URGENT_AUDIO);
        short[] frame = new short[FRAME_SAMPLES];
        long[] counts = new long[2];
        try {
            boolean privateOutput = privateOutput();
            // Each pass opens the microphone for one route. A route change that flips automatic echo
            // cancellation opens it again; the stream and the host never notice.
            while (true) {
                Boolean reopenFor = capture(privateOutput, frame, counts);
                if (reopenFor == null) {
                    return;
                }
                LimeLog.info("Microphone: output is now " + (reopenFor ? "headphones" : "the speaker") +
                        ", reopening with echo cancellation " +
                        (processing.wantsEchoCancellation(reopenFor) ? "on" : "off"));
                privateOutput = reopenFor;
            }
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
        } catch (RuntimeException e) {
            // SecurityException when the permission was revoked, IllegalStateException from a
            // record the system took away.
            fail(e.toString());
        } finally {
            LimeLog.info("Microphone: stopped after " + counts[0] + " packets sent" +
                    (counts[1] > 0 ? " (" + counts[1] + " not sent)" : ""));
            if (failed) {
                releaseFailed();
            }
        }
    }

    // Runs on the capture thread once the record and its effects are released. Whichever of this and
    // stop() takes the thread first does the cleanup, so a stop is never reported as a failure.
    private void releaseFailed() {
        synchronized (this) {
            if (thread != Thread.currentThread()) {
                return;
            }
            thread = null;
        }
        if (route != null) {
            route.watch(null);
        }
        synchronized (lock) {
            running = false;
        }
        if (failureListener != null) {
            failureListener.failed(this);
        }
    }

    // Records until capture ends (null) or the route wants the other echo setting (the new route).
    @SuppressLint("MissingPermission")
    private Boolean capture(boolean privateOutput, short[] frame, long[] counts) throws InterruptedException {
        AudioRecord record = null;
        MicEffects effects = new MicEffects();
        EchoCanceller canceller = null;
        try {
            int minBuffer = AudioRecord.getMinBufferSize(SAMPLE_RATE, AudioFormat.CHANNEL_IN_MONO,
                    AudioFormat.ENCODING_PCM_16BIT);
            if (minBuffer <= 0) {
                fail("this device cannot record 48 kHz mono (" + minBuffer + ")");
                return null;
            }
            routeChanged = false;
            MicProcessing.Plan plan = processing.plan(privateOutput, effects);
            // Nightjar's own canceller hears the stream's sound itself and works on the plain MIC
            // source; VOICE_COMMUNICATION applies the device's, which hears only what the device
            // routes into its echo reference.
            record = new AudioRecord(plan.source, SAMPLE_RATE,
                    AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT,
                    Math.max(minBuffer, FRAME_SAMPLES * 2 * 4));
            if (record.getState() != AudioRecord.STATE_INITIALIZED) {
                fail("could not open the microphone");
                return null;
            }
            List<MicProcessing.Effect> failed = effects.attach(record.getAudioSessionId(), plan);
            if (plan.ownEchoCanceller) canceller = EchoCanceller.open();
            List<MicProcessing.Effect> missing = notOn(plan, failed, canceller != null);
            LimeLog.info("Microphone: started (" + FRAME_MS + " ms frames, " +
                    (plan.source == MediaRecorder.AudioSource.VOICE_COMMUNICATION ? "voice" : "plain") +
                    " source, " + echoCancellers(plan.echoCanceler && !failed.contains(MicProcessing.Effect.ECHO_CANCELER),
                    canceller != null) +
                    ", output " + (privateOutput ? "headphones" : "speaker") + ", " + processing +
                    (missing.isEmpty() ? "" : ", not on this device: " + missing) + ")");
            if (!missing.isEmpty()) {
                LimeLog.warning("Microphone: this device has no " + missing + "; the settings ask for it");
                if (effectsListener != null) {
                    effectsListener.missing(missing);
                }
            }

            while (awaitUnmuted(record)) {
                if (routeChanged) {
                    routeChanged = false;
                    boolean now = privateOutput();
                    if (processing.wantsEchoCancellation(now) != processing.wantsEchoCancellation(privateOutput)) {
                        return now;
                    }
                }
                if (record.getRecordingState() != AudioRecord.RECORDSTATE_RECORDING) {
                    record.startRecording();
                    if (record.getRecordingState() != AudioRecord.RECORDSTATE_RECORDING) {
                        fail("recording did not start (is another app using the microphone?)");
                        return null;
                    }
                    if (canceller != null) canceller.restart();
                }
                int read = readFrame(record, frame);
                if (read < 0) {
                    fail("read failed: " + read);
                    return null;
                }
                // A partial frame comes only from a stop or mute; drop it.
                if (read < FRAME_SAMPLES || !shouldSend()) {
                    continue;
                }
                // Before the gain: the canceller models the echo the microphone itself picked up.
                if (canceller != null && !canceller.process(frame, FRAME_SAMPLES)) {
                    fail("the echo canceller refused a frame");
                    return null;
                }
                MicGain.apply(frame, FRAME_SAMPLES, processing.gainPercent);
                if (sink.send(frame, FRAME_SAMPLES) >= 0) {
                    counts[0]++;
                } else {
                    counts[1]++;
                }
            }
            return null;
        } finally {
            if (record != null) {
                try {
                    if (record.getRecordingState() == AudioRecord.RECORDSTATE_RECORDING) {
                        record.stop();
                    }
                } catch (IllegalStateException ignored) {
                }
                record.release();
            }
            effects.release();
            if (canceller != null) {
                LimeLog.info("Microphone: echo canceller " + canceller.stats());
                canceller.close();
            }
        }
    }

    /** The echo cancellers on this capture, for the log. */
    static String echoCancellers(boolean android, boolean own) {
        if (android && own) return "echo cancellers Nightjar's and Android's";
        if (own) return "echo canceller Nightjar's";
        return android ? "echo canceller Android's" : "no echo canceller";
    }

    private void fail(String reason) {
        failed = true;
        LimeLog.warning("Microphone: " + reason);
    }
}
