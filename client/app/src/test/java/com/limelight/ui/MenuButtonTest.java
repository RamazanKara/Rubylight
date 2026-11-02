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

/** The stream's floating menu button: where it sits, that a tap opens the menu and a drag only moves it. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 34, qualifiers = "en-w800dp-h360dp-land-xhdpi", application = Application.class)
public class MenuButtonTest {
    private static final int WIDTH = 1600, HEIGHT = 720;
    private static final float SIZE = 96, MARGIN = 24; // 48 dp and 12 dp at xhdpi

    private static final class FakeHost implements MenuButton.Host {
        int opened;
        float rowBottom = Float.NaN;
        @Override public void open() { opened++; }
        @Override public float topRowBottom() { return rowBottom; }
    }

    private final FakeHost host = new FakeHost();
    private FrameLayout frame;
    private MenuButton button;

    @Before
    public void setUp() {
        RuntimeEnvironment.getApplication().getSharedPreferences(MenuButton.PREFS, 0).edit().clear().commit();
        UiNavigationTest.Screen screen = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup().visible().get();
        frame = new FrameLayout(screen);
        screen.setContentView(frame, new FrameLayout.LayoutParams(WIDTH, HEIGHT));
        button = new MenuButton(screen, host);
        frame.addView(button, button.layoutParams());
        button.setShown(true);
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

    private void touch(int action, float x, float y) {
        long now = SystemClock.uptimeMillis();
        // Raw coordinates drive the drag, as in the microphone button's test.
        MotionEvent event = MotionEvent.obtain(now, now, action, x, y, 0);
        try {
            button.dispatchTouchEvent(event);
        } finally {
            event.recycle();
        }
    }

    /** A TV got the button too, with no touch screen to tap it and no switch in Settings to turn it off. */
    @Test public void onlyATouchDeviceThatIsNoTelevisionGetsTheButton() {
        android.content.Context context = RuntimeEnvironment.getApplication();
        org.robolectric.Shadows.shadowOf(context.getPackageManager()).setSystemFeature(android.content.pm.PackageManager.FEATURE_TOUCHSCREEN, true);
        assertTrue(MenuButton.offered(context));
        org.robolectric.Shadows.shadowOf(context.getPackageManager()).setSystemFeature(android.content.pm.PackageManager.FEATURE_TOUCHSCREEN, false);
        assertFalse("no touch screen, no switch in Settings", MenuButton.offered(context));
    }

    @Test @Config(qualifiers = "television")
    public void aTelevisionGetsNoButton() {
        android.content.Context context = RuntimeEnvironment.getApplication();
        org.robolectric.Shadows.shadowOf(context.getPackageManager()).setSystemFeature(android.content.pm.PackageManager.FEATURE_TOUCHSCREEN, true);
        assertFalse(MenuButton.offered(context));
    }

    @Test public void itIsOffUntilItsSettingTurnsItOn() {
        MenuButton fresh = new MenuButton(frame.getContext(), host);
        assertEquals(View.GONE, fresh.getVisibility());
        assertEquals(View.VISIBLE, button.getVisibility());
        button.setShown(false);
        assertEquals(View.GONE, button.getVisibility());
    }

    @Test public void byDefaultItSitsBesideTheMicrophoneButtonAtTheTop() {
        assertEquals(WIDTH / 2f + SIZE + MARGIN, centerX(), 1);
        assertEquals(MARGIN + SIZE / 2, centerY(), 1);
        host.rowBottom = 120;
        button.place();
        assertEquals("Below the View, Home and Menu row", 120 + MARGIN + SIZE / 2, centerY(), 1);
    }

    @Test public void aTapOpensTheMenu() {
        touch(MotionEvent.ACTION_DOWN, centerX(), centerY());
        touch(MotionEvent.ACTION_UP, centerX(), centerY());
        assertEquals(1, host.opened);
    }

    /** On the Retroid a dragged button stayed undrawn at its new place until it was hidden and shown again. */
    @Test public void aDragEndLaysTheButtonOutAgainSoItIsDrawnWhereItWasLeft() {
        touch(MotionEvent.ACTION_DOWN, centerX(), centerY());
        touch(MotionEvent.ACTION_MOVE, 300, 500);
        assertFalse(button.isLayoutRequested());
        touch(MotionEvent.ACTION_UP, 300, 500);
        assertTrue("the drag's end asks for a layout and a frame", button.isLayoutRequested());
        layout();
        assertEquals(300, centerX(), 1);
        assertEquals(500, centerY(), 1);
    }

    @Test public void draggingMovesItWithoutOpeningTheMenuAndTheNewPlaceIsSaved() {
        float x = centerX(), y = centerY();
        touch(MotionEvent.ACTION_DOWN, x, y);
        touch(MotionEvent.ACTION_MOVE, 300, 500);
        touch(MotionEvent.ACTION_UP, 300, 500);
        assertEquals("A drag is not a tap", 0, host.opened);
        assertEquals(300, centerX(), 1);
        assertEquals(500, centerY(), 1);
        float[] kept = MenuButton.Placement.center(
                RuntimeEnvironment.getApplication().getSharedPreferences(MenuButton.PREFS, 0).getFloat(MenuButton.KEY_X, Float.NaN),
                RuntimeEnvironment.getApplication().getSharedPreferences(MenuButton.PREFS, 0).getFloat(MenuButton.KEY_Y, Float.NaN),
                WIDTH, HEIGHT, SIZE, MARGIN, Float.NaN);
        assertEquals("The next session puts it back there", 300, kept[0], 1);
        assertEquals(500, kept[1], 1);
    }
}
