package com.limelight.preferences;

import android.app.Application;
import android.content.Intent;
import android.content.ServiceConnection;
import android.net.Uri;
import android.os.Bundle;
import android.widget.TextView;
import android.view.inputmethod.EditorInfo;
import androidx.appcompat.app.AlertDialog;
import com.google.android.material.textfield.TextInputLayout;
import com.limelight.R;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowDialog;
import org.robolectric.shadows.ShadowLooper;
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

    private static Intent opening(String link) {
        return new Intent(Intent.ACTION_VIEW, Uri.parse(link), RuntimeEnvironment.getApplication(), Screen.class);
    }

    private static LinkedBlockingQueue<?> queue(Screen activity) {
        return ReflectionHelpers.getField(activity, "computersToAdd");
    }

    private static String message(AlertDialog dialog) {
        TextView text = dialog.findViewById(android.R.id.message);
        return text.getText().toString();
    }

    private static AlertDialog shownDialog() {
        ShadowLooper.idleMainLooper();
        android.app.Dialog dialog = ShadowDialog.getLatestDialog();
        assertNotNull("a dialog opens", dialog);
        return (AlertDialog) dialog;
    }

    @Test public void anOpenedLinkAsksAndNamesThePcBeforeAnythingHappens() {
        ShadowDialog.reset();
        try (var controller = Robolectric.buildActivity(Screen.class, opening(LINK)).setup()) {
            Screen activity = controller.get();
            AlertDialog dialog = shownDialog();
            assertTrue(dialog.isShowing());
            assertEquals("Wohnzimmer\n192.168.4.2:47989", message(dialog));
            assertEquals(activity.getString(R.string.pcview_menu_pair_pc), dialog.getButton(AlertDialog.BUTTON_POSITIVE).getText().toString());
            assertTrue("Cancel has the focus", dialog.getButton(AlertDialog.BUTTON_NEGATIVE).isFocused());
            assertTrue(dialog.getButton(AlertDialog.BUTTON_POSITIVE).getFilterTouchesWhenObscured());
            assertTrue("nothing is added before the answer", queue(activity).isEmpty());
        }
    }

    @Test public void pairGoesThroughTheExistingFlow() {
        ShadowDialog.reset();
        try (var controller = Robolectric.buildActivity(Screen.class, opening(LINK)).setup()) {
            Screen activity = controller.get();
            shownDialog().getButton(AlertDialog.BUTTON_POSITIVE).performClick();
            ShadowLooper.idleMainLooper();
            assertEquals(LINK, queue(activity).peek());
            assertFalse(activity.isFinishing());
        }
    }

    @Test public void cancelAndBackAddNothingAndLeave() {
        ShadowDialog.reset();
        try (var controller = Robolectric.buildActivity(Screen.class, opening(LINK)).setup()) {
            Screen activity = controller.get();
            shownDialog().getButton(AlertDialog.BUTTON_NEGATIVE).performClick();
            ShadowLooper.idleMainLooper();
            assertTrue(queue(activity).isEmpty());
            assertTrue(activity.isFinishing());
        }
        ShadowDialog.reset();
        try (var controller = Robolectric.buildActivity(Screen.class, opening(LINK)).setup()) {
            Screen activity = controller.get();
            shownDialog().cancel();
            ShadowLooper.idleMainLooper();
            assertTrue(queue(activity).isEmpty());
            assertTrue(activity.isFinishing());
        }
    }

    @Test public void badLinksGetTheExistingRefusalAndNoConfirmation() {
        String huge = LINK + "&x=" + "a".repeat(2 * 1024 * 1024);
        for (String bad : new String[] {LINK.replace("v=1", "v=2"), "rubylight://pair", "rubylight://pair/x?v=1", "rubylight://other?otp=4821", huge}) {
            ShadowDialog.reset();
            try (var controller = Robolectric.buildActivity(Screen.class, opening(bad)).setup()) {
                Screen activity = controller.get();
                AlertDialog dialog = shownDialog();
                assertEquals(activity.getString(R.string.addpc_qr_invalid), message(dialog));
                assertNotEquals(activity.getString(R.string.pcview_menu_pair_pc), dialog.getButton(AlertDialog.BUTTON_POSITIVE).getText().toString());
                assertTrue(queue(activity).isEmpty());
                assertFalse(activity.isFinishing());
            }
        }
    }

    @Test public void aNormalStartAsksNothing() {
        ShadowDialog.reset();
        try (var controller = Robolectric.buildActivity(Screen.class).setup()) {
            assertNull(ShadowDialog.getLatestDialog());
            assertTrue(queue(controller.get()).isEmpty());
        }
    }

    @Test public void aLinkAsksOnceAndNotAgainFromRecents() {
        Intent again = opening(LINK).addFlags(Intent.FLAG_ACTIVITY_LAUNCHED_FROM_HISTORY);
        ShadowDialog.reset();
        try (var controller = Robolectric.buildActivity(Screen.class, again).setup()) {
            assertNull("an old link in Recents is not asked about again", ShadowDialog.getLatestDialog());
        }
        Bundle answered = new Bundle();
        answered.putBoolean("linkAnswered", true);
        ShadowDialog.reset();
        try (var controller = Robolectric.buildActivity(Screen.class, opening(LINK)).setup(answered)) {
            assertNull("answered before the process went away", ShadowDialog.getLatestDialog());
        }
    }

    @Test public void aNewLinkReplacesTheQuestionOnTheOpenScreen() {
        ShadowDialog.reset();
        try (var controller = Robolectric.buildActivity(Screen.class).setup()) {
            controller.newIntent(opening(LINK));
            AlertDialog first = shownDialog();
            controller.newIntent(opening(LINK.replace("Wohnzimmer", "Studio")));
            AlertDialog second = shownDialog();
            assertFalse(first.isShowing());
            assertEquals("Studio\n192.168.4.2:47989", message(second));
        }
    }

    @Test public void aLinkWaitsForTheRequestOnItsWay() {
        try (var controller = Robolectric.buildActivity(Screen.class).setup()) {
            Screen activity = controller.get();
            ((TextView) activity.findViewById(R.id.hostTextView)).setText("pc.local");
            activity.findViewById(R.id.addPcButton).performClick();
            ShadowDialog.reset();
            controller.newIntent(opening(LINK));
            assertNull(ShadowDialog.getLatestDialog());
            ReflectionHelpers.callInstanceMethod(activity, "setSubmitting",
                    ReflectionHelpers.ClassParameter.from(boolean.class, false));
            assertEquals("Wohnzimmer\n192.168.4.2:47989", message(shownDialog()));
        }
    }

    @Test public void theManifestOpensTheLinkOnThisScreen() throws Exception {
        String manifest = new String(java.nio.file.Files.readAllBytes(java.nio.file.Paths.get("src/main/AndroidManifest.xml")),
                java.nio.charset.StandardCharsets.UTF_8);
        int start = manifest.indexOf("android:name=\".preferences.AddComputerManually\"");
        String activity = manifest.substring(start, manifest.indexOf("</activity>", start));
        assertTrue(activity.contains("android:exported=\"true\""));
        assertTrue(activity.contains("android.intent.action.VIEW"));
        assertTrue(activity.contains("android.intent.category.DEFAULT"));
        assertTrue(activity.contains("android.intent.category.BROWSABLE"));
        assertTrue(activity.contains("<data android:scheme=\"rubylight\" android:host=\"pair\" />"));
    }
}
