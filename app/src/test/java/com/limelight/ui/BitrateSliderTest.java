package com.limelight.ui;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;
import static org.robolectric.Shadows.shadowOf;

import android.app.Application;
import android.os.Handler;
import android.os.Looper;
import android.widget.EditText;
import android.widget.SeekBar;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import java.time.Duration;
import java.util.concurrent.atomic.AtomicInteger;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class BitrateSliderTest {
    @Test
    public void theScaleIsVPlussAndRoundTrips() {
        assertEquals(67, BitrateSlider.MAX_PROGRESS);
        // V+'s segment boundaries.
        assertEquals(500, BitrateSlider.kbps(0));
        assertEquals(5_000, BitrateSlider.kbps(9));
        assertEquals(20_000, BitrateSlider.kbps(24));
        assertEquals(50_000, BitrateSlider.kbps(39));
        assertEquals(100_000, BitrateSlider.kbps(49));
        assertEquals(200_000, BitrateSlider.kbps(59));
        assertEquals(400_000, BitrateSlider.kbps(63));
        assertEquals(800_000, BitrateSlider.kbps(67));
        assertEquals(800_000, BitrateSlider.kbps(99));
        int previous = 0;
        for (int at = 0; at <= BitrateSlider.MAX_PROGRESS; at++) {
            int kbps = BitrateSlider.kbps(at);
            assertTrue(kbps > previous);
            assertEquals(at, BitrateSlider.progress(kbps));
            previous = kbps;
        }
        assertEquals(BitrateSlider.MAX_PROGRESS, BitrateSlider.progress(2_000_000));
        assertEquals(0, BitrateSlider.progress(100));
        assertEquals("12.5", BitrateSlider.mbpsText(12_500));
        assertEquals("800", BitrateSlider.mbpsText(800_000));
    }

    @Test
    public void keyStepsSendOnceTheyStopAndADragWhenItEnds() {
        EditText field = new EditText(RuntimeEnvironment.getApplication());
        SeekBar bar = new SeekBar(RuntimeEnvironment.getApplication());
        AtomicInteger sent = new AtomicInteger();
        BitrateSlider.Settle settle = new BitrateSlider.Settle(field, sent::incrementAndGet,
                new Handler(Looper.getMainLooper()));

        settle.onProgressChanged(bar, 24, false);
        shadowOf(Looper.getMainLooper()).idleFor(Duration.ofSeconds(1));
        assertEquals("a change not made by the user sends nothing", 0, sent.get());

        settle.onProgressChanged(bar, 24, true);
        shadowOf(Looper.getMainLooper()).idleFor(Duration.ofMillis(300));
        settle.onProgressChanged(bar, 25, true);
        shadowOf(Looper.getMainLooper()).idleFor(Duration.ofMillis(300));
        assertEquals("each step restarts the wait", 0, sent.get());
        assertEquals("22", field.getText().toString());
        shadowOf(Looper.getMainLooper()).idleFor(Duration.ofMillis(200));
        assertEquals(1, sent.get());

        settle.onStartTrackingTouch(bar);
        settle.onProgressChanged(bar, 49, true);
        shadowOf(Looper.getMainLooper()).idleFor(Duration.ofSeconds(1));
        assertEquals("nothing goes out mid-drag", 1, sent.get());
        settle.onStopTrackingTouch(bar);
        assertEquals("100", field.getText().toString());
        assertEquals(2, sent.get());
        shadowOf(Looper.getMainLooper()).idleFor(Duration.ofSeconds(1));
        assertEquals(2, sent.get());

        settle.onStartTrackingTouch(bar);
        settle.onStopTrackingTouch(bar);
        assertEquals("a touch that moved nothing sends nothing", 2, sent.get());
    }
}
