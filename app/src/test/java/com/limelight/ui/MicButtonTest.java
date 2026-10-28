package com.limelight.ui;

import android.app.Application;
import android.os.SystemClock;
import android.view.MotionEvent;
import android.view.View;
import android.widget.FrameLayout;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowLooper;

import static org.junit.Assert.*;

/** The stream's microphone button: where it sits, what a tap does, and when it shows. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, qualifiers = "en-w800dp-h360dp-land-xhdpi", application = Application.class)
public class MicButtonTest {
    private static final int WIDTH = 1600, HEIGHT = 720;
    private static final float SIZE = 96, MARGIN = 24; // 48 dp and 12 dp at xhdpi

    private static final class FakeHost implements MicButton.Host {
        boolean offered = true, muted;
        float rowBottom = Float.NaN;
        @Override public boolean offered() { return offered; }
        @Override public boolean muted() { return muted; }
        @Override public void setMuted(boolean muted) { this.muted = muted; }
        @Override public float topRowBottom() { return rowBottom; }
    }

    private final FakeHost host = new FakeHost();
    private FrameLayout frame;
    private MicButton button;

    @Before
    public void setUp() {
        RuntimeEnvironment.getApplication().getSharedPreferences(MicButton.PREFS, 0).edit().clear().commit();
        UiNavigationTest.Screen screen = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup().visible().get();
        frame = new FrameLayout(screen);
        screen.setContentView(frame, new FrameLayout.LayoutParams(WIDTH, HEIGHT));
        button = new MicButton(screen, host);
        frame.addView(button, button.layoutParams());
        button.setSuppressed(false);
        layout();
    }

    private void layout() {
        ShadowLooper.idleMainLooper();
        frame.measure(View.MeasureSpec.makeMeasureSpec(WIDTH, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(HEIGHT, View.MeasureSpec.EXACTLY));
        frame.layout(0, 0, WIDTH, HEIGHT);
        ShadowLooper.idleMainLooper();
    }

    private float centerX() {
        return button.getTranslationX() + button.getWidth() / 2f;
    }

    private float centerY() {
        return button.getTranslationY() + button.getHeight() / 2f;
    }

    // Placement

    @Test public void itIsAFullTouchTarget() {
        assertEquals(SIZE, button.getWidth(), 0);
        assertEquals(SIZE, button.getHeight(), 0);
    }

    @Test public void byDefaultItSitsTopCenterOrBelowTheControllersTopRow() {
        assertEquals(WIDTH / 2f, centerX(), 1);
        assertEquals(MARGIN + SIZE / 2, centerY(), 1);
        host.rowBottom = 120;
        button.place();
        assertEquals("Below the View, Home and Menu row", 120 + MARGIN + SIZE / 2, centerY(), 1);
    }

    @Test public void aDraggedPlaceIsKeptAsAFractionAndStaysOnTheSurface() {
        float[] kept = MicButton.Placement.center(.25f, .5f, WIDTH, HEIGHT, SIZE, MARGIN, 120);
        assertEquals("A moved button ignores the top row", WIDTH * .25f, kept[0], 0);
        assertEquals(HEIGHT * .5f, kept[1], 0);
        float[] edge = MicButton.Placement.center(1, 1, WIDTH, HEIGHT, SIZE, MARGIN, Float.NaN);
        assertEquals(WIDTH - SIZE / 2, edge[0], 0);
        assertEquals(HEIGHT - SIZE / 2, edge[1], 0);
        float[] rotated = MicButton.Placement.center(.25f, .5f, HEIGHT, WIDTH, SIZE, MARGIN, Float.NaN);
        assertEquals("A rotation keeps the same fraction", HEIGHT * .25f, rotated[0], 0);
    }

    @Test public void draggingMovesItWithoutMutingAndTheNewPlaceIsSaved() {
        touch(MotionEvent.ACTION_DOWN, 800, 72);
        touch(MotionEvent.ACTION_MOVE, 600, 300);
        touch(MotionEvent.ACTION_UP, 600, 300);
        assertFalse("A drag is not a tap", host.muted);
        assertEquals(600, centerX(), 1);
        assertEquals(300, centerY(), 1);
        button.setTranslationX(0);
        button.place();
        assertEquals("The saved place comes back", 600, centerX(), 1);
    }

    // Tapping

    @Test public void aTapMutesAndTheNextUnmutes() {
        assertEquals("Mute microphone", button.getContentDescription());
        touch(MotionEvent.ACTION_DOWN, 800, 72);
        touch(MotionEvent.ACTION_UP, 800, 72);
        assertTrue(host.muted);
        assertEquals("Unmute microphone", button.getContentDescription());
        assertTrue("TalkBack's double tap does the same", button.performClick());
        assertFalse(host.muted);
        assertEquals("Mute microphone", button.getContentDescription());
    }

    @Test public void aMuteFromTheMenuOrAControllerShowsOnTheButton() {
        host.muted = true;
        button.refresh();
        assertEquals("Unmute microphone", button.getContentDescription());
    }

    // Showing

    @Test public void itHidesWhenTheHostTakesNoMicrophone() {
        assertEquals(View.VISIBLE, button.getVisibility());
        host.offered = false;
        button.refresh();
        assertEquals(View.GONE, button.getVisibility());
        host.offered = true;
        button.refresh();
        assertEquals("A host that offers it late shows it", View.VISIBLE, button.getVisibility());
    }

    @Test public void itHidesWhenInputIsNotAllowedOrTheSettingIsOff() {
        assertTrue(MicButton.allowed(true, false, true));
        assertFalse("No input permission", MicButton.allowed(true, false, false));
        assertFalse("Setting off", MicButton.allowed(false, false, true));
        assertFalse("Picture in picture", MicButton.allowed(true, true, true));
        button.setSuppressed(!MicButton.allowed(true, false, false));
        assertEquals(View.GONE, button.getVisibility());
        host.offered = true;
        button.refresh();
        assertEquals("Offering does not override the suppression", View.GONE, button.getVisibility());
    }

    private void touch(int action, float x, float y) {
        long now = SystemClock.uptimeMillis();
        // Raw coordinates drive the drag; the local ones only need to land on the button.
        MotionEvent event = MotionEvent.obtain(now, now, action, x, y, 0);
        try {
            button.dispatchTouchEvent(event);
        } finally {
            event.recycle();
        }
    }
}
