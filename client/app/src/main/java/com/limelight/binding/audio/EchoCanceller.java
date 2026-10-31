package com.limelight.binding.audio;

import com.limelight.LimeLog;

import java.util.concurrent.atomic.AtomicReference;

/**
 * Nightjar's own echo canceller for the microphone: WebRTC's AEC3 in libnightjar_core.so
 * (protocol/android/src/echo.rs), fed with the stream's sound as the renderer hands it to the
 * output. Android's AcousticEchoCanceler hears only what the device routes into its echo reference,
 * which can leave the stream's game sound out; this one hears exactly what Nightjar plays.
 *
 * One canceller is active at a time: the microphone opens it, the renderer feeds the active one
 * through {@link #reference}, and closing it ends the feed.
 */
public final class EchoCanceller {
    /** AEC3 takes whole 10 ms frames at 48 kHz. */
    public static final int RATE = 48000, FRAME_SAMPLES = RATE / 100;

    // Loaded on the microphone's first open, never on the renderer's thread.
    private static Boolean available;
    // Cleared only by the canceller it holds: a capture that outlived its stop() cannot
    // unhook the next one's.
    private static final AtomicReference<EchoCanceller> active = new AtomicReference<>();

    // The renderer's thread takes referenceLock, the microphone's captureLock; close() takes both,
    // so neither thread can pass a destroyed handle.
    private final Object referenceLock = new Object(), captureLock = new Object();
    private long handle;

    private static native long nativeCreate();
    private static native void nativeReference(long handle, short[] pcm, int length, int channels, int outputMs);
    private static native boolean nativeProcess(long handle, short[] pcm, int samples);
    private static native void nativeRestart(long handle);
    private static native int[] nativeStats(long handle);
    private static native void nativeDestroy(long handle);

    private static boolean load() {
        try {
            System.loadLibrary("nightjar_core");
            return true;
        } catch (UnsatisfiedLinkError e) {
            LimeLog.warning("Microphone: Nightjar's echo canceller is unavailable: " + e.getMessage());
            return false;
        }
    }

    private EchoCanceller(long handle) {
        this.handle = handle;
    }

    /** Whether the library with the canceller loads. */
    public static synchronized boolean isAvailable() {
        if (available == null) available = load();
        return available;
    }

    /** Opens a canceller and makes it the one the renderer feeds; null when the library is missing. */
    public static EchoCanceller open() {
        if (!isAvailable()) return null;
        long handle = nativeCreate();
        if (handle == 0) return null;
        EchoCanceller canceller = new EchoCanceller(handle);
        active.set(canceller);
        return canceller;
    }

    /**
     * What the renderer just handed to the output: {@code length} samples of {@code pcm} at 48 kHz,
     * interleaved with {@code channels} channels, behind {@code outputMs} of the output's queue.
     * Costs one volatile read while no microphone cancels echo.
     */
    public static void reference(short[] pcm, int length, int channels, int outputMs) {
        EchoCanceller canceller = active.get();
        if (canceller == null) return;
        synchronized (canceller.referenceLock) {
            if (canceller.handle != 0) nativeReference(canceller.handle, pcm, length, channels, outputMs);
        }
    }

    /** Takes the stream's sound out of a frame of whole 10 ms blocks in place; false leaves it as it was. */
    public boolean process(short[] pcm, int samples) {
        synchronized (captureLock) {
            return handle != 0 && nativeProcess(handle, pcm, samples);
        }
    }

    /** The microphone reads again after a pause: what played meanwhile is in none of its frames. */
    public void restart() {
        synchronized (captureLock) {
            if (handle != 0) nativeRestart(handle);
        }
    }

    /** AEC3's echo return loss enhancement and delay for the log, once it has estimated them. */
    public String stats() {
        int[] stats;
        synchronized (captureLock) {
            stats = handle != 0 ? nativeStats(handle) : null;
        }
        return describe(stats);
    }

    /** The native stats: {ERLE measured, ERLE in tenths of a dB, delay in ms or -1, any sound heard}. */
    static String describe(int[] stats) {
        if (stats == null || stats.length < 4) return "no estimate";
        // Audio passthrough hands the output compressed frames, none of which reach the canceller.
        if (stats[3] == 0) return "heard none of the stream's sound, so it removed nothing";
        return (stats[0] != 0 ? String.format(java.util.Locale.ROOT, "echo removed %.1f dB", stats[1] / 10.0) : "echo not measured") +
                (stats[2] >= 0 ? ", delay " + stats[2] + " ms" : "");
    }

    public void close() {
        active.compareAndSet(this, null);
        synchronized (referenceLock) {
            synchronized (captureLock) {
                if (handle != 0) {
                    nativeDestroy(handle);
                    handle = 0;
                }
            }
        }
    }
}
