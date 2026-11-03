package com.limelight.binding.video;

import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.locks.ReentrantLock;

/**
 * Serializes calls into a native renderer against its teardown, without letting a caller wait on
 * the GPU. Stopping and freeing the renderer may wait for the GPU, for seconds when it is wedged
 * (#73: the UI waited on it and Android reported the app not responding). Once teardown has begun,
 * other calls give up instead of waiting for it, and stop() waits for the native stop only so long.
 */
final class RendererGate {
    /** How long stop() waits for the native stop before leaving it to finish on its own. */
    static final long STOP_WAIT_MS = 1000;
    private static final long POLL_MS = 5;

    private final ReentrantLock lock = new ReentrantLock();
    private final AtomicBoolean stopping = new AtomicBoolean();

    /**
     * Takes the lock for a call, waiting for other calls (which are quick) but not for teardown:
     * false, without the lock, once stop() or free() has begun. Release it with {@link #exit()}.
     */
    boolean enter() {
        try {
            while (!lock.tryLock(POLL_MS, TimeUnit.MILLISECONDS)) {
                if (stopping.get()) return false;
            }
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            return false;
        }
        if (stopping.get()) {
            lock.unlock();
            return false;
        }
        return true;
    }

    void exit() {
        lock.unlock();
    }

    /**
     * Runs nativeStop once, on its own thread under the lock, and waits up to STOP_WAIT_MS for it.
     * Later calls return at once: the first one is stopping the renderer. False only when this call
     * started the stop and stopped waiting for it.
     */
    boolean stop(Runnable nativeStop) {
        if (!stopping.compareAndSet(false, true)) return true;
        Thread worker = new Thread(() -> {
            lock.lock();
            try {
                nativeStop.run();
            } finally {
                lock.unlock();
            }
        }, "Vulkan stop");
        worker.start();
        try {
            worker.join(STOP_WAIT_MS);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
        }
        return !worker.isAlive();
    }

    /** Runs nativeFree under the lock, after any stop, however long it takes; for the shutdown worker. */
    void free(Runnable nativeFree) {
        stopping.set(true);
        lock.lock();
        try {
            nativeFree.run();
        } finally {
            lock.unlock();
        }
    }
}
