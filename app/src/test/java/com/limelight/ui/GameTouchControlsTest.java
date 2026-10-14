package com.limelight.ui;

import android.app.Application;
import android.os.Bundle;
import android.view.KeyEvent;
import android.view.MotionEvent;
import android.view.View;
import android.widget.FrameLayout;
import android.widget.TextView;
import androidx.appcompat.app.AppCompatActivity;
import com.limelight.R;
import com.limelight.binding.input.HeldInputState;
import com.limelight.binding.input.TouchLayout;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowLooper;
import org.robolectric.util.ReflectionHelpers;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.TimeUnit;
import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class)
public class GameTouchControlsTest {
    public static class Screen extends AppCompatActivity {
        @Override protected void onCreate(Bundle state) {
            setTheme(R.style.AppTheme);
            super.onCreate(state);
            setContentView(new FrameLayout(this));
        }
    }
    @Test public void cancellingAHeldGestureCannotToggleItOnAfterReturningToTheStream() {
        try (ActivityController<Screen> controller = Robolectric.buildActivity(Screen.class).setup()) {
            Screen screen = controller.get();
            List<Boolean> transitions = new ArrayList<>();
            TouchLayout.Button button = new TouchLayout.Button("fire", "Fire", new int[0], 1,
                    true, .1f, .1f, .2f, .2f);
            new TouchLayout(Collections.singletonList(button)).save(screen, "test");
            TouchLayout.setEnabled(screen, "test", true);
            GameTouchControls controls = new GameTouchControls(screen, "test", new HeldInputState.Sink() {
                public void key(int key, boolean down) { fail("Mouse binding only"); }
                public void mouse(int button, boolean down) { transitions.add(down); }
            }, () -> true, dialog -> { });
            controls.layout(0, 0, 800, 400);
            touch(controls, MotionEvent.ACTION_DOWN);
            controls.releaseInput(); // Menu/background teardown cancels the gesture.
            touch(controls, MotionEvent.ACTION_UP);
            assertTrue(transitions.isEmpty());
            touch(controls, MotionEvent.ACTION_DOWN);
            touch(controls, MotionEvent.ACTION_UP);
            assertEquals(Collections.singletonList(true), transitions);
            controls.releaseInput();
            assertEquals(java.util.Arrays.asList(true, false), transitions);
        }
    }

    @Test public void changingProfilesReleasesChordsAndLoadsVisibilityWithoutSavingEitherLayout() {
        try (ActivityController<Screen> controller = Robolectric.buildActivity(Screen.class).setup()) {
            Screen screen = controller.get();
            new TouchLayout(Collections.singletonList(new TouchLayout.Button("hold", "First game",
                    new int[] {113, 51}, 0, false, .1f, .1f, .2f, .2f))).save(screen, "first");
            saveMouseLayout(screen, "second", "Second game", 3, false);
            TouchLayout.setEnabled(screen, "first", true);
            TouchLayout.setEnabled(screen, "second", false);
            Map<String, ?> saved = new HashMap<>(screen.getSharedPreferences("GameTouchLayouts", 0).getAll());
            List<String> transitions = new ArrayList<>();
            GameTouchControls controls = controls(screen, "first", transitions);

            touch(controls, MotionEvent.ACTION_DOWN);
            ReflectionHelpers.setField(controls, "editing", true);
            controls.setProfile("second");

            assertEquals(Arrays.asList("113:true", "51:true", "51:false", "113:false"), transitions);
            assertEquals("Second game", ((TextView) button(controls)).getText().toString());
            assertEquals(View.GONE, controls.getVisibility());
            assertFalse(controls.isEditing());
            controls.setProfile("first");
            assertEquals("First game", ((TextView) button(controls)).getText().toString());
            assertEquals(View.VISIBLE, controls.getVisibility());
            assertEquals(saved, screen.getSharedPreferences("GameTouchLayouts", 0).getAll());
        }
    }

