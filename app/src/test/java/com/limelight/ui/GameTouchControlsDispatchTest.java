package com.limelight.ui;

import android.app.Application;
import android.view.InputDevice;
import android.view.MotionEvent;
import android.view.View;
import android.widget.FrameLayout;

import com.limelight.binding.input.HeldInputState;
import com.limelight.binding.input.TouchLayout;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;

import java.util.Arrays;

import static org.junit.Assert.*;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 30, application = Application.class)
public class GameTouchControlsDispatchTest {
    @Test public void customControlsAskForUnbufferedTouchesLikeTheStream() {
        try (ActivityController<GameTouchControlsTest.Screen> controller =
                     Robolectric.buildActivity(GameTouchControlsTest.Screen.class).setup()) {
            GameTouchControlsTest.Screen screen = controller.get();
            new TouchLayout(Arrays.asList(
                    TouchLayout.Button.wasd("move", "WASD", new int[] {51, 29, 47, 32}, false, .2f, .4f, .05f, .5f, .25f, .4f),
                    new TouchLayout.Button("fire", "Fire", new int[0], 1, false, .6f, .5f, .2f, .3f))).save(screen, "dispatch");
            TouchLayout.setEnabled(screen, "dispatch", true);
            GameTouchControls controls = new GameTouchControls(screen, "dispatch", new HeldInputState.Sink() {
                public void key(int key, boolean down) { }
                public void mouse(int button, boolean down) { }
            }, () -> true, dialog -> { });
            ((FrameLayout) screen.findViewById(android.R.id.content)).addView(controls);
            for (int pass = 0; pass < 2; pass++) {
                controls.measure(View.MeasureSpec.makeMeasureSpec(800, View.MeasureSpec.EXACTLY),
                        View.MeasureSpec.makeMeasureSpec(400, View.MeasureSpec.EXACTLY));
                controls.layout(0, 0, 800, 400);
            }
            // The stream view's own request is per event too: the window delivers the rest of the
            // gesture as it arrives instead of batching it to the next frame.
            for (View view : new View[] {controls.getChildAt(1), controls.getChildAt(2)}) {
                Object attachInfo = ReflectionHelpers.getField(view, "mAttachInfo");
                assertNotNull(attachInfo);
                ReflectionHelpers.setField(attachInfo, "mUnbufferedDispatchRequested", false);
                MotionEvent down = MotionEvent.obtain(0, 0, MotionEvent.ACTION_DOWN, 10, 10, 0);
                down.setSource(InputDevice.SOURCE_TOUCHSCREEN);
                MotionEvent up = MotionEvent.obtain(0, 5, MotionEvent.ACTION_UP, 10, 10, 0);
                up.setSource(InputDevice.SOURCE_TOUCHSCREEN);
                view.dispatchTouchEvent(down);
                assertTrue(view.toString(), (boolean) ReflectionHelpers.getField(attachInfo, "mUnbufferedDispatchRequested"));
                view.dispatchTouchEvent(up);
                down.recycle();
                up.recycle();
            }
        }
    }
}
