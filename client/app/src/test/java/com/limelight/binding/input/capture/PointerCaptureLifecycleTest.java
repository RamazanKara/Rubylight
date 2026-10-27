package com.limelight.binding.input.capture;

import android.app.Activity;
import android.app.Application;
import android.os.Looper;
import android.view.InputDevice;
import android.view.View;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.annotation.Config;
import org.robolectric.annotation.Implementation;
import org.robolectric.annotation.Implements;
import org.robolectric.shadow.api.Shadow;

import java.time.Duration;

import static org.junit.Assert.assertEquals;
import static org.robolectric.Shadows.shadowOf;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class, shadows = PointerCaptureLifecycleTest.Mouse.class)
public class PointerCaptureLifecycleTest {
    @Implements(InputDevice.class)
    public static class Mouse {
        @Implementation protected static int[] getDeviceIds() { return new int[]{1}; }
        @Implementation protected static InputDevice getDevice(int id) {
            return Shadow.newInstanceOf(InputDevice.class);
        }
        @Implementation protected boolean supportsSource(int source) {
            return source == InputDevice.SOURCE_MOUSE;
        }
    }

    private static class CaptureView extends View {
        int requests;
        CaptureView(Activity activity) { super(activity); }
        @Override public void requestPointerCapture() { requests++; }
        @Override public boolean hasWindowFocus() { return true; }
    }

    @Test
    public void delayedFocusCallbackCannotCaptureAfterInputWasDisabled() {
        Activity activity = Robolectric.buildActivity(Activity.class).setup().get();
        CaptureView view = new CaptureView(activity);
        AndroidNativePointerCaptureProvider provider = new AndroidNativePointerCaptureProvider(activity, view);
        provider.enableCapture();
        assertEquals(1, view.requests);
        provider.onWindowFocusChanged(true);
        provider.disableCapture();
        shadowOf(Looper.getMainLooper()).idleFor(Duration.ofSeconds(1));
        assertEquals(1, view.requests);
    }
}