    @Test public void lateGesturesFromThePreviousProfileCannotAffectTheNewControls() {
        try (ActivityController<Screen> controller = Robolectric.buildActivity(Screen.class).setup()) {
            Screen screen = controller.get();
            saveMouseLayout(screen, "first", "First game", 1, true);
            saveMouseLayout(screen, "second", "Second game", 3, false);
            List<String> transitions = new ArrayList<>();
            GameTouchControls controls = controls(screen, "first", transitions);
            View previous = button(controls);
            touch(previous, MotionEvent.ACTION_DOWN);
            touch(previous, MotionEvent.ACTION_UP);
            touch(previous, MotionEvent.ACTION_DOWN);

            controls.setProfile("second");
            touch(controls, MotionEvent.ACTION_DOWN);
            // Imported profiles may reuse button IDs. An old cancel must not release the new button.
            touch(previous, MotionEvent.ACTION_UP);
            touch(previous, MotionEvent.ACTION_CANCEL);
            previous.performClick();
            assertEquals(Arrays.asList("m1:true", "m1:false", "m3:true"), transitions);
            touch(controls, MotionEvent.ACTION_UP);
            assertEquals(Arrays.asList("m1:true", "m1:false", "m3:true", "m3:false"), transitions);
        }
    }

    @Test public void profileSelectionPreservesPipHidingAndDoesNotResetTheSameGame() {
        try (ActivityController<Screen> controller = Robolectric.buildActivity(Screen.class).setup()) {
            Screen screen = controller.get();
            saveMouseLayout(screen, "first", "First game", 1, false);
            saveMouseLayout(screen, "second", "Second game", 3, false);
            List<String> transitions = new ArrayList<>();
            GameTouchControls controls = controls(screen, "first", transitions);
            controls.hideForPip(true);
            controls.setProfile("second");
            assertEquals(View.GONE, controls.getVisibility());
            controls.hideForPip(false);
            assertEquals(View.VISIBLE, controls.getVisibility());
            touch(controls, MotionEvent.ACTION_DOWN);
            View current = button(controls);
            controls.setProfile("second");
            assertSame(current, button(controls));
            assertEquals(Collections.singletonList("m3:true"), transitions);
            controls.releaseInput();
            assertEquals(Arrays.asList("m3:true", "m3:false"), transitions);
        }
    }

    @Test public void aFingerBesideAHeldControlReachesTheStreamAndTheControlReleasesOnItsOwnLift() {
        try (ActivityController<Screen> controller = Robolectric.buildActivity(Screen.class).setup()) {
            Screen screen = controller.get();
            // "W" held by one finger at 80..240 x 40..120 while another finger aims on the stream.
            new TouchLayout(Collections.singletonList(new TouchLayout.Button("w", "W", new int[] {51}, 0,
                    false, .1f, .1f, .2f, .2f))).save(screen, "fps");
            TouchLayout.setEnabled(screen, "fps", true);
            List<String> transitions = new ArrayList<>();
            List<String> stream = new ArrayList<>();
            GameTouchControls controls = controls(screen, "fps", transitions);
            controls.setStreamTouchListener((view, event) -> {
                stream.add(MotionEvent.actionToString(event.getActionMasked()) + " " + event.getPointerCount() +
                        " " + Math.round(event.getX()));
                return true;
            });
            layOut(controls);
            float[] w = {100, 60}, aim = {600, 300};
            int[] first = {0}, second = {1}, two = {0, 1};

            dispatch(controls, MotionEvent.ACTION_DOWN, 0, first, w);
            assertEquals(Collections.singletonList("51:true"), transitions);
            dispatch(controls, MotionEvent.ACTION_POINTER_DOWN, 1, two, w, aim);
            assertEquals(Collections.singletonList("ACTION_DOWN 1 600"), stream);
            dispatch(controls, MotionEvent.ACTION_MOVE, 0, two, new float[] {101, 61}, new float[] {640, 300});
            assertEquals("ACTION_MOVE 1 640", stream.get(stream.size() - 1));
            dispatch(controls, MotionEvent.ACTION_POINTER_UP, 0, two, new float[] {101, 61}, new float[] {640, 300});
            assertEquals("Lifting the W finger releases W while the aim continues",
                    Arrays.asList("51:true", "51:false"), transitions);
            dispatch(controls, MotionEvent.ACTION_UP, 0, second, new float[] {660, 300});
            assertEquals("ACTION_UP 1 660", stream.get(stream.size() - 1));
            assertEquals(Arrays.asList("51:true", "51:false"), transitions);

            // The other order: aiming first, then pressing and lifting W with a second finger.
            stream.clear();
            dispatch(controls, MotionEvent.ACTION_DOWN, 0, first, aim);
            dispatch(controls, MotionEvent.ACTION_POINTER_DOWN, 1, two, aim, w);
            dispatch(controls, MotionEvent.ACTION_POINTER_UP, 1, two, aim, w);
            assertEquals(Arrays.asList("51:true", "51:false", "51:true", "51:false"), transitions);
            dispatch(controls, MotionEvent.ACTION_UP, 0, first, aim);
            assertEquals("ACTION_DOWN 1 600", stream.get(0));
            assertEquals("ACTION_UP 1 600", stream.get(stream.size() - 1));
        }
    }

