package com.limelight;

import android.app.Application;
import android.os.Looper;
import android.view.InputDevice;
import android.view.MotionEvent;
import android.view.View;
import android.widget.FrameLayout;

import com.limelight.ui.AdaptiveLayout;
import com.limelight.ui.DesktopGestures;
import com.limelight.ui.GameTouchControls;
import com.limelight.ui.StreamView;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.Shadows;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import java.util.concurrent.TimeUnit;

import static com.limelight.GameTouchInputTest.game;
import static com.limelight.GameTouchInputTest.motion;
import static com.limelight.GameTouchInputTest.touchControls;
import static org.junit.Assert.*;

/**
 * Verifies that expanded Safe Fit / cutout / hinge touch surfaces keep backgroundTouchView and
 * GameTouchControls on a single unsplit multi-pointer coordinate space under symmetric and
 * asymmetric paddings.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class,
        shadows = PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class GameSafeFitTouchTest {
    @Test
    public void safeFitAndAsymmetricCutoutMarginsStayUnsplitForPinchNativeTouchAndGameControls() {
        GameTouchInputTest.RecordingConnection connection = new GameTouchInputTest.RecordingConnection();
        Game game = game(connection);
        game.setTheme(R.style.StreamTheme);
        game.requestWindowFeature(android.view.Window.FEATURE_NO_TITLE);
        game.setContentView(R.layout.activity_game);
        FrameLayout content = game.findViewById(android.R.id.content);
        content.setPadding(100, 50, 100, 50);
        content.setClipToPadding(false);
        content.setClipChildren(false);
        StreamView stream = game.findViewById(R.id.surfaceView);
        View background = game.findViewById(R.id.backgroundTouchView);
        background.setOnTouchListener(game);
        AdaptiveLayout.expandTouchSurface(content, background);
        ReflectionHelpers.setField(game, "streamView", stream);
        View decor = game.getWindow().getDecorView();
        decor.measure(View.MeasureSpec.makeMeasureSpec(1000, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(500, View.MeasureSpec.EXACTLY));
        decor.layout(0, 0, 1000, 500);
        assertEquals(0, background.getLeft());
        assertEquals(0, background.getTop());
        assertEquals(1000, background.getRight());
        assertEquals(500, background.getBottom());
        ReflectionHelpers.callInstanceMethod(game.touchInput, "initializeTouchContexts");

        // Shifted into the left Safe Fit padding via Picture Position: touch at x=60 hits backgroundTouchView.
        stream.setTranslationX(-80);
        assertEquals(20f, stream.getX(), 0.01f);
        dispatchActivityTouch(game, MotionEvent.ACTION_DOWN, 60, 250);
        dispatchActivityTouch(game, MotionEvent.ACTION_UP, 60, 250);
        assertEquals("shifted streamView touch delivered once", 1, connection.positions.size());
        assertArrayEquals(new int[] {40, 200, 800, 400}, connection.positions.get(0));
        Shadows.shadowOf(Looper.getMainLooper()).idleFor(100, TimeUnit.MILLISECONDS);

        // Zoomed into the left Safe Fit padding: touch at x=50 arrives on backgroundTouchView without splitting.
        connection.positions.clear();
        stream.setTranslationX(0);
        DesktopGestures desktop = new DesktopGestures(game, stream, () -> { }, () -> { });
        desktop.setEnabled(true);
        desktop.viewport().resize(800, 400);
        desktop.viewport().zoom(2f, 400, 200);
        ReflectionHelpers.setField(game, "desktopGestures", desktop);
        dispatchActivityTouch(game, MotionEvent.ACTION_DOWN, 50, 250);
        dispatchActivityTouch(game, MotionEvent.ACTION_UP, 50, 250);
        assertEquals("Safe Fit edge touch forwarded once", 1, connection.positions.size());
        assertArrayEquals(new int[] {175, 200, 800, 400}, connection.positions.get(0));
        Shadows.shadowOf(Looper.getMainLooper()).idleFor(100, TimeUnit.MILLISECONDS);

        // Multi-finger pinch straddling streamView and Safe Fit padding stays unsplit on one view.
        float scaleBefore = desktop.viewport().scale();
        MotionEvent pinchDown0 = motion(MotionEvent.ACTION_DOWN, 0, new int[] {0}, 1500, 0, new float[] {50, 250});
        MotionEvent pinchDown1 = motion(MotionEvent.ACTION_POINTER_DOWN, 1, new int[] {0, 1}, 1510, 0,
                new float[] {50, 250}, new float[] {450, 250});
        MotionEvent pinchMove1 = motion(MotionEvent.ACTION_MOVE, 0, new int[] {0, 1}, 1520, 0,
                new float[] {30, 250}, new float[] {530, 250});
        MotionEvent pinchMove2 = motion(MotionEvent.ACTION_MOVE, 0, new int[] {0, 1}, 1530, 0,
                new float[] {10, 250}, new float[] {650, 250});
        MotionEvent pinchUp1 = motion(MotionEvent.ACTION_POINTER_UP, 1, new int[] {0, 1}, 1540, 0,
                new float[] {10, 250}, new float[] {650, 250});
        MotionEvent pinchUp0 = motion(MotionEvent.ACTION_UP, 0, new int[] {0}, 1550, 0, new float[] {10, 250});
        try {
            assertTrue(game.dispatchTouchEvent(pinchDown0));
            assertTrue(game.dispatchTouchEvent(pinchDown1));
            assertTrue(game.dispatchTouchEvent(pinchMove1));
            assertTrue(game.dispatchTouchEvent(pinchMove2));
            assertTrue("pinch across video and Safe Fit margin updates DesktopGestures zoom",
                    desktop.viewport().scale() > scaleBefore);
            game.dispatchTouchEvent(pinchUp1);
            game.dispatchTouchEvent(pinchUp0);
        } finally {
            pinchDown0.recycle();
            pinchDown1.recycle();
            pinchMove1.recycle();
            pinchMove2.recycle();
            pinchUp1.recycle();
            pinchUp0.recycle();
        }

        // Native multi-touch straddling video and Safe Fit padding: lifting finger 0 first still tracks and lifts finger 1 cleanly.
        ReflectionHelpers.setField(game, "desktopGestures", null);
        ReflectionHelpers.setField(game, "nativeTouchEnabled", true);
        connection.touches.clear();
        stream.setTranslationX(-80);
        MotionEvent down0 = motion(MotionEvent.ACTION_DOWN, 0, new int[] {0}, 2000, 0, new float[] {60, 250});
        MotionEvent down1 = motion(MotionEvent.ACTION_POINTER_DOWN, 1, new int[] {0, 1}, 2010, 0,
                new float[] {60, 250}, new float[] {860, 250});
        MotionEvent moveBoth = motion(MotionEvent.ACTION_MOVE, 0, new int[] {0, 1}, 2020, 0,
                new float[] {70, 250}, new float[] {870, 250});
        MotionEvent up0First = motion(MotionEvent.ACTION_POINTER_UP, 0, new int[] {0, 1}, 2030, 0,
                new float[] {70, 250}, new float[] {870, 250});
        MotionEvent move1After = motion(MotionEvent.ACTION_MOVE, 0, new int[] {1}, 2040, 0, new float[] {880, 250});
        MotionEvent up1Last = motion(MotionEvent.ACTION_UP, 0, new int[] {1}, 2050, 0, new float[] {880, 250});
        try {
            assertTrue(game.dispatchTouchEvent(down0));
            assertTrue(game.dispatchTouchEvent(down1));
            assertEquals("both pointers tracked", 2, connection.heldTouches.size());
            connection.touches.clear();
            assertTrue(game.dispatchTouchEvent(moveBoth));
            assertEquals("both pointers move", 2, connection.touches.size());
            assertTrue(game.dispatchTouchEvent(up0First));
            assertEquals("finger 1 stays down after lifting finger 0", 1, connection.heldTouches.size());
            connection.touches.clear();
            assertTrue(game.dispatchTouchEvent(move1After));
            assertEquals("finger 1 keeps moving", 1, connection.touches.size());
            assertTrue(game.dispatchTouchEvent(up1Last));
            assertTrue("lifting finger 1 leaves zero stuck host touches", connection.heldTouches.isEmpty());
        } finally {
            down0.recycle();
            down1.recycle();
            moveBoth.recycle();
            up0First.recycle();
            move1After.recycle();
            up1Last.recycle();
        }
        ReflectionHelpers.setField(game, "nativeTouchEnabled", false);
        stream.setTranslationX(0);

        // Asymmetric cutout/Safe Fit padding (120, 36, 0, 64) with GameTouchControls visible:
        // both backgroundTouchView and GameTouchControls must span [0, 0, 1000, 500] with zero dead edge.
        content.setPadding(120, 36, 0, 64);
        AdaptiveLayout.expandTouchSurface(content, background);
        GameTouchControls controls = touchControls(game);
        content.addView(controls, new FrameLayout.LayoutParams(
                FrameLayout.LayoutParams.MATCH_PARENT, FrameLayout.LayoutParams.MATCH_PARENT));
        AdaptiveLayout.expandTouchSurface(content, controls);
        controls.setSafeInsets(120, 36, 0, 64);
        decor.measure(View.MeasureSpec.makeMeasureSpec(1000, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(500, View.MeasureSpec.EXACTLY));
        decor.layout(0, 0, 1000, 500);
        assertEquals(0, background.getLeft());
        assertEquals(0, background.getTop());
        assertEquals(1000, background.getRight());
        assertEquals(500, background.getBottom());
        assertEquals(0, controls.getLeft());
        assertEquals(0, controls.getTop());
        assertEquals(1000, controls.getRight());
        assertEquals(500, controls.getBottom());
        connection.positions.clear();
        dispatchActivityTouch(game, MotionEvent.ACTION_DOWN, 980, 250);
        dispatchActivityTouch(game, MotionEvent.ACTION_UP, 980, 250);
        assertEquals("opposite edge under asymmetric cutout receives touch", 1, connection.positions.size());
        Shadows.shadowOf(Looper.getMainLooper()).idleFor(100, TimeUnit.MILLISECONDS);
        desktop.viewport().resize(stream.getWidth(), stream.getHeight());
        desktop.viewport().reset();
        desktop.viewport().zoom(2f, stream.getWidth() / 2f, stream.getHeight() / 2f);
        ReflectionHelpers.setField(game, "desktopGestures", desktop);
        float controlsScaleBefore = desktop.viewport().scale();
        MotionEvent cDown0 = motion(MotionEvent.ACTION_DOWN, 0, new int[] {0}, 2500, 0, new float[] {50, 250});
        MotionEvent cDown1 = motion(MotionEvent.ACTION_POINTER_DOWN, 1, new int[] {0, 1}, 2510, 0,
                new float[] {50, 250}, new float[] {650, 250});
        MotionEvent cMove1 = motion(MotionEvent.ACTION_MOVE, 0, new int[] {0, 1}, 2520, 0,
                new float[] {30, 250}, new float[] {730, 250});
        MotionEvent cMove2 = motion(MotionEvent.ACTION_MOVE, 0, new int[] {0, 1}, 2530, 0,
                new float[] {10, 250}, new float[] {850, 250});
        MotionEvent cUp1 = motion(MotionEvent.ACTION_POINTER_UP, 1, new int[] {0, 1}, 2540, 0,
                new float[] {10, 250}, new float[] {850, 250});
        MotionEvent cUp0 = motion(MotionEvent.ACTION_UP, 0, new int[] {0}, 2550, 0, new float[] {10, 250});
        try {
            assertTrue(game.dispatchTouchEvent(cDown0));
            assertTrue(game.dispatchTouchEvent(cDown1));
            assertTrue(game.dispatchTouchEvent(cMove1));
            assertTrue(game.dispatchTouchEvent(cMove2));
            assertTrue("pinch across Safe Fit margin and video works with GameTouchControls visible",
                    desktop.viewport().scale() > controlsScaleBefore);
            game.dispatchTouchEvent(cUp1);
            game.dispatchTouchEvent(cUp0);
        } finally {
            cDown0.recycle();
            cDown1.recycle();
            cMove1.recycle();
            cMove2.recycle();
            cUp1.recycle();
            cUp0.recycle();
        }
        // Half-opened foldable hinge (paddingLeft = 500, paddingRight = 0): full [0, 0, 1000, 500] stays live.
        content.setPadding(500, 0, 0, 0);
        AdaptiveLayout.expandTouchSurface(content, background);
        AdaptiveLayout.expandTouchSurface(content, controls);
        decor.measure(View.MeasureSpec.makeMeasureSpec(1000, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(500, View.MeasureSpec.EXACTLY));
        decor.layout(0, 0, 1000, 500);
        assertEquals(0, background.getLeft());
        assertEquals(1000, background.getRight());
        assertEquals(0, controls.getLeft());
        assertEquals(1000, controls.getRight());
        content.removeView(controls);
        ReflectionHelpers.setField(game, "gameTouchControls", null);
        ReflectionHelpers.setField(game, "desktopGestures", null);

        // Ungrabbed touches are rejected once cleanly.
        ReflectionHelpers.setField(game, "grabbedInput", false);
        assertFalse(dispatchActivityTouch(game, MotionEvent.ACTION_DOWN, 500, 250));
    }

    private static boolean dispatchActivityTouch(Game game, int action, float x, float y) {
        MotionEvent event = MotionEvent.obtain(1000, 1010, action, x, y, 0);
        event.setSource(InputDevice.SOURCE_TOUCHSCREEN);
        try {
            return game.dispatchTouchEvent(event);
        } finally {
            event.recycle();
        }
    }
}
