package com.limelight.binding.audio;

import android.annotation.SuppressLint;
import android.media.AudioFormat;
import android.media.AudioRecord;
import android.media.MediaRecorder;
import android.media.audiofx.AcousticEchoCanceler;
import android.media.audiofx.NoiseSuppressor;
import android.os.Process;

import com.limelight.LimeLog;

// Records this device's microphone on its own thread and hands 20 ms frames of 48 kHz mono PCM
// to a sink (MoonBridge.sendMicrophonePcm encodes them as Opus and sends them to the host).
// While muted, recording stops entirely, so the system's microphone indicator goes out too.
public class MicrophoneCapture {
    public static final int SAMPLE_RATE = 48000;
    public static final int FRAME_MS = 20;
    public static final int FRAME_SAMPLES = SAMPLE_RATE * FRAME_MS / 1000;

    public interface FrameSink {
        // Returns a negative value when the frame was not sent.
        int send(short[] pcm, int samples);
    }

    private final FrameSink sink;
    private final Object lock = new Object();
    private Thread thread;
    // Guarded by lock
    private boolean running;
    private boolean muted;
    private volatile boolean failed;

    public MicrophoneCapture(FrameSink sink, boolean muted) {
        this.sink = sink;
        this.muted = muted;
    }

    public synchronized void start() {
        if (thread != null) {
            return;
        }
        synchronized (lock) {
            running = true;
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

    // The caller checks RECORD_AUDIO before starting capture.
    @SuppressLint("MissingPermission")
    private void run() {
        Process.setThreadPriority(Process.THREAD_PRIORITY_URGENT_AUDIO);
        AudioRecord record = null;
        AcousticEchoCanceler echoCanceler = null;
        NoiseSuppressor noiseSuppressor = null;
        short[] frame = new short[FRAME_SAMPLES];
        long sent = 0, notSent = 0;
        try {
            int minBuffer = AudioRecord.getMinBufferSize(SAMPLE_RATE, AudioFormat.CHANNEL_IN_MONO,
                    AudioFormat.ENCODING_PCM_16BIT);
            if (minBuffer <= 0) {
                fail("this device cannot record 48 kHz mono (" + minBuffer + ")");
                return;
            }
            // VOICE_COMMUNICATION applies the device's echo cancellation, which keeps game audio
            // from this device's speaker out of the microphone.
            record = new AudioRecord(MediaRecorder.AudioSource.VOICE_COMMUNICATION, SAMPLE_RATE,
                    AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT,
                    Math.max(minBuffer, FRAME_SAMPLES * 2 * 4));
            if (record.getState() != AudioRecord.STATE_INITIALIZED) {
                fail("could not open the microphone");
                return;
            }
            int session = record.getAudioSessionId();
            if (AcousticEchoCanceler.isAvailable()) {
                echoCanceler = AcousticEchoCanceler.create(session);
                if (echoCanceler != null) {
                    echoCanceler.setEnabled(true);
                }
            }
            if (NoiseSuppressor.isAvailable()) {
                noiseSuppressor = NoiseSuppressor.create(session);
                if (noiseSuppressor != null) {
                    noiseSuppressor.setEnabled(true);
                }
            }
            LimeLog.info("Microphone: started (" + FRAME_MS + " ms frames, echo canceller " +
                    (echoCanceler != null ? "on" : "unavailable") + ", noise suppressor " +
                    (noiseSuppressor != null ? "on" : "unavailable") + ")");

            while (awaitUnmuted(record)) {
                if (record.getRecordingState() != AudioRecord.RECORDSTATE_RECORDING) {
                    record.startRecording();
                    if (record.getRecordingState() != AudioRecord.RECORDSTATE_RECORDING) {
                        fail("recording did not start (is another app using the microphone?)");
                        return;
                    }
                }
                int read = readFrame(record, frame);
                if (read < 0) {
                    fail("read failed: " + read);
                    return;
                }
                // A partial frame comes only from a stop or mute; drop it.
                if (read < FRAME_SAMPLES || !shouldSend()) {
                    continue;
                }
                if (sink.send(frame, FRAME_SAMPLES) >= 0) {
                    sent++;
                } else {
                    notSent++;
                }
            }
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
        } catch (RuntimeException e) {
            // SecurityException when the permission was revoked, IllegalStateException from a
            // record the system took away.
            fail(e.toString());
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
            if (echoCanceler != null) {
                echoCanceler.release();
            }
            if (noiseSuppressor != null) {
                noiseSuppressor.release();
            }
            LimeLog.info("Microphone: stopped after " + sent + " packets sent" +
                    (notSent > 0 ? " (" + notSent + " not sent)" : ""));
        }
    }

    private void fail(String reason) {
        failed = true;
        LimeLog.warning("Microphone: " + reason);
    }
}
