package com.limelight.preferences;

import android.app.Application;
import android.content.Intent;
import android.content.ServiceConnection;
import android.widget.TextView;
import android.view.inputmethod.EditorInfo;
import com.google.android.material.textfield.TextInputLayout;
import com.limelight.R;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.annotation.Config;
import org.robolectric.util.ReflectionHelpers;
import java.util.concurrent.LinkedBlockingQueue;
import static org.junit.Assert.*;

@RunWith(org.robolectric.RobolectricTestRunner.class)
@Config(sdk = 28, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class,
        instrumentedPackages = "com.limelight.nvstream.jni")
public class AddComputerInputTest {
    public static class Screen extends AddComputerManually {
        @Override public boolean bindService(Intent intent, ServiceConnection connection, int flags) { return true; }
        @Override public void unbindService(ServiceConnection connection) { }
    }
    public static class UnavailableScreen extends Screen {
        @Override public boolean bindService(Intent intent, ServiceConnection connection, int flags) { return false; }
    }

    @Test public void unavailableDiscoveryDoesNotLeaveSubmitDisabled() {
        try (var controller = Robolectric.buildActivity(UnavailableScreen.class).setup()) {
            UnavailableScreen activity = controller.get();
            ((TextView) activity.findViewById(R.id.hostTextView)).setText("pc.local");
            activity.findViewById(R.id.addPcButton).performClick();
            TextInputLayout input = activity.findViewById(R.id.host_input);
            assertEquals(activity.getString(R.string.add_pc_service_unavailable), input.getError());
            assertTrue(activity.findViewById(R.id.addPcButton).isEnabled());
        }
    }

    @Test public void invalidInputStaysInlineAndEditingClearsTheError() {
        try (var controller = Robolectric.buildActivity(Screen.class).setup()) {
            Screen activity = controller.get();
            TextView field = activity.findViewById(R.id.hostTextView);
            TextInputLayout input = activity.findViewById(R.id.host_input);
            activity.findViewById(R.id.addPcButton).performClick();
            assertEquals(activity.getString(R.string.addpc_enter_ip), input.getError());
            field.setText("pc.local:65536");
            assertNull(input.getError());
            field.onEditorAction(EditorInfo.IME_ACTION_DONE);
            assertEquals(activity.getString(R.string.add_pc_address_error), input.getError());
            LinkedBlockingQueue<?> queue = ReflectionHelpers.getField(activity, "computersToAdd");
            assertTrue(queue.isEmpty());
        }
    }

    @Test public void repeatedImeAndButtonSubmissionsQueueOnlyOneRequest() {
        try (var controller = Robolectric.buildActivity(Screen.class).setup()) {
            Screen activity = controller.get();
            TextView field = activity.findViewById(R.id.hostTextView);
            field.setText("  pc.local:47989  ");
            field.onEditorAction(EditorInfo.IME_ACTION_DONE);
            field.onEditorAction(EditorInfo.IME_ACTION_DONE);
            activity.findViewById(R.id.addPcButton).performClick();
            LinkedBlockingQueue<?> queue = ReflectionHelpers.getField(activity, "computersToAdd");
            assertEquals(1, queue.size());
            assertEquals("pc.local:47989", queue.peek());
            assertFalse(activity.findViewById(R.id.addPcButton).isEnabled());
        }
    }
}
