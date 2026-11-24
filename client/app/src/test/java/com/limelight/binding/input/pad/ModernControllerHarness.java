package com.limelight.binding.input.pad;

import android.content.Context;
import android.view.MotionEvent;
import android.view.View;
import android.widget.FrameLayout;

import com.limelight.binding.input.HeldInputState;
import com.limelight.preferences.PreferenceConfiguration;
import com.limelight.ui.UiNavigationTest;

import org.junit.After;
import org.junit.Before;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.shadows.ShadowLooper;

import java.util.ArrayList;
import java.util.List;

import static org.junit.Assert.*;

/**
 * Shared Robolectric fixture and gesture helpers for the {@link ModernController} test suites.
 */
abstract class ModernControllerHarness {
    static final String GAME = "pc-1/app-7";
    final List<int[]> reports = new ArrayList<>();
    /** Keys from the WASD stick, as "+W" and "-W"; only the WASD stick sends any. */
    final List<String> keys = new ArrayList<>();
    final HeldInputState.Sink sink = new HeldInputState.Sink() {
        @Override public void key(int key, boolean down) { keys.add((down ? "+" : "-") + (char) key); }
        @Override public void mouse(int button, boolean down) { fail("Gamepad only"); }
        @Override public void gamepad(int buttons, short lx, short ly, short rx, short ry, byte lt, byte rt) {
            reports.add(new int[] {buttons, lx, ly, rx, ry, lt & 0xFF, rt & 0xFF});
        }
    };
    FrameLayout frame;
    PreferenceConfiguration prefs;
    /** The pointer touchpad's output, recorded by the tests. */
    final List<int[]> pointerMoves = new ArrayList<>();
    final List<Integer> pointerButtons = new ArrayList<>();
    final List<int[]> pointerScrolls = new ArrayList<>();
    final ModernController.PointerSink pointerSink = new ModernController.PointerSink() {
        @Override public void move(int dx, int dy) { pointerMoves.add(new int[] {dx, dy}); }
        @Override public void button(int button, boolean down) { pointerButtons.add(down ? button : -button); }
        @Override public void scroll(int vertical, int horizontal) { pointerScrolls.add(new int[] {vertical, horizontal}); }
    };

    @Before
    public void setUp() {
        Context app = RuntimeEnvironment.getApplication();
        ModernController.clearLayouts(app);
        app.getSharedPreferences(ClassicLayoutMigration.CLASSIC_PREFS,
                Context.MODE_PRIVATE).edit().clear().commit();
        UiNavigationTest.Screen screen = Robolectric.buildActivity(UiNavigationTest.Screen.class).setup().visible().get();
        frame = new FrameLayout(screen);
        screen.setContentView(frame, new FrameLayout.LayoutParams(1600, 720));
        prefs = new PreferenceConfiguration();
        prefs.oscOpacity = 90;
        // HOME follows the guide button setting; on here so every control is covered.
        prefs.showGuideButton = true;
        prefs.showTouchpadButton = true;
        pointerMoves.clear();
        pointerButtons.clear();
        pointerScrolls.clear();
        // The editor's panel settles at once, so sizes and places are read as they end up.
        PadEditor.transitions = false;
        layout();
    }

    @After
    public void tearDown() {
        PadEditor.transitions = true;
    }

    void layout() {
        frame.measure(View.MeasureSpec.makeMeasureSpec(1600, View.MeasureSpec.EXACTLY),
                View.MeasureSpec.makeMeasureSpec(720, View.MeasureSpec.EXACTLY));
        frame.layout(0, 0, 1600, 720);
    }

    ModernController controller(String game) {
        ModernController controller = new ModernController(frame.getContext(), frame, sink, prefs, game);
        controller.refreshLayout();
        controller.show();
        layout();
        return controller;
    }

    int[] last() {
        return reports.get(reports.size() - 1);
    }

    /** A controller whose saved layout shows the WASD stick, with fixed sticks so a push starts at the drawn center. */
    ModernController wasdController() {
        prefs.oscFloatingSticks = false;
        ModernController first = controller(GAME);
        PadLayout draft = first.layout().copy();
        draft.spec(PadLayout.Control.WASD).hidden = false;
        first.save(draft, false);
        first.destroy();
        ModernController controller = controller(GAME);
        assertEquals(View.VISIBLE, controller.view(PadLayout.Control.WASD).getVisibility());
        return controller;
    }

    /** The pointer touchpad is built from a saved layout with its mode, and it sends through the pointer sink. */
    ModernController pointerController(boolean camera) {
        ModernController first = controller(GAME);
        PadLayout draft = first.layout().copy();
        draft.spec(PadLayout.Control.POINTER_PAD).hidden = false;
        draft.setPointerMode(camera ? PadLayout.PointerMode.CAMERA : PadLayout.PointerMode.MOUSE);
        first.save(draft, false);
        first.destroy();
        ModernController controller = controller(GAME);
        controller.setPointerSink(pointerSink);
        return controller;
    }

    /** Lays the frame out and lets the editor's posted panel updates run, as the next frames would. */
    void settle() {
        for (int pass = 0; pass < 3; pass++) {
            ShadowLooper.idleMainLooper();
            layout();
        }
    }

    static void drag(View view, float fromX, float fromY, float toX, float toY) {
        long now = android.os.SystemClock.uptimeMillis();
        MotionEvent down = MotionEvent.obtain(now, now, MotionEvent.ACTION_DOWN, fromX, fromY, 0);
        view.dispatchTouchEvent(down);
        down.recycle();
        for (int step = 1; step <= 4; step++) {
            float t = step / 4f;
            MotionEvent move = MotionEvent.obtain(now, now + 16 * step, MotionEvent.ACTION_MOVE,
                    fromX + (toX - fromX) * t, fromY + (toY - fromY) * t, 0);
            view.dispatchTouchEvent(move);
            move.recycle();
        }
        MotionEvent up = MotionEvent.obtain(now, now + 100, MotionEvent.ACTION_UP, toX, toY, 0);
        view.dispatchTouchEvent(up);
        up.recycle();
    }

    static void tap(View view, float x, float y) {
        long now = android.os.SystemClock.uptimeMillis();
        MotionEvent down = MotionEvent.obtain(now, now, MotionEvent.ACTION_DOWN, x, y, 0);
        view.dispatchTouchEvent(down);
        down.recycle();
        MotionEvent up = MotionEvent.obtain(now, now + 40, MotionEvent.ACTION_UP, x, y, 0);
        view.dispatchTouchEvent(up);
        up.recycle();
    }
}
