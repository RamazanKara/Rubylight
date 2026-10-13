package com.limelight.binding.input.driver;

import java.util.ArrayDeque;
import java.util.Arrays;

/** Bounded actuator PCM queue. No USB I/O is performed on the control receive thread. */
final class DualSenseHapticsQueue {
    static final int USB_FRAMES = 192; // Four 1 ms USB packets per submission.
    static final int USB_BYTES = USB_FRAMES * 8;
    static final long STALE_NS = 20_000_000L;
    static final long IDLE_NS = 100_000_000L;
    private static final int MAX_FRAMES = 960;
    private final ArrayDeque<Chunk> chunks = new ArrayDeque<>();
    private boolean haveSequence, haveNonzero;
    private int sequence, queuedFrames;
    private long lastNonzero;

    private static final class Chunk {
        final byte[] pcm;
        final long received;
        int offset;
        Chunk(byte[] pcm, long received) {
            this.pcm = pcm.clone();
            this.received = received;
        }
    }

    synchronized void offer(int nextSequence, byte[] pcm, long now) {
        if (pcm == null || pcm.length == 0 || pcm.length > 960 || pcm.length % 4 != 0) return;
        // Signed subtraction also handles wrap from 0xffffffff to zero.
        if (haveSequence && nextSequence - sequence <= 0) return;
        if (haveSequence && nextSequence != sequence + 1) clearSamples();
        sequence = nextSequence;
        haveSequence = true;
        boolean nonzero = false;
        for (byte sample : pcm) nonzero |= sample != 0;
        if (nonzero) {
            lastNonzero = now;
            haveNonzero = true;
        }
        if (!active(now)) return; // Continuous host silence must not suppress ordinary rumble.
        int frames = pcm.length / 4;
        if (queuedFrames + frames > MAX_FRAMES) clearSamples();
        chunks.addLast(new Chunk(pcm, now));
        queuedFrames += frames;
    }

    synchronized boolean active(long now) {
        return haveNonzero && now - lastNonzero < IDLE_NS;
    }

    synchronized boolean render(byte[] usb, long now) {
        return render(usb, now, 4, true);
    }
    synchronized boolean render(byte[] usb, long now, int channelOffset, boolean clear) {
        if (clear) Arrays.fill(usb, (byte)0);
        int frame = 0;
        while (!chunks.isEmpty() && frame < USB_FRAMES) {
            Chunk chunk = chunks.peekFirst();
            if (now - chunk.received >= STALE_NS) {
                queuedFrames -= (chunk.pcm.length - chunk.offset) / 4;
                chunks.removeFirst();
                continue;
            }
            int count = Math.min(USB_FRAMES - frame, (chunk.pcm.length - chunk.offset) / 4);
            for (int i = 0; i < count; i++) {
                // Channels 1/2 are speaker audio and stay silent. Preserve signed L/R samples.
                System.arraycopy(chunk.pcm, chunk.offset, usb, frame * 8 + channelOffset, 4);
                chunk.offset += 4;
                frame++;
            }
            queuedFrames -= count;
            if (chunk.offset == chunk.pcm.length) chunks.removeFirst();
        }
        return active(now);
    }

    private void clearSamples() {
        chunks.clear();
        queuedFrames = 0;
    }

    synchronized void clear() {
        clearSamples();
        haveSequence = haveNonzero = false;
    }
}
