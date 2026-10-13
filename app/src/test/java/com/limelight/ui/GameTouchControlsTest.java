package com.limelight.ui;

import android.app.Application;
import android.os.Bundle;
import android.view.MotionEvent;
import android.widget.FrameLayout;
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
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
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
    private static void touch(GameTouchControls controls, int action) {
        MotionEvent event = MotionEvent.obtain(0, 10, action, 10, 10, 0);
        try { controls.getChildAt(0).dispatchTouchEvent(event); } finally { event.recycle(); }
    }
}
