package com.limelight.binding.audio;

import android.app.Application;
import android.media.AudioRecord;

import org.junit.After;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowAudioRecord;

import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicReference;

import static org.junit.Assert.*;

/** A capture that fails on its own lets go of itself and says so; a stop never counts as a failure. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class MicrophoneCaptureTest {
    @After public void clearSource() {
        ShadowAudioRecord.clearSource();
    }

    @Test public void aReadErrorReleasesTheCaptureAndTellsItsOwnerOnce() throws Exception {
        ShadowAudioRecord.setSource(new ShadowAudioRecord.AudioRecordSource() {
            @Override public int readInShortArray(short[] data, int offset, int size, boolean blocking) {
                return AudioRecord.ERROR_DEAD_OBJECT;
            }
        });
        CountDownLatch failed = new CountDownLatch(1);
        AtomicInteger reports = new AtomicInteger();
        AtomicReference<MicrophoneCapture> reported = new AtomicReference<>();
        MicrophoneCapture capture = new MicrophoneCapture((pcm, samples) -> 0, false, MicProcessing.DEFAULT,
                null, null, which -> {
                    reports.incrementAndGet();
                    reported.set(which);
                    failed.countDown();
                });
        capture.start();
        assertTrue("The failure is reported", failed.await(5, TimeUnit.SECONDS));
        assertSame(capture, reported.get());
        assertFalse(capture.isCapturing());
        // The capture already let go of its thread, so a stop from the owner has nothing to wait for.
        long before = System.nanoTime();
        capture.stop();
        assertTrue(TimeUnit.NANOSECONDS.toMillis(System.nanoTime() - before) < 500);
        assertEquals(1, reports.get());
    }

    @Test public void aStopIsNotAFailure() throws Exception {
        CountDownLatch reading = new CountDownLatch(1);
        ShadowAudioRecord.setSource(new ShadowAudioRecord.AudioRecordSource() {
            @Override public int readInShortArray(short[] data, int offset, int size, boolean blocking) {
                reading.countDown();
                try {
                    Thread.sleep(MicrophoneCapture.FRAME_MS);
                } catch (InterruptedException e) {
                    Thread.currentThread().interrupt();
                }
                return size;
            }
        });
        AtomicInteger reports = new AtomicInteger();
        MicrophoneCapture capture = new MicrophoneCapture((pcm, samples) -> 0, false, MicProcessing.DEFAULT,
                null, null, which -> reports.incrementAndGet());
        capture.start();
        assertTrue(reading.await(5, TimeUnit.SECONDS));
        assertTrue(capture.isCapturing());
        capture.stop();
        assertFalse(capture.isCapturing());
        assertEquals(0, reports.get());
    }
}
