package com.limelight.ui;

import android.app.Application;
import android.view.InputDevice;
import android.view.MotionEvent;

import com.limelight.nvstream.http.NvApp;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import java.util.List;
import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.Assert.*;

/**
 * Tests {@link DesktopGestures} passive pinch-to-zoom alongside two-finger scroll-through at 1.0x
 * (#189) and menu Zoom reset visibility when zoomed without explicit Zoom mode.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class DesktopGesturesTest {
    private static MotionEvent fingerEvent(int action, int actionIndex, int[] ids, long time, float[]... points) {
        MotionEvent.PointerProperties[] props = new MotionEvent.PointerProperties[points.length];
        MotionEvent.PointerCoords[] coords = new MotionEvent.PointerCoords[points.length];
        for (int i = 0; i < points.length; i++) {
            props[i] = new MotionEvent.PointerProperties();
            props[i].id = ids[i];
            props[i].toolType = MotionEvent.TOOL_TYPE_FINGER;
            coords[i] = new MotionEvent.PointerCoords();
            coords[i].x = points[i][0];
            coords[i].y = points[i][1];
        }
        int masked = action == MotionEvent.ACTION_POINTER_DOWN || action == MotionEvent.ACTION_POINTER_UP
                ? action | (actionIndex << MotionEvent.ACTION_POINTER_INDEX_SHIFT)
                : action;
        return MotionEvent.obtain(1000L, time, masked, points.length, props, coords, 0, 0, 1f, 1f,
                0, 0, InputDevice.SOURCE_TOUCHSCREEN, 0);
    }

    private static boolean send(DesktopGestures gestures, boolean allowRightClick, boolean allowScrollThrough,
                                int action, int actionIndex, int[] ids, long time, float[]... points) {
        MotionEvent event = fingerEvent(action, actionIndex, ids, time, points);
        try {
            return gestures.onTouch(event, allowRightClick, allowScrollThrough);
        } finally {
            event.recycle();
        }
    }

    private static DesktopGestures newGestures(AtomicInteger cancels, AtomicInteger rightClicks) {
        Application app = RuntimeEnvironment.getApplication();
        StreamView view = new StreamView(app);
        view.initializeSurface(false);
        view.layout(0, 0, 800, 400);
        DesktopGestures gestures = new DesktopGestures(app, view, cancels::incrementAndGet, rightClicks::incrementAndGet);
        gestures.viewport().resize(800, 400);
        return gestures;
    }

    @Test
    public void passiveTwoFingerDragAtOneXPassesThroughForScrollingWithoutCancelingHostInput() {
        AtomicInteger cancels = new AtomicInteger();
        AtomicInteger rightClicks = new AtomicInteger();
        DesktopGestures gestures = newGestures(cancels, rightClicks);
        int[] one = {0};
        int[] two = {0, 1};

        assertFalse(send(gestures, true, true, MotionEvent.ACTION_DOWN, 0, one, 1000, new float[] {200, 150}));
        assertFalse(send(gestures, true, true, MotionEvent.ACTION_POINTER_DOWN, 1, two, 1010,
                new float[] {200, 150}, new float[] {400, 150}));
        assertFalse(send(gestures, true, true, MotionEvent.ACTION_MOVE, 0, two, 1020,
                new float[] {200, 260}, new float[] {400, 260}));
        assertFalse(send(gestures, true, true, MotionEvent.ACTION_POINTER_UP, 1, two, 1030,
                new float[] {200, 260}, new float[] {400, 260}));
        assertFalse(send(gestures, true, true, MotionEvent.ACTION_UP, 0, one, 1040, new float[] {200, 260}));

        assertEquals(0, cancels.get());
        assertEquals(0, rightClicks.get());
        assertEquals(1.0f, gestures.viewport().scale(), 0f);
    }

    @Test
    public void passivePinchPastThresholdZoomsPansAndReturnsToScrollingAtOneX() {
        AtomicInteger cancels = new AtomicInteger();
        AtomicInteger rightClicks = new AtomicInteger();
        DesktopGestures gestures = newGestures(cancels, rightClicks);
        int[] one = {0};
        int[] two = {0, 1};

        // Small divergence below 28 dp keeps scrolling instead of hijacking input.
        assertFalse(send(gestures, true, true, MotionEvent.ACTION_DOWN, 0, one, 1000, new float[] {300, 200}));
        assertFalse(send(gestures, true, true, MotionEvent.ACTION_POINTER_DOWN, 1, two, 1010,
                new float[] {300, 200}, new float[] {500, 200}));
        assertFalse(send(gestures, true, true, MotionEvent.ACTION_MOVE, 0, two, 1020,
                new float[] {292, 200}, new float[] {508, 200}));
        assertEquals(0, cancels.get());
        assertEquals(1.0f, gestures.viewport().scale(), 0f);

        // Diverging >= 28 dp latches consuming, cancels host input once, and zooms in.
        assertTrue(send(gestures, true, true, MotionEvent.ACTION_MOVE, 0, two, 1030,
                new float[] {200, 200}, new float[] {600, 200}));
        assertEquals(1, cancels.get());
        assertTrue(send(gestures, true, true, MotionEvent.ACTION_MOVE, 0, two, 1040,
                new float[] {100, 200}, new float[] {700, 200}));
        float zoomedScale = gestures.viewport().scale();
        assertTrue("expected zoom > 1.0, got " + zoomedScale, zoomedScale > 1.0f);
        assertTrue(send(gestures, true, true, MotionEvent.ACTION_POINTER_UP, 1, two, 1050,
                new float[] {100, 200}, new float[] {700, 200}));
        assertTrue(send(gestures, true, true, MotionEvent.ACTION_UP, 0, one, 1060, new float[] {100, 200}));

        // While zoomed > 1.0x without Zoom toggled on, two-finger drag pans (cancels once on pointer down).
        assertFalse(send(gestures, true, true, MotionEvent.ACTION_DOWN, 0, one, 2000, new float[] {100, 150}));
        assertTrue(send(gestures, true, true, MotionEvent.ACTION_POINTER_DOWN, 1, two, 2010,
                new float[] {100, 150}, new float[] {700, 150}));
        assertEquals(2, cancels.get());
        assertTrue(send(gestures, true, true, MotionEvent.ACTION_MOVE, 0, two, 2080,
                new float[] {50, 110}, new float[] {650, 110}));
        assertTrue(gestures.viewport().x() < 0f);
        assertTrue(gestures.viewport().y() < 0f);

        // Pinching back down to 1.0x ends consumed gesture, then subsequent 2-finger drags scroll again.
        assertTrue(send(gestures, true, true, MotionEvent.ACTION_MOVE, 0, two, 2100,
                new float[] {150, 110}, new float[] {550, 110}));
        assertTrue(send(gestures, true, true, MotionEvent.ACTION_MOVE, 0, two, 2120,
                new float[] {250, 110}, new float[] {450, 110}));
        assertEquals(1.0f, gestures.viewport().scale(), 0.001f);
        assertTrue(send(gestures, true, true, MotionEvent.ACTION_POINTER_UP, 1, two, 2130,
                new float[] {250, 110}, new float[] {450, 110}));
        assertTrue(send(gestures, true, true, MotionEvent.ACTION_UP, 0, one, 2140, new float[] {250, 110}));

        assertFalse(send(gestures, true, true, MotionEvent.ACTION_DOWN, 0, one, 3000, new float[] {200, 150}));
        assertFalse(send(gestures, true, true, MotionEvent.ACTION_POINTER_DOWN, 1, two, 3010,
                new float[] {200, 150}, new float[] {400, 150}));
        assertFalse(send(gestures, true, true, MotionEvent.ACTION_MOVE, 0, two, 3020,
                new float[] {200, 260}, new float[] {400, 260}));
        assertEquals(2, cancels.get());
        assertEquals(0, rightClicks.get());
    }

    @Test
    public void zoomedStreamMenuShowsZoomResetEvenWhenZoomToggleIsOff() {
        StreamMenu.Options options = StreamMenuTestSupport.options("rubylight-serverinfo.xml", NvApp.Role.STREAM);
        options.localOutput = true;
        options.zoomEnabled = false;
        options.zoomed = false;
        assertFalse(StreamMenuTestSupport.items(StreamMenu.mainPage(options)).contains(StreamMenu.Item.ZOOM_RESET));

        options.zoomed = true;
        List<StreamMenu.Item> items = StreamMenuTestSupport.items(StreamMenu.mainPage(options));
        assertTrue(items.contains(StreamMenu.Item.ZOOM_RESET));
        assertEquals(Boolean.FALSE, StreamMenuTestSupport.entry(StreamMenu.mainPage(options), StreamMenu.Item.ZOOM).checked);
    }

    @Test
    public void passivePinchOutBelowOneXZoomsPansAndReturnsToScrollingAtOneX() {
        AtomicInteger cancels = new AtomicInteger();
        AtomicInteger rightClicks = new AtomicInteger();
        DesktopGestures gestures = newGestures(cancels, rightClicks);
        int[] one = {0};
        int[] two = {0, 1};

        assertFalse(send(gestures, true, true, MotionEvent.ACTION_DOWN, 0, one, 1000, new float[] {100, 200}));
        assertFalse(send(gestures, true, true, MotionEvent.ACTION_POINTER_DOWN, 1, two, 1010,
                new float[] {100, 200}, new float[] {700, 200}));
        assertTrue(send(gestures, true, true, MotionEvent.ACTION_MOVE, 0, two, 1020,
                new float[] {250, 200}, new float[] {550, 200}));
        assertTrue(send(gestures, true, true, MotionEvent.ACTION_MOVE, 0, two, 1030,
                new float[] {300, 200}, new float[] {500, 200}));
        float zoomedOut = gestures.viewport().scale();
        assertTrue("expected zoom < 1.0, got " + zoomedOut, zoomedOut < 1.0f && zoomedOut >= DesktopViewport.MIN_SCALE);
        assertTrue(send(gestures, true, true, MotionEvent.ACTION_POINTER_UP, 1, two, 1040,
                new float[] {300, 200}, new float[] {500, 200}));
        assertTrue(send(gestures, true, true, MotionEvent.ACTION_UP, 0, one, 1050, new float[] {300, 200}));

        // Panning while zoomed out moves freely.
        assertFalse(send(gestures, true, true, MotionEvent.ACTION_DOWN, 0, one, 2000, new float[] {200, 200}));
        assertTrue(send(gestures, true, true, MotionEvent.ACTION_POINTER_DOWN, 1, two, 2010,
                new float[] {200, 200}, new float[] {400, 200}));
        assertTrue(send(gestures, true, true, MotionEvent.ACTION_MOVE, 0, two, 2020,
                new float[] {160, 180}, new float[] {360, 180}));
        assertTrue(gestures.viewport().isTransformed());

        // Pinching back out to 1.0x resets and returns to scrolling.
        assertTrue(send(gestures, true, true, MotionEvent.ACTION_MOVE, 0, two, 2030,
                new float[] {60, 180}, new float[] {460, 180}));
        assertTrue(send(gestures, true, true, MotionEvent.ACTION_MOVE, 0, two, 2040,
                new float[] {10, 180}, new float[] {610, 180}));
        assertFalse(gestures.viewport().isTransformed());
        assertEquals(1.0f, gestures.viewport().scale(), 0.001f);
        assertTrue(send(gestures, true, true, MotionEvent.ACTION_POINTER_UP, 1, two, 2050,
                new float[] {10, 180}, new float[] {610, 180}));
        assertTrue(send(gestures, true, true, MotionEvent.ACTION_UP, 0, one, 2060, new float[] {10, 180}));

        assertFalse(send(gestures, true, true, MotionEvent.ACTION_DOWN, 0, one, 3000, new float[] {200, 150}));
        assertFalse(send(gestures, true, true, MotionEvent.ACTION_POINTER_DOWN, 1, two, 3010,
                new float[] {200, 150}, new float[] {400, 150}));
        assertFalse(send(gestures, true, true, MotionEvent.ACTION_MOVE, 0, two, 3020,
                new float[] {200, 260}, new float[] {400, 260}));
    }

    @Test
    public void explicitZoomAllowsFreeOneXDraggingAndContinuousZoomFromHalfToFourX() {
        AtomicInteger cancels = new AtomicInteger();
        AtomicInteger rightClicks = new AtomicInteger();
        DesktopGestures gestures = newGestures(cancels, rightClicks);
        gestures.setEnabled(true);
        int[] one = {0};
        int[] two = {0, 1};

        // Drag immediately at 1.0x to pull top-right inward.
        assertFalse(send(gestures, true, false, MotionEvent.ACTION_DOWN, 0, one, 1000, new float[] {300, 100}));
        assertTrue(send(gestures, true, false, MotionEvent.ACTION_POINTER_DOWN, 1, two, 1010,
                new float[] {300, 100}, new float[] {700, 100}));
        assertTrue(send(gestures, true, false, MotionEvent.ACTION_MOVE, 0, two, 1080,
                new float[] {150, 250}, new float[] {550, 250}));
        assertEquals(-150f, gestures.viewport().x(), 0.1f);
        assertEquals(150f, gestures.viewport().y(), 0.1f);
        assertEquals(1.0f, gestures.viewport().scale(), 0.001f);

        // Pinch in and out crosses 1.0x continuously without snapping back to 0 pan.
        assertTrue(send(gestures, true, false, MotionEvent.ACTION_MOVE, 0, two, 1100,
                new float[] {200, 250}, new float[] {500, 250}));
        assertTrue(send(gestures, true, false, MotionEvent.ACTION_MOVE, 0, two, 1120,
                new float[] {250, 250}, new float[] {450, 250}));
        float zoomedOut = gestures.viewport().scale();
        assertTrue("expected < 1.0, got " + zoomedOut,
                zoomedOut < 1.0f && zoomedOut >= DesktopViewport.MIN_SCALE);
        assertTrue(send(gestures, true, false, MotionEvent.ACTION_MOVE, 0, two, 1140,
                new float[] {100, 250}, new float[] {600, 250}));
        assertTrue("expected > 1.0, got " + gestures.viewport().scale(), gestures.viewport().scale() > 1.0f);
        assertTrue("expected pan preserved across 1.0x", gestures.viewport().y() != 0f);
    }

    @Test
    public void streamViewUnclippedAndSyncsContainerBoundsAcrossBlackBorders() {
        Application app = RuntimeEnvironment.getApplication();
        android.widget.FrameLayout parent = new android.widget.FrameLayout(app);
        StreamView view = new StreamView(app);
        view.initializeSurface(false);
        assertFalse(view.getClipChildren());
        assertFalse(view.getClipToPadding());
        parent.addView(view);
        parent.layout(0, 0, 2400, 1080);
        view.layout(240, 0, 2160, 1080);

        DesktopGestures gestures = new DesktopGestures(app, view, () -> { }, () -> { });
        gestures.setEnabled(true);
        int[] one = {0};
        int[] two = {0, 1};
        assertFalse(send(gestures, true, false, MotionEvent.ACTION_DOWN, 0, one, 1000, new float[] {400, 300}));
        assertTrue(send(gestures, true, false, MotionEvent.ACTION_POINTER_DOWN, 1, two, 1010,
                new float[] {400, 300}, new float[] {800, 300}));
        assertTrue(send(gestures, true, false, MotionEvent.ACTION_MOVE, 0, two, 1080,
                new float[] {160, 300}, new float[] {560, 300}));
        assertEquals(-240f, gestures.viewport().x(), 0.1f);
        assertEquals(-240f, view.getChildAt(0).getTranslationX(), 0.1f);
    }

    @Test
    public void passivePinchCrossingThresholdOnBeginDoesNotStrandPanAndReversingSubOnePercentResets() {
        DesktopGestures gestures = newGestures(new AtomicInteger(), new AtomicInteger());
        int[] one = {0}, two = {0, 1};

        assertFalse(send(gestures, true, true, MotionEvent.ACTION_DOWN, 0, one, 1000, new float[] {250, 200}));
        assertFalse(send(gestures, true, true, MotionEvent.ACTION_POINTER_DOWN, 1, two, 1010,
                new float[] {250, 200}, new float[] {550, 200}));
        assertTrue(send(gestures, true, true, MotionEvent.ACTION_MOVE, 0, two, 1020,
                new float[] {220, 200}, new float[] {580, 200}));
        assertTrue(send(gestures, true, true, MotionEvent.ACTION_MOVE, 0, two, 1030,
                new float[] {180, 220}, new float[] {540, 220}));
        assertTrue(send(gestures, true, true, MotionEvent.ACTION_POINTER_UP, 1, two, 1040,
                new float[] {180, 220}, new float[] {540, 220}));
        assertTrue(send(gestures, true, true, MotionEvent.ACTION_UP, 0, one, 1050, new float[] {180, 220}));
        assertFalse(gestures.viewport().isTransformed());

        assertFalse(send(gestures, true, true, MotionEvent.ACTION_DOWN, 0, one, 2000, new float[] {-600, 200}));
        assertFalse(send(gestures, true, true, MotionEvent.ACTION_POINTER_DOWN, 1, two, 2010,
                new float[] {-600, 200}, new float[] {1400, 200}));
        assertTrue(send(gestures, true, true, MotionEvent.ACTION_MOVE, 0, two, 2020,
                new float[] {-620, 200}, new float[] {1420, 200}));
        assertTrue(send(gestures, true, true, MotionEvent.ACTION_MOVE, 0, two, 2030,
                new float[] {-628, 200}, new float[] {1428, 200}));
        assertTrue(gestures.viewport().scale() > 1.0f && gestures.viewport().scale() <= 1.01f);
        assertTrue(send(gestures, true, true, MotionEvent.ACTION_MOVE, 0, two, 2040,
                new float[] {-575, 200}, new float[] {1375, 200}));
        assertEquals(1.0f, gestures.viewport().scale(), 0.001f);
        assertFalse(gestures.viewport().isTransformed());
    }

    @Test
    public void onTransformChangedFiresWhenParentPaddingOrLayoutChangesSyncBounds() {
        Application app = RuntimeEnvironment.getApplication();
        android.widget.FrameLayout parent = new android.widget.FrameLayout(app);
        StreamView view = new StreamView(app);
        view.initializeSurface(false);
        parent.addView(view, new android.widget.FrameLayout.LayoutParams(1920, 1080));
        parent.measure(android.view.View.MeasureSpec.makeMeasureSpec(2400, android.view.View.MeasureSpec.EXACTLY),
                android.view.View.MeasureSpec.makeMeasureSpec(1080, android.view.View.MeasureSpec.EXACTLY));
        parent.layout(0, 0, 2400, 1080);

        AtomicInteger syncs = new AtomicInteger();
        DesktopGestures gestures = new DesktopGestures(app, view, () -> { }, () -> { });
        gestures.setOnTransformChanged(syncs::incrementAndGet);
        parent.setPadding(100, 0, 100, 0);
        parent.measure(android.view.View.MeasureSpec.makeMeasureSpec(2400, android.view.View.MeasureSpec.EXACTLY),
                android.view.View.MeasureSpec.makeMeasureSpec(1200, android.view.View.MeasureSpec.EXACTLY));
        parent.layout(0, 0, 2400, 1200);
        assertTrue(syncs.get() > 0);
    }
}
