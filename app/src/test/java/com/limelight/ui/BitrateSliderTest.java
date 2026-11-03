package com.limelight.ui;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;
import static org.robolectric.Shadows.shadowOf;

import android.app.Application;
import android.os.Handler;
import android.os.Looper;
import android.widget.EditText;

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
    // #65: in the stream menu's bitrate dialogs D-pad Up and Down move between the slider and the fields,
    // and only Left and Right change the bitrate. Also the Radeon panel's sliders leave Up and Down alone.
    @Test
    public void dpadUpAndDownMoveTheFocusAndOnlyLeftAndRightChangeTheValue() {
        android.app.Activity activity = org.robolectric.Robolectric.buildActivity(android.app.Activity.class).setup().get();
        android.content.Context themed = new androidx.appcompat.view.ContextThemeWrapper(activity, com.limelight.R.style.StreamTheme);
        android.widget.LinearLayout column = new android.widget.LinearLayout(themed);
        column.setOrientation(android.widget.LinearLayout.VERTICAL);
        EditText field = new EditText(themed);
        field.setSingleLine(true);
        field.setText("20");
        BitrateSlider.moveFocusOnUpDown(field);
        StepSlider bar = BitrateSlider.create(themed, field, 20_000, () -> { });
        EditText fps = new EditText(themed);
        fps.setSingleLine(true);
        fps.setText("60");
        BitrateSlider.moveFocusOnUpDown(fps);
        com.google.android.material.slider.Slider radeon = new com.google.android.material.slider.Slider(themed);
        radeon.setValueFrom(0);
        radeon.setValueTo(100);
        radeon.setStepSize(1);
        radeon.setValue(50);
        for (android.view.View view : new android.view.View[] {bar, field, fps, radeon}) {
            view.setFocusable(true);
            column.addView(view, new android.widget.LinearLayout.LayoutParams(400, 60));
        }
        activity.setContentView(column);
        column.measure(android.view.View.MeasureSpec.makeMeasureSpec(400, android.view.View.MeasureSpec.EXACTLY),
                android.view.View.MeasureSpec.makeMeasureSpec(400, android.view.View.MeasureSpec.EXACTLY));
        column.layout(0, 0, 400, 400);
        int progress = bar.getPosition();

        field.requestFocus();
        field.selectAll();
        dpad(field, android.view.KeyEvent.KEYCODE_DPAD_UP);
        assertTrue("Up leaves the field at once", bar.isFocused());
        // The slider leaves Up and Down to the window, which moves the focus.
        assertEquals(false, bar.onKeyDown(android.view.KeyEvent.KEYCODE_DPAD_UP,
                new android.view.KeyEvent(android.view.KeyEvent.ACTION_DOWN, android.view.KeyEvent.KEYCODE_DPAD_UP)));
        assertEquals(false, bar.onKeyDown(android.view.KeyEvent.KEYCODE_DPAD_DOWN,
                new android.view.KeyEvent(android.view.KeyEvent.ACTION_DOWN, android.view.KeyEvent.KEYCODE_DPAD_DOWN)));
        assertEquals("Up and Down never change the bitrate", progress, bar.getPosition());
        assertEquals("20", field.getText().toString());
        field.requestFocus();
        field.setSelection(1);
        dpad(field, android.view.KeyEvent.KEYCODE_DPAD_DOWN);
        assertTrue("Down leaves the field at once", fps.isFocused());

        bar.requestFocus();
        dpad(bar, android.view.KeyEvent.KEYCODE_DPAD_RIGHT);
        assertEquals(progress + 1, bar.getPosition());

        radeon.requestFocus();
        assertEquals(false, radeon.onKeyDown(android.view.KeyEvent.KEYCODE_DPAD_UP,
                new android.view.KeyEvent(android.view.KeyEvent.ACTION_DOWN, android.view.KeyEvent.KEYCODE_DPAD_UP)));
        assertEquals(false, radeon.onKeyDown(android.view.KeyEvent.KEYCODE_DPAD_DOWN,
                new android.view.KeyEvent(android.view.KeyEvent.ACTION_DOWN, android.view.KeyEvent.KEYCODE_DPAD_DOWN)));
        assertEquals(50f, radeon.getValue(), 0);
    }

    private static void dpad(android.view.View view, int keyCode) {
        view.dispatchKeyEvent(new android.view.KeyEvent(android.view.KeyEvent.ACTION_DOWN, keyCode));
        view.dispatchKeyEvent(new android.view.KeyEvent(android.view.KeyEvent.ACTION_UP, keyCode));
    }

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
        StepSlider bar = new StepSlider(new androidx.appcompat.view.ContextThemeWrapper(
                RuntimeEnvironment.getApplication(), com.limelight.R.style.StreamTheme));
        bar.setMax(BitrateSlider.MAX_PROGRESS);
        AtomicInteger sent = new AtomicInteger();
        BitrateSlider.Settle settle = new BitrateSlider.Settle(field, sent::incrementAndGet,
                new Handler(Looper.getMainLooper()));
        bar.setOnPositionChangeListener(settle);

        bar.setPosition(24);
        shadowOf(Looper.getMainLooper()).idleFor(Duration.ofSeconds(1));
        assertEquals("a change not made by the user sends nothing", 0, sent.get());

        settle.onPositionChanged(bar, 24);
        shadowOf(Looper.getMainLooper()).idleFor(Duration.ofMillis(300));
        settle.onPositionChanged(bar, 25);
        shadowOf(Looper.getMainLooper()).idleFor(Duration.ofMillis(300));
        assertEquals("each step restarts the wait", 0, sent.get());
        assertEquals("22", field.getText().toString());
        shadowOf(Looper.getMainLooper()).idleFor(Duration.ofMillis(200));
        assertEquals(1, sent.get());

        settle.onStartTrackingTouch(bar);
        settle.onPositionChanged(bar, 49);
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
