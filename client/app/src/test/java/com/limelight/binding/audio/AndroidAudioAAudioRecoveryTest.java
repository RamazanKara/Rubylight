package com.limelight.binding.audio;

import android.app.Application;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import java.util.List;
import java.util.concurrent.TimeUnit;

import static com.limelight.binding.audio.AndroidAudioRendererTest.ScriptedRenderer;
import static com.limelight.binding.audio.AndroidAudioRendererTest.afterAStall;
import static com.limelight.binding.audio.AndroidAudioRendererTest.disconnectedAAudio;
import static org.junit.Assert.*;

/**
 * Unit tests for {@link AndroidAudioRenderer} AAudio recovery: disconnect reopens, short and
 * lasting write stalls, concealed late-packet skipping, stop/cleanup aborts, and AudioTrack fallback.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class AndroidAudioAAudioRecoveryTest {
    @Test public void persistentNativeUnderrunsSwitchOutputBeforeWritingTheNextPacket() {
        ScriptedRenderer renderer = new ScriptedRenderer(6);
        AudioBufferPolicy policy = new AudioBufferPolicy(192, 384, 384, 0, 0);
        for (int i = 1; i <= 3; i++) policy.sample(i, i * 1000L);
        ReflectionHelpers.setField(renderer, "nativeAudio", 1L);
        ReflectionHelpers.setField(renderer, "bufferPolicy", policy);
        renderer.play();
        assertEquals("AAudio hardware keeps underrunning at its largest device buffer", renderer.fallbackReason);
        assertEquals(List.of(0), renderer.offsets);
        assertEquals(List.of(6), renderer.lengths);
    }

    @Test public void aStallsConcealedPacketsAreSkippedNotWrittenOrCountedAsDrops() {
        ScriptedRenderer renderer = afterAStall(6);
        ReflectionHelpers.setField(renderer, "nativeAudio", 1L);
        ReflectionHelpers.setField(renderer, "bufferPolicy", new AudioBufferPolicy(864, 6912, 1728, 0, 0));
        ReflectionHelpers.setField(renderer, "lastBufferCheckMs", Long.MAX_VALUE);
        renderer.nativeWriteSucceeds = false;
        renderer.late = true;
        renderer.play();
        // Asked per packet in frames, with the network queue behind it in frames (130 ms at 48 kHz): those
        // packets refill the reserve, so the native side skips only the excess. Skipped before any write or fallback.
        assertEquals(List.of(3), renderer.lateChecks);
        assertEquals(List.of(6240), renderer.latePending);
        assertNull(renderer.fallbackReason);
        assertEquals(0, renderer.nextResult);
        assertEquals(Long.valueOf(0), ReflectionHelpers.<Long>getField(renderer, "lateDropped"));
        assertEquals(Long.valueOf(0), ReflectionHelpers.<Long>getField(renderer, "droppedWrites"));
        // A packet in time is written as before.
        renderer.late = false;
        renderer.nativeWriteSucceeds = true;
        renderer.play();
        assertNull(renderer.fallbackReason);
        assertEquals(List.of(3, 3), renderer.lateChecks);
        ReflectionHelpers.setField(renderer, "nativeAudio", 0L);
    }

    @Test public void aaudioReserveStarvationDoesNotSwitchToAudioTrack() {
        ScriptedRenderer renderer = afterAStall();
        AudioBufferPolicy starved = AudioBufferPolicy.forReserve(192, 384, 384, 0, 0, 0);
        for (int i = 1; i <= 4; i++) starved.sample(i, i * 1000L);
        assertTrue(starved.needsFallback());
        ReflectionHelpers.setField(renderer, "nativeAudio", 1L);
        ReflectionHelpers.setField(renderer, "bufferPolicy", new AudioBufferPolicy(192, 384, 384, 0, 0));
        ReflectionHelpers.setField(renderer, "reservePolicy", starved);
        ReflectionHelpers.setField(renderer, "lastBufferCheckMs", Long.MAX_VALUE);
        renderer.play();
        // Late network audio would starve AudioTrack the same way: AAudio keeps it.
        assertNull(renderer.fallbackReason);
        assertEquals(0, renderer.nextResult);
        assertEquals(Long.valueOf(0), ReflectionHelpers.<Long>getField(renderer, "lateDropped"));
        assertEquals(Long.valueOf(0), ReflectionHelpers.<Long>getField(renderer, "droppedWrites"));
        ReflectionHelpers.setField(renderer, "nativeAudio", 0L);
    }

    @Test public void aaudioFailuresThatEndTheBufferAreCountedAsDropped() {
        // Falling back from persistent underruns fails: the buffer is lost.
        ScriptedRenderer renderer = new ScriptedRenderer();
        renderer.fallbackSucceeds = false;
        AudioBufferPolicy policy = new AudioBufferPolicy(192, 384, 384, 0, 0);
        for (int i = 1; i <= 3; i++) policy.sample(i, i * 1000L);
        ReflectionHelpers.setField(renderer, "nativeAudio", 1L);
        ReflectionHelpers.setField(renderer, "bufferPolicy", policy);
        renderer.play();
        assertEquals(Long.valueOf(1), ReflectionHelpers.<Long>getField(renderer, "droppedWrites"));

        // The stream disconnects, AAudio does not reopen and the fallback cannot start either.
        renderer = new ScriptedRenderer();
        renderer.fallbackSucceeds = false;
        renderer.nativeWriteSucceeds = false;
        ReflectionHelpers.setField(renderer, "nativeAudio", 1L);
        ReflectionHelpers.setField(renderer, "bufferPolicy", new AudioBufferPolicy(192, 384, 384, 0, 0));
        renderer.play();
        assertEquals(1, renderer.reopens);
        assertEquals("AAudio disconnected and AAudio did not reopen", renderer.fallbackReason);
        assertEquals(Long.valueOf(1), ReflectionHelpers.<Long>getField(renderer, "droppedWrites"));
    }

    @Test public void aRouteChangeReopensAAudioInsteadOfFallingBack() {
        // Headphones plugged in: the stream disconnects, AAudio opens on the new route and plays the buffer.
        ScriptedRenderer renderer = disconnectedAAudio();
        renderer.nativeWriteSucceeds = true;
        renderer.failNextNativeWrites = 1;
        renderer.reopenSucceeds = true;
        renderer.play();
        assertEquals(1, renderer.closes);
        assertEquals(1, renderer.reopens);
        assertNull(renderer.fallbackReason);
        assertEquals(Long.valueOf(2), ReflectionHelpers.<Long>getField(renderer, "nativeAudio"));
        assertEquals("the failed write, then the same buffer on the new stream", 2, renderer.nativeWrites);
        assertEquals(0, renderer.nextResult);
        assertEquals(Long.valueOf(0), ReflectionHelpers.<Long>getField(renderer, "droppedWrites"));
        // Unplugged again much later: reopened again, never AudioTrack.
        renderer.failNextNativeWrites = 1;
        renderer.now += 60000;
        renderer.play();
        assertEquals(2, renderer.reopens);
        assertNull(renderer.fallbackReason);
        assertEquals(Long.valueOf(0), ReflectionHelpers.<Long>getField(renderer, "droppedWrites"));
        ReflectionHelpers.setField(renderer, "nativeAudio", 0L);
    }

    @Test public void stopAndCleanupDoNotWaitBehindAWriteStalledOnAFullOutput() throws Exception {
        ScriptedRenderer renderer = disconnectedAAudio();
        renderer.stallWrites = true;
        renderer.reopenSucceeds = true;
        Thread decoder = new Thread(renderer::play);
        decoder.start();
        assertTrue(renderer.writeWaiting.await(5, TimeUnit.SECONDS));
        // The write holds the renderer's monitor: stop() has to reach the output first.
        long start = System.nanoTime();
        renderer.stop();
        decoder.join(5000);
        assertFalse(decoder.isAlive());
        assertTrue("the write was cut short by the abort, not its own wait", renderer.writeWasAborted);
        assertTrue("stop() waited behind the write", System.nanoTime() - start < 5_000_000_000L);
        // A write the stop cut short is no disconnect: nothing reopens, the buffer is dropped.
        assertEquals(0, renderer.reopens);
        assertEquals(0, renderer.closes);
        assertEquals(Long.valueOf(1), ReflectionHelpers.<Long>getField(renderer, "droppedWrites"));
        assertEquals(1, renderer.aborts);
        // cleanup() aborts first too, then closes the output under the monitor (no native stats here).
        ReflectionHelpers.setField(renderer, "nativeAudio", 0L);
        renderer.cleanup();
        assertEquals("stop() and cleanup() each abort before taking the monitor", 2, renderer.aborts);
    }

    @Test public void aShortAAudioStallDropsTheBufferWithoutReopening() {
        // The callback held back for 150 ms (a Bluetooth handover) while the stream still runs:
        // stalls with buffers played between them drop theirs and never reopen or fall back.
        ScriptedRenderer renderer = disconnectedAAudio();
        renderer.nativeWriteSucceeds = true;
        renderer.reopenSucceeds = true;
        for (int i = 0; i <= AudioBufferPolicy.MAX_QUICK_REOPENS; i++) {
            for (int stall = 1; stall < AndroidAudioRenderer.MAX_NATIVE_STALLS; stall++) {
                renderer.stallNextNativeWrites = 1;
                renderer.play();
            }
            renderer.play();
            renderer.now += 200;
        }
        long dropped = (AudioBufferPolicy.MAX_QUICK_REOPENS + 1) * (AndroidAudioRenderer.MAX_NATIVE_STALLS - 1);
        assertEquals(0, renderer.reopens);
        assertEquals(0, renderer.closes);
        assertNull(renderer.fallbackReason);
        assertEquals(0, renderer.nextResult);
        assertEquals(Long.valueOf(dropped), ReflectionHelpers.<Long>getField(renderer, "droppedWrites"));
        // The next buffer plays on the same stream.
        renderer.play();
        assertEquals(Long.valueOf(1), ReflectionHelpers.<Long>getField(renderer, "nativeAudio"));
        assertEquals(Long.valueOf(dropped), ReflectionHelpers.<Long>getField(renderer, "droppedWrites"));
        ReflectionHelpers.setField(renderer, "nativeAudio", 0L);
    }

    @Test public void aLastingAAudioStallReopensAndKeepsFallingBackIfItRepeats() {
        // The callback stopped for good without an error callback: after MAX_NATIVE_STALLS writes in a
        // row the stream is reopened, and the buffer that found it stopped plays on the new one.
        ScriptedRenderer renderer = disconnectedAAudio();
        renderer.nativeWriteSucceeds = true;
        renderer.reopenSucceeds = true;
        renderer.stallNextNativeWrites = AndroidAudioRenderer.MAX_NATIVE_STALLS;
        for (int i = 0; i < AndroidAudioRenderer.MAX_NATIVE_STALLS; i++) renderer.play();
        assertEquals(1, renderer.reopens);
        assertEquals(1, renderer.closes);
        assertNull(renderer.fallbackReason);
        assertEquals(Long.valueOf(2), ReflectionHelpers.<Long>getField(renderer, "nativeAudio"));
        assertEquals(Long.valueOf(AndroidAudioRenderer.MAX_NATIVE_STALLS - 1),
                ReflectionHelpers.<Long>getField(renderer, "droppedWrites"));

        // Stopping again right after each reopen ends on AudioTrack, as a disconnect does.
        for (int round = 0; round < AudioBufferPolicy.MAX_QUICK_REOPENS && renderer.fallbackReason == null; round++) {
            renderer.now += 200;
            renderer.stallNextNativeWrites = AndroidAudioRenderer.MAX_NATIVE_STALLS;
            for (int i = 0; i < AndroidAudioRenderer.MAX_NATIVE_STALLS; i++) renderer.play();
        }
        assertNotNull(renderer.fallbackReason);
        assertTrue(renderer.fallbackReason, renderer.fallbackReason.startsWith("AAudio stopped taking audio"));
    }

    @Test public void anAAudioThatDoesNotReopenFallsBackToAudioTrack() {
        ScriptedRenderer renderer = disconnectedAAudio();
        renderer.play();
        assertEquals(1, renderer.reopens);
        assertEquals("AAudio disconnected and AAudio did not reopen", renderer.fallbackReason);
        assertEquals("the buffer goes to the AudioTrack", 1, renderer.nextResult);
    }

    @Test public void anAAudioThatKeepsDyingRightAfterReopeningFallsBack() {
        ScriptedRenderer renderer = disconnectedAAudio();
        renderer.reopenSucceeds = true;
        for (int i = 0; i < AudioBufferPolicy.MAX_QUICK_REOPENS; i++) {
            renderer.play();
            renderer.now += 200;
            assertNull(renderer.fallbackReason);
        }
        assertEquals(AudioBufferPolicy.MAX_QUICK_REOPENS, renderer.reopens);
        renderer.play();
        assertEquals("no fourth reopen within a second of each", AudioBufferPolicy.MAX_QUICK_REOPENS, renderer.reopens);
        assertEquals("AAudio disconnected 3 times right after reopening", renderer.fallbackReason);
    }
}
