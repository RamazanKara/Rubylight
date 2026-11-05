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
import org.robolectric.RuntimeEnvironment;
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

    private static final String LINK = "rubylight://pair?v=1&host=192.168.4.2&port=47989&otp=4821&pass=hunter2&fp="
            + "00".repeat(32) + "&name=Wohnzimmer";

    private static Intent fromLink() {
        return new Intent(RuntimeEnvironment.getApplication(), Screen.class).putExtra(AddComputerManually.EXTRA_FROM_LINK, true);
    }

    private static LinkedBlockingQueue<?> queue(Screen activity) {
        return ReflectionHelpers.getField(activity, "computersToAdd");
    }

    @Test public void aConfirmedLinkGoesThroughTheExistingFlowOnce() {
        LinkGate.reset();
        LinkGate.confirm(LINK);
        try (var controller = Robolectric.buildActivity(Screen.class, fromLink()).setup()) {
            assertEquals(LINK, queue(controller.get()).peek());
            assertTrue("a link from outside waits for this request", LinkGate.held());
        }
        assertFalse("the screen leaving lets go", LinkGate.held());
        // Back from Recents or recreated: the mark is there, the link is not.
        try (var controller = Robolectric.buildActivity(Screen.class, fromLink()).setup()) {
            assertTrue(queue(controller.get()).isEmpty());
        }
    }

    @Test public void aScreenOpenedForALinkLeavesToThePcListWhateverBroughtItBack() {
        LinkGate.reset();
        try (var controller = Robolectric.buildActivity(Screen.class, fromLink()).setup()) {
            Screen activity = controller.get();
            org.robolectric.Shadows.shadowOf(activity).setIsTaskRoot(true);
            activity.finish();
            Intent next = org.robolectric.Shadows.shadowOf(activity).getNextStartedActivity();
            assertEquals(com.limelight.PcView.class.getName(), next.getComponent().getClassName());
        }
    }

    @Test public void aNormalStartLeavesAConfirmedLinkAlone() {
        LinkGate.reset();
        LinkGate.confirm(LINK);
        try (var controller = Robolectric.buildActivity(Screen.class).setup()) {
            assertTrue(queue(controller.get()).isEmpty());
        }
        LinkGate.reset();
    }

    @Test public void aLinkWaitsBehindATypedAddressAndIsNotLostWhenItSucceeds() {
        LinkGate.reset();
        try (var controller = Robolectric.buildActivity(Screen.class).setup()) {
            Screen activity = controller.get();
            ((TextView) activity.findViewById(R.id.hostTextView)).setText("pc.local");
            activity.findViewById(R.id.addPcButton).performClick();
            assertTrue(LinkGate.held());
            LinkGate.put(LINK);
            // Added: the screen closes, as it does after a typed address.
            activity.finish();
            controller.pause().stop().destroy();
            assertFalse(LinkGate.held());
            Intent asks = org.robolectric.Shadows.shadowOf(RuntimeEnvironment.getApplication()).getNextStartedActivity();
            assertEquals(PairLinkActivity.class.getName(), asks.getComponent().getClassName());
            assertEquals(LINK, LinkGate.take());
        }
    }

    @Test public void aFailedRequestLetsTheLinkThrough() {
        LinkGate.reset();
        try (var controller = Robolectric.buildActivity(Screen.class).setup()) {
            Screen activity = controller.get();
            ((TextView) activity.findViewById(R.id.hostTextView)).setText("pc.local");
            activity.findViewById(R.id.addPcButton).performClick();
            LinkGate.put(LINK);
            ReflectionHelpers.callInstanceMethod(activity, "setSubmitting",
                    ReflectionHelpers.ClassParameter.from(boolean.class, false));
            Intent asks = org.robolectric.Shadows.shadowOf(RuntimeEnvironment.getApplication()).getNextStartedActivity();
            assertEquals(PairLinkActivity.class.getName(), asks.getComponent().getClassName());
        }
        LinkGate.reset();
    }
}
