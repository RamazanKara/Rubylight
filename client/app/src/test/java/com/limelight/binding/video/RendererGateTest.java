package com.limelight.binding.video;

import org.junit.Test;

import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.Assert.*;

// #73: the UI waited on the Vulkan renderer's lock while its teardown waited for the GPU.
public class RendererGateTest {
    @Test(timeout = 5000)
    public void callsGiveUpWhileAFreeWaitsForTheGpu() throws Exception {
        RendererGate gate = new RendererGate();
        CountDownLatch freeing = new CountDownLatch(1);
        CountDownLatch gpu = new CountDownLatch(1);
        Thread worker = new Thread(() -> gate.free(() -> {
            freeing.countDown();
            await(gpu);
        }));
        worker.start();
        assertTrue(freeing.await(1, TimeUnit.SECONDS));

        long start = System.nanoTime();
        assertFalse(gate.enter());
        assertTrue(gate.stop(() -> fail("a stop after free has begun must not run")));
        assertTrue(TimeUnit.NANOSECONDS.toMillis(System.nanoTime() - start) < 200);
        gpu.countDown();
        worker.join();
    }

    @Test(timeout = 5000)
    public void stopWaitsOnlySoLongForAWedgedGpuAndFreeWaitsForIt() throws Exception {
        RendererGate gate = new RendererGate();
        CountDownLatch gpu = new CountDownLatch(1);
        AtomicInteger order = new AtomicInteger();
        long start = System.nanoTime();
        assertFalse(gate.stop(() -> {
            await(gpu);
            assertEquals(0, order.getAndIncrement());
        }));
        long waited = TimeUnit.NANOSECONDS.toMillis(System.nanoTime() - start);
        assertTrue(waited >= RendererGate.STOP_WAIT_MS - 50 && waited < RendererGate.STOP_WAIT_MS + 500);
        // Later stops return at once; the first one is still stopping.
        start = System.nanoTime();
        assertTrue(gate.stop(() -> fail("stopped twice")));
        assertFalse(gate.enter());
        assertTrue(TimeUnit.NANOSECONDS.toMillis(System.nanoTime() - start) < 200);

        Thread worker = new Thread(() -> gate.free(() -> assertEquals(1, order.getAndIncrement())));
        worker.start();
        Thread.sleep(50);
        assertEquals(0, order.get());
        gpu.countDown();
        worker.join();
        assertEquals(2, order.get());
    }

    @Test(timeout = 5000)
    public void runningCallsWaitForEachOtherAndAQuickStopIsWaitedFor() throws Exception {
        RendererGate gate = new RendererGate();
        assertTrue(gate.enter());
        CountDownLatch entered = new CountDownLatch(1);
        Thread other = new Thread(() -> {
            if (gate.enter()) {
                entered.countDown();
                gate.exit();
            }
        });
        other.start();
        assertFalse(entered.await(100, TimeUnit.MILLISECONDS));
        gate.exit();
        assertTrue(entered.await(1, TimeUnit.SECONDS));
        other.join();

        AtomicInteger stops = new AtomicInteger();
        assertTrue(gate.stop(stops::incrementAndGet));
        assertEquals(1, stops.get());
        assertFalse(gate.enter());
    }

    @Test(timeout = 5000)
    public void anInterruptedThreadStillGetsItsCallsAndKeepsItsFlag() throws Exception {
        RendererGate gate = new RendererGate();
        Thread.currentThread().interrupt();
        try {
            assertTrue(gate.enter());
            gate.exit();
            assertTrue(Thread.currentThread().isInterrupted());

            // Also while another call holds the lock and the wait would throw
            CountDownLatch held = new CountDownLatch(1);
            CountDownLatch release = new CountDownLatch(1);
            Thread other = new Thread(() -> {
                if (gate.enter()) {
                    held.countDown();
                    await(release);
                    gate.exit();
                }
            });
            other.start();
            Thread.interrupted();
            assertTrue(held.await(1, TimeUnit.SECONDS));
            Thread.currentThread().interrupt();
            new Thread(() -> {
                try {
                    Thread.sleep(100);
                } catch (InterruptedException ignored) {
                }
                release.countDown();
            }).start();
            assertTrue(gate.enter());
            gate.exit();
            assertTrue(Thread.currentThread().isInterrupted());
            // join() throws on an interrupted thread when the other one has not ended yet
            Thread.interrupted();
            other.join();
            Thread.currentThread().interrupt();

            AtomicInteger stops = new AtomicInteger();
            assertTrue(gate.stop(stops::incrementAndGet));
            assertEquals(1, stops.get());
            assertTrue(Thread.currentThread().isInterrupted());
        } finally {
            Thread.interrupted();
        }
    }

    private static void await(CountDownLatch latch) {
        try {
            latch.await();
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
        }
    }
}
