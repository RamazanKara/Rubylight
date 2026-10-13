package com.limelight.binding.input.driver;

import com.limelight.LimeLog;

import java.util.function.Consumer;

final class DualSenseHaptics implements AutoCloseable {
    private final DualSenseHapticsQueue queue = new DualSenseHapticsQueue();
    private final DualSenseUsbAudio audio;
    private final Consumer<Boolean> mode;
    private final Thread worker;
    private volatile boolean stopped;

    DualSenseHaptics(DualSenseUsbAudio audio, Consumer<Boolean> mode) {
        this.audio = audio;
        this.mode = mode;
        worker = new Thread(this::play, "DualSense waveform output");
        worker.start();
    }

    boolean isReady() {
        return !stopped;
    }

    void offer(int sequence, byte[] pcm) {
        if (!stopped) queue.offer(sequence, pcm, System.nanoTime());
    }

    private void play() {
        byte[] samples = new byte[DualSenseHapticsQueue.USB_BYTES];
        boolean active = false;
        try {
            while (!stopped) {
                long now = System.nanoTime();
                boolean nextActive = queue.render(samples, now);
                if (active != nextActive) {
                    active = nextActive;
                    mode.accept(active);
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
            queue.clear();
            audio.close();
            mode.accept(false);
        }
    }

    @Override
    public void close() {
        stopped = true;
        boolean interrupted = false;
        while (worker.isAlive()) {
            try { worker.join(); }
            catch (InterruptedException e) { interrupted = true; }
        }
        if (interrupted) Thread.currentThread().interrupt();
    }
}
