package com.limelight.utils;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotSame;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertSame;
import static org.junit.Assert.assertTrue;

import org.junit.Test;

import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.LinkedBlockingQueue;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicReference;

public class StreamHealthSamplerTest {
    private static StreamHealth.Sample sample(long atMs) {
        return new StreamHealth.Sample(atMs, true, 60, 0, 1, 2, 3, 4, 60, 60, 60, 0, false);
    }

    @Test
    public void aHangingSampleNeverBlocksTheCaller() throws Exception {
        ExecutorService worker = Executors.newSingleThreadExecutor();
        LinkedBlockingQueue<Runnable> main = new LinkedBlockingQueue<>();
        StreamHealthSampler sampler = new StreamHealthSampler(worker, main::add);
        CountDownLatch decoderHangs = new CountDownLatch(1);
        AtomicReference<Thread> sampledOn = new AtomicReference<>();
        List<StreamHealth.Sample> added = new ArrayList<>();
        StreamHealth.Sample taken = sample(1000);
        try {
            long startNs = System.nanoTime();
            assertTrue(sampler.sample(() -> {
                sampledOn.set(Thread.currentThread());
                try {
                    decoderHangs.await(10, TimeUnit.SECONDS);
                } catch (InterruptedException e) {
                    throw new AssertionError(e);
                }
                return taken;
            }, added::add));
            // A second tick while the first still hangs is skipped, not queued behind it.
            assertFalse(sampler.sample(() -> {
                throw new AssertionError("queued behind a hanging sample");
            }, added::add));
            assertTrue("the caller waited for the sample", System.nanoTime() - startNs < TimeUnit.SECONDS.toNanos(2));
            assertTrue(main.isEmpty());

            decoderHangs.countDown();
            Runnable post = main.poll(5, TimeUnit.SECONDS);
            assertNotSame(Thread.currentThread(), sampledOn.get());
            // The result reaches the stream health only through the main executor.
            assertTrue(added.isEmpty());
            post.run();
            assertEquals(1, added.size());
            assertSame(taken, added.get(0));

            // Once the sample is back, the next tick samples again.
            assertTrue(sampler.sample(() -> sample(2000), added::add));
            main.poll(5, TimeUnit.SECONDS).run();
            assertEquals(2, added.size());
        } finally {
            decoderHangs.countDown();
            worker.shutdownNow();
        }
    }

    @Test
    public void aRejectedSampleIsSkippedAndTheNextOneRuns() {
        boolean[] reject = {true};
        StreamHealthSampler sampler = new StreamHealthSampler(task -> {
            if (reject[0]) throw new java.util.concurrent.RejectedExecutionException("shut down");
            task.run();
        }, Runnable::run);
        List<StreamHealth.Sample> added = new ArrayList<>();
        assertFalse(sampler.sample(() -> sample(1000), added::add));
        reject[0] = false;
        assertTrue(sampler.sample(() -> sample(2000), added::add));
        assertEquals(1, added.size());
    }

    @Test
    public void aThrowingSampleIsSkippedAndTheNextOneRuns() throws Exception {
        ExecutorService worker = Executors.newSingleThreadExecutor();
        AtomicReference<Throwable> uncaught = new AtomicReference<>();
        LinkedBlockingQueue<Runnable> main = new LinkedBlockingQueue<>();
        StreamHealthSampler sampler = new StreamHealthSampler(task -> worker.execute(() -> {
            try {
                task.run();
            } catch (Throwable t) {
                // What the default uncaught handler would get: on Android it ends the app.
                uncaught.set(t);
            }
        }), main::add);
        List<StreamHealth.Sample> added = new ArrayList<>();
        try {
            assertTrue(sampler.sample(() -> {
                throw new IllegalStateException("decoder released");
            }, added::add));
            // The next tick finds the sampler free again and its result is posted.
            StreamHealth.Sample taken = sample(2000);
            long deadlineNs = System.nanoTime() + TimeUnit.SECONDS.toNanos(5);
            while (!sampler.sample(() -> taken, added::add)) {
                assertTrue("the failed sample never finished", System.nanoTime() < deadlineNs);
                Thread.sleep(1);
            }
            main.poll(5, TimeUnit.SECONDS).run();
            assertNull(uncaught.get());
            assertTrue(main.isEmpty());
            assertEquals(1, added.size());
            assertSame(taken, added.get(0));
        } finally {
            worker.shutdownNow();
        }
    }
}