    @Test public void aHeldControlReleasesOnItsOwnLiftWhenTheStreamDeclinesTheOtherFinger() {
        try (ActivityController<Screen> controller = Robolectric.buildActivity(Screen.class).setup()) {
            Screen screen = controller.get();
            new TouchLayout(Collections.singletonList(new TouchLayout.Button("w", "W", new int[] {51}, 0,
                    false, .1f, .1f, .2f, .2f))).save(screen, "fps");
            TouchLayout.setEnabled(screen, "fps", true);
            List<String> transitions = new ArrayList<>();
            GameTouchControls controls = controls(screen, "fps", transitions);
            // The stream declines a finger that lands on the debug performance overlay, so
            // Android hands that finger to the control that is already held.
            controls.setStreamTouchListener((view, event) -> false);
            layOut(controls);
            float[] w = {100, 60}, overlay = {600, 300};
            int[] first = {0}, second = {1}, two = {0, 1};

            dispatch(controls, MotionEvent.ACTION_DOWN, 0, first, w);
            dispatch(controls, MotionEvent.ACTION_POINTER_DOWN, 1, two, w, overlay);
            dispatch(controls, MotionEvent.ACTION_MOVE, 0, two, w, new float[] {620, 300});
            dispatch(controls, MotionEvent.ACTION_POINTER_UP, 0, two, w, new float[] {620, 300});
            assertEquals("Lifting the W finger releases W while the overlay finger stays down",
                    Arrays.asList("51:true", "51:false"), transitions);
            dispatch(controls, MotionEvent.ACTION_MOVE, 0, second, new float[] {640, 300});
            dispatch(controls, MotionEvent.ACTION_UP, 0, second, new float[] {640, 300});
            assertEquals(Arrays.asList("51:true", "51:false"), transitions);

            // The other order: the overlay finger lifts first and W stays held until its own lift.
            dispatch(controls, MotionEvent.ACTION_DOWN, 0, first, w);
            dispatch(controls, MotionEvent.ACTION_POINTER_DOWN, 1, two, w, overlay);
            dispatch(controls, MotionEvent.ACTION_POINTER_UP, 1, two, w, overlay);
            assertEquals(Arrays.asList("51:true", "51:false", "51:true"), transitions);
            dispatch(controls, MotionEvent.ACTION_UP, 0, first, w);
            assertEquals(Arrays.asList("51:true", "51:false", "51:true", "51:false"), transitions);
        }
    }

