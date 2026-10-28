package com.limelight.utils;

import java.util.concurrent.Executor;
import java.util.concurrent.RejectedExecutionException;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.function.Consumer;
import java.util.function.Supplier;

/**
 * Takes the stream doctor's once-a-second sample off the main thread. The sample reads the decoder's
 * statistics and the connection's round trip time, which wait on locks a hanging decoder holds for
 * seconds, so the caller only hands it to the worker and the result is posted back to the main
 * executor. While a sample still runs, the next one is skipped instead of queued behind it.
 */
public final class StreamHealthSampler {
    private final Executor worker;
    private final Executor main;
    private final AtomicBoolean running = new AtomicBoolean();

    public StreamHealthSampler(Executor worker, Executor main) {
        this.worker = worker;
        this.main = main;
    }

    /** Starts a sample on the worker unless one still runs (false then); never waits for it. */
    public boolean sample(Supplier<StreamHealth.Sample> sample, Consumer<StreamHealth.Sample> result) {
        if (!running.compareAndSet(false, true)) return false;
        try {
            worker.execute(() -> {
                StreamHealth.Sample taken;
                try {
                    taken = sample.get();
                } finally {
                    running.set(false);
                }
                main.execute(() -> result.accept(taken));
            });
        } catch (RejectedExecutionException e) {
            // The stream is over and the worker shut down.
            running.set(false);
            return false;
        }
        return true;
    }
}
