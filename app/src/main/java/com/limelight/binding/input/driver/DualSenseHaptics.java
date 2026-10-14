package com.limelight.binding.input.driver;

import com.limelight.LimeLog;

import java.util.function.Consumer;

final class DualSenseHaptics implements AutoCloseable {
    private final DualSenseHapticsQueue queue = new DualSenseHapticsQueue();
    private final DualSenseUsbAudio audio;
    private final DualSenseHapticsQueue speaker = new DualSenseHapticsQueue();
    private final DualSenseHapticsQueue headphones = new DualSenseHapticsQueue();
    private int headphoneSequence;
    interface Microphone {
        boolean enabled();
        boolean start();
        void finish();
        void samples(byte[] pcm, int length);
        void ready(boolean ready);
        void closed();
    }
    private volatile Microphone microphone;
    private final Thread microphoneWorker;
    private final Consumer<Boolean> mode;
    private final Thread worker;
    private volatile boolean stopped;

    DualSenseHaptics(DualSenseUsbAudio audio, Consumer<Boolean> mode) {
        this.audio = audio;
        this.mode = mode;
        microphoneWorker = new Thread(this::capture, "DualSense microphone");
        microphoneWorker.start();
        worker = new Thread(this::play, "DualSense waveform output");
        worker.start();
    }

    boolean isReady() {
        return !stopped;
    }

    void offer(int sequence, byte[] pcm) {
        if (!stopped) queue.offer(sequence, pcm, System.nanoTime());
    }

    void microphone(Microphone microphone) { this.microphone = microphone; }
    void speaker(int sequence, byte[] samples) { if (!stopped) speaker.offer(sequence, samples, System.nanoTime()); }
    synchronized void headphones(short[] pcm, int length, float volume) {
        for (int offset = 0; offset < length;) {
            int count = Math.min(480, length - offset) & ~1;
            if (count == 0) break;
            byte[] bytes = new byte[count * 2];
            for (int i = 0; i < count; i++) {
                int sample = Math.round(pcm[offset + i] * volume);
                bytes[i * 2] = (byte)sample; bytes[i * 2 + 1] = (byte)(sample >> 8);
            }
            headphones.offer(headphoneSequence++, bytes, System.nanoTime()); offset += count;
        }
    }
    private void capture() {
        byte[] samples = new byte[784];
        try {
            while (!stopped) {
                Microphone sink = microphone;
                boolean failed = false;
                if (sink != null) sink.ready(false);
                if (sink != null && sink.enabled() && sink.start()) {
                    try (DualSenseUsbMicrophone mic = audio.openMicrophone()) {
                        if (mic != null) {
                            sink.ready(true);
                            while (!stopped && sink == microphone && sink.enabled()) {
                                int length = mic.read(samples);
                                if (length < 0) { failed = true; break; }
                                if (length > 0 && !stopped && sink.enabled()) sink.samples(samples, length);
                            }
                        } else failed = true;
                    } catch (RuntimeException error) { failed = true; }
                    finally { sink.ready(false); sink.finish(); }
                }
                try { Thread.sleep(failed ? 1000 : 100); }
                catch (InterruptedException e) { Thread.currentThread().interrupt(); break; }
            }
        } finally {
            Microphone sink = microphone;
            if (sink != null) sink.closed();
        }
    }
    private void play() {
        byte[] samples = new byte[DualSenseHapticsQueue.USB_BYTES];
        boolean active = false;
        byte[] sound = new byte[DualSenseHapticsQueue.USB_BYTES];
        byte[] game = new byte[DualSenseHapticsQueue.USB_BYTES];
        try {
            while (!stopped) {
                long now = System.nanoTime();
                boolean nextActive = queue.render(samples, now);
                if (active != nextActive) {
                    active = nextActive;
                    mode.accept(active);
                }
                speaker.render(sound, now, 0, true);
                headphones.render(game, now, 0, true);
                for (int f = 0; f < DualSenseHapticsQueue.USB_FRAMES; f++) {
                    for (int c = 0; c < 4; c += 2) {
                        int at = f * 8 + c;
                        int a = (short)((sound[at] & 255) | (sound[at + 1] << 8));
                        int b = (short)((game[at] & 255) | (game[at + 1] << 8));
                        int sum = Math.max(-32768, Math.min(32767, a + b));
                        samples[at] = (byte)sum; samples[at + 1] = (byte)(sum >> 8);
                    }
                }
                // The kernel schedules 1 ms packets. Keep at most 8 ms submitted, with a
                // bounded wait for completion; never pace or send USB data on the network thread.
                if (!audio.write(samples)) {
                    LimeLog.warning("DualSense waveform output failed; reconnect the controller to retry");
                    break;
                }
            }
        }
        finally {
            stopped = true;
            queue.clear(); speaker.clear(); headphones.clear();
            microphoneWorker.interrupt();
            boolean interrupted = false;
            while (microphoneWorker.isAlive()) {
                try { microphoneWorker.join(); }
                catch (InterruptedException e) { interrupted = true; }
            }
            if (interrupted) Thread.currentThread().interrupt();
            audio.close();
            mode.accept(false);
        }
    }

    // Controller teardown runs on the main thread. A worker stuck in USB audio cleanup (its microphone
    // join, interface restore or mode switch) must not hold it; the worker finishes that on its own.
    static final long CLOSE_WAIT_MS = 500;

    @Override
    public void close() {
        stopped = true;
        long deadline = System.nanoTime() + java.util.concurrent.TimeUnit.MILLISECONDS.toNanos(CLOSE_WAIT_MS);
        boolean interrupted = false;
        while (worker.isAlive()) {
            long remainingMs = java.util.concurrent.TimeUnit.NANOSECONDS.toMillis(deadline - System.nanoTime());
            if (remainingMs <= 0) {
                LimeLog.warning("DualSense waveform output is still closing; continuing without waiting");
                break;
            }
            try { worker.join(remainingMs); }
            catch (InterruptedException e) { interrupted = true; }
        }
        if (interrupted) Thread.currentThread().interrupt();
    }
}