    @Test public void enterAndSpaceReachTheStreamInsteadOfActivatingAControl() {
        try (ActivityController<Screen> controller = Robolectric.buildActivity(Screen.class).setup()) {
            Screen screen = controller.get();
            saveMouseLayout(screen, "keys", "Fire", 1, false);
            List<String> transitions = new ArrayList<>();
            List<String> streamKeys = new ArrayList<>();
            FrameLayout content = screen.findViewById(android.R.id.content);
            // The stream view takes keys for the host the way Game sets it up.
            StreamView stream = new StreamView(screen);
            stream.setFocusable(true);
            stream.setFocusableInTouchMode(true);
            stream.setOnKeyListener((view, keyCode, event) -> {
                streamKeys.add(keyCode + (event.getAction() == KeyEvent.ACTION_DOWN ? " down" : " up"));
                return true;
            });
            content.addView(stream);
            GameTouchControls controls = controls(screen, "keys", transitions);
            content.addView(controls);
            ShadowLooper.idleMainLooper();
            // A hardware keyboard takes the window out of touch mode, where focus can move by key.
            assertTrue(stream.requestFocusFromTouch());

            // Accessibility and keyboard navigation try to move focus onto the control.
            button(controls).requestFocus();
            View next = stream.focusSearch(View.FOCUS_FORWARD);
            if (next != null) next.requestFocus();
            for (int key : new int[] {KeyEvent.KEYCODE_ENTER, KeyEvent.KEYCODE_SPACE}) {
                screen.dispatchKeyEvent(new KeyEvent(KeyEvent.ACTION_DOWN, key));
                screen.dispatchKeyEvent(new KeyEvent(KeyEvent.ACTION_UP, key));
            }
            ShadowLooper.idleMainLooper(100, TimeUnit.MILLISECONDS);

            int enter = KeyEvent.KEYCODE_ENTER, space = KeyEvent.KEYCODE_SPACE;
            assertEquals(Arrays.asList(enter + " down", enter + " up", space + " down", space + " up"), streamKeys);
            assertEquals("A control took the keys meant for the host", Collections.emptyList(), transitions);
            assertTrue(stream.isFocused());
        }
    }

    private static View button(GameTouchControls controls) {
        // Child 0 is the full-size layer that forwards touches beside the controls to the stream.
        return controls.getChildAt(1);
    }

    private static void layOut(GameTouchControls controls) {
        for (int pass = 0; pass < 2; pass++) {
            controls.measure(View.MeasureSpec.makeMeasureSpec(800, View.MeasureSpec.EXACTLY),
                    View.MeasureSpec.makeMeasureSpec(400, View.MeasureSpec.EXACTLY));
            controls.layout(0, 0, 800, 400);
        }
    }

    private static void dispatch(View view, int action, int actionIndex, int[] ids, float[]... points) {
        MotionEvent.PointerProperties[] properties = new MotionEvent.PointerProperties[points.length];
        MotionEvent.PointerCoords[] coords = new MotionEvent.PointerCoords[points.length];
        for (int i = 0; i < points.length; i++) {
            properties[i] = new MotionEvent.PointerProperties();
            properties[i].id = ids[i];
            properties[i].toolType = MotionEvent.TOOL_TYPE_FINGER;
            coords[i] = new MotionEvent.PointerCoords();
            coords[i].x = points[i][0];
            coords[i].y = points[i][1];
        }
        int masked = action == MotionEvent.ACTION_POINTER_DOWN || action == MotionEvent.ACTION_POINTER_UP ?
                action | (actionIndex << MotionEvent.ACTION_POINTER_INDEX_SHIFT) : action;
        MotionEvent event = MotionEvent.obtain(0, 10, masked, points.length, properties, coords, 0, 0, 1, 1, 0, 0,
                android.view.InputDevice.SOURCE_TOUCHSCREEN, 0);
        try { view.dispatchTouchEvent(event); } finally { event.recycle(); }
    }

    private static void saveMouseLayout(Screen screen, String profile, String name, int mouse, boolean toggle) {
        new TouchLayout(Collections.singletonList(new TouchLayout.Button("hold", name, new int[0], mouse,
                toggle, .1f, .1f, .2f, .2f))).save(screen, profile);
        TouchLayout.setEnabled(screen, profile, true);
    }

    private static GameTouchControls controls(Screen screen, String profile, List<String> transitions) {
        GameTouchControls controls = new GameTouchControls(screen, profile, new HeldInputState.Sink() {
            public void key(int key, boolean down) { transitions.add(key + ":" + down); }
            public void mouse(int button, boolean down) { transitions.add("m" + button + ":" + down); }
        }, () -> true, dialog -> { });
        controls.layout(0, 0, 800, 400);
        return controls;
    }

    private static void touch(GameTouchControls controls, int action) {
        touch(button(controls), action);
    }

    private static void touch(View view, int action) {
        MotionEvent event = MotionEvent.obtain(0, 10, action, 10, 10, 0);
        try { view.dispatchTouchEvent(event); } finally { event.recycle(); }
    }
}
