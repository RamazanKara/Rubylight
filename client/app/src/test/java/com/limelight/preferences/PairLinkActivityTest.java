package com.limelight.preferences;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotEquals;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.content.Intent;
import android.net.Uri;
import android.os.Looper;
import android.widget.TextView;

import androidx.appcompat.app.AlertDialog;

import com.limelight.R;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.Shadows;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowDialog;
import org.robolectric.shadows.ShadowLooper;

import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Paths;
import java.time.Duration;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 33, application = Application.class,
        shadows = com.limelight.PcViewLifecycleTest.NoNativeMoonBridge.class)
public class PairLinkActivityTest {
    private static final String LINK = "rubylight://pair?v=1&host=192.168.4.2&port=47989&otp=4821&pass=hunter2&fp="
            + "00".repeat(32) + "&name=Wohnzimmer";

    private static Intent opening(String link) {
        return new Intent(Intent.ACTION_VIEW, Uri.parse(link), RuntimeEnvironment.getApplication(), PairLinkActivity.class);
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

    private static Intent nextStarted() {
        return Shadows.shadowOf(RuntimeEnvironment.getApplication()).getNextStartedActivity();
    }

    private static void pass(long milliseconds) {
        Shadows.shadowOf(Looper.getMainLooper()).idleFor(Duration.ofMillis(milliseconds));
    }

    @Before
    public void clean() {
        LinkGate.reset();
        ShadowDialog.reset();
    }

    @After
    public void cleanAfter() {
        LinkGate.reset();
    }

    @Test
    public void anOpenedLinkAsksAndNamesThePcBeforeAnythingHappens() {
        try (var controller = Robolectric.buildActivity(PairLinkActivity.class, opening(LINK)).setup()) {
            PairLinkActivity activity = controller.get();
            AlertDialog dialog = shownDialog();
            assertTrue(dialog.isShowing());
            assertEquals("Wohnzimmer\nAddress: 192.168.4.2:47989", message(dialog));
            assertEquals(activity.getString(R.string.pcview_menu_pair_pc), dialog.getButton(AlertDialog.BUTTON_POSITIVE).getText().toString());
            assertTrue("Cancel has the focus", dialog.getButton(AlertDialog.BUTTON_NEGATIVE).isFocused());
            assertTrue(dialog.getButton(AlertDialog.BUTTON_POSITIVE).getFilterTouchesWhenObscured());
            assertNull("nothing is added before the answer", LinkGate.takeConfirmed());
            assertNull(nextStarted());
        }
    }

    @Test
    public void pairOnlyWorksOnceTheQuestionHasBeenThereASecond() {
        try (var controller = Robolectric.buildActivity(PairLinkActivity.class, opening(LINK)).setup()) {
            AlertDialog dialog = shownDialog();
            assertFalse("a tap meant for what was there before", dialog.getButton(AlertDialog.BUTTON_POSITIVE).isEnabled());
            pass(PairLinkActivity.PAIR_DELAY_MS - 1);
            assertFalse(dialog.getButton(AlertDialog.BUTTON_POSITIVE).isEnabled());
            pass(1);
            assertTrue(dialog.getButton(AlertDialog.BUTTON_POSITIVE).isEnabled());
        }
    }

    @Test
    public void pairHandsTheLinkOverInMemoryOnly() {
        try (var controller = Robolectric.buildActivity(PairLinkActivity.class, opening(LINK)).setup()) {
            PairLinkActivity activity = controller.get();
            pass(PairLinkActivity.PAIR_DELAY_MS);
            shownDialog().getButton(AlertDialog.BUTTON_POSITIVE).performClick();
            ShadowLooper.idleMainLooper();
            assertTrue(activity.isFinishing());
            Intent next = Shadows.shadowOf(activity).getNextStartedActivity();
            assertEquals(AddComputerManually.class.getName(), next.getComponent().getClassName());
            assertTrue(next.getBooleanExtra(AddComputerManually.EXTRA_FROM_LINK, false));
            assertNull("no secret in the intent Recents keeps", next.getData());
            assertFalse(next.toUri(Intent.URI_INTENT_SCHEME).contains("4821"));
            assertEquals(1, next.getExtras().size());
            assertTrue("no other link comes up before it is picked up", LinkGate.held());
            assertEquals(LINK, LinkGate.takeConfirmed());
            assertFalse(LinkGate.held());
        }
    }

    @Test
    public void theIntentWithTheSecretsIsDropped() {
        try (var controller = Robolectric.buildActivity(PairLinkActivity.class, opening(LINK)).setup()) {
            assertNull(controller.get().getIntent().getData());
        }
    }

    @Test
    public void cancelAndBackAddNothingAndLeave() {
        try (var controller = Robolectric.buildActivity(PairLinkActivity.class, opening(LINK)).setup()) {
            PairLinkActivity activity = controller.get();
            shownDialog().getButton(AlertDialog.BUTTON_NEGATIVE).performClick();
            ShadowLooper.idleMainLooper();
            assertTrue(activity.isFinishing());
            assertNull(LinkGate.takeConfirmed());
        }
        ShadowDialog.reset();
        try (var controller = Robolectric.buildActivity(PairLinkActivity.class, opening(LINK)).setup()) {
            PairLinkActivity activity = controller.get();
            shownDialog().cancel();
            ShadowLooper.idleMainLooper();
            assertTrue(activity.isFinishing());
            assertNull(LinkGate.takeConfirmed());
        }
    }

    @Test
    public void badLinksGetTheExistingRefusalAndNoConfirmation() {
        String huge = LINK + "&x=" + "a".repeat(2 * 1024 * 1024);
        for (String bad : new String[] {LINK.replace("v=1", "v=2"), "rubylight://pair", "rubylight://pair/x?v=1", "rubylight://other?otp=4821", huge}) {
            ShadowDialog.reset();
            try (var controller = Robolectric.buildActivity(PairLinkActivity.class, opening(bad)).setup()) {
                PairLinkActivity activity = controller.get();
                AlertDialog dialog = shownDialog();
                assertEquals(activity.getString(R.string.addpc_qr_invalid), message(dialog));
                assertNotEquals(activity.getString(R.string.pcview_menu_pair_pc), dialog.getButton(AlertDialog.BUTTON_POSITIVE).getText().toString());
                assertFalse(activity.isFinishing());
                dialog.getButton(AlertDialog.BUTTON_POSITIVE).performClick();
                ShadowLooper.idleMainLooper();
                assertTrue("nothing stays behind the refusal", activity.isFinishing());
            }
        }
    }

    @Test
    public void anOldLinkFromRecentsIsNotAskedAboutAgain() {
        Intent again = opening(LINK).addFlags(Intent.FLAG_ACTIVITY_LAUNCHED_FROM_HISTORY);
        try (var controller = Robolectric.buildActivity(PairLinkActivity.class, again).create()) {
            assertNull(ShadowDialog.getLatestDialog());
            assertTrue(controller.get().isFinishing());
        }
    }

    @Test
    public void aNewLinkNeverSwapsTheQuestionThatIsUp() {
        try (var controller = Robolectric.buildActivity(PairLinkActivity.class, opening(LINK)).setup()) {
            AlertDialog first = shownDialog();
            pass(PairLinkActivity.PAIR_DELAY_MS);
            controller.newIntent(opening(LINK.replace("Wohnzimmer", "Erste")));
            controller.newIntent(opening(LINK.replace("Wohnzimmer", "Studio")));
            assertTrue(first.isShowing());
            assertEquals("Wohnzimmer\nAddress: 192.168.4.2:47989", message(shownDialog()));
            assertTrue("Pair still means the one on screen", first.getButton(AlertDialog.BUTTON_POSITIVE).isEnabled());

            // Answered, only the newest comes next, and Pair is off again.
            first.getButton(AlertDialog.BUTTON_NEGATIVE).performClick();
            AlertDialog second = shownDialog();
            assertNotEquals(first, second);
            assertEquals("Studio\nAddress: 192.168.4.2:47989", message(second));
            assertFalse(second.getButton(AlertDialog.BUTTON_POSITIVE).isEnabled());
            second.getButton(AlertDialog.BUTTON_NEGATIVE).performClick();
            ShadowLooper.idleMainLooper();
            assertTrue(controller.get().isFinishing());
        }
    }

    @Test
    public void aLinkDuringAStreamEndsThisScreenAtOnceAndWaits() {
        // Only Game.onStop stops a stream, and a screen that ends in onCreate never comes up over it.
        Object stream = new Object();
        LinkGate.hold(stream);
        try (var controller = Robolectric.buildActivity(PairLinkActivity.class, opening(LINK)).create()) {
            assertTrue(controller.get().isFinishing());
            assertNull("nothing comes up", ShadowDialog.getLatestDialog());
            assertNull("the app's task is left alone", Shadows.shadowOf(controller.get()).getNextStartedActivity());
            assertNull(nextStarted());
        }
        LinkGate.release(stream, RuntimeEnvironment.getApplication());
        Intent asks = nextStarted();
        assertEquals(PairLinkActivity.class.getName(), asks.getComponent().getClassName());
        try (var controller = Robolectric.buildActivity(PairLinkActivity.class, asks).setup()) {
            assertEquals("Wohnzimmer\nAddress: 192.168.4.2:47989", message(shownDialog()));
        }
    }

    @Test
    public void onlyTheNewestLinkWaitsAndOnlyForTheLastHolder() {
        Object pairing = new Object();
        Object request = new Object();
        LinkGate.hold(pairing);
        LinkGate.hold(request);
        LinkGate.put("first");
        LinkGate.put("second");
        LinkGate.release(pairing, RuntimeEnvironment.getApplication());
        assertNull(nextStarted());
        LinkGate.release(request, RuntimeEnvironment.getApplication());
        assertNotNull(nextStarted());
        assertEquals("second", LinkGate.take());
        assertNull(LinkGate.take());
    }

    private static String source(String path) throws Exception {
        return new String(Files.readAllBytes(Paths.get(path)), StandardCharsets.UTF_8);
    }

    private static String activityIn(String manifest, String name) {
        int start = manifest.indexOf("android:name=\"" + name + "\"");
        return manifest.substring(start, manifest.indexOf("</activity>", start));
    }

    @Test
    public void theLinkOpensAScreenOfItsOwnThatLeavesNoTrace() throws Exception {
        String manifest = source("src/main/AndroidManifest.xml");
        String activity = activityIn(manifest, ".preferences.PairLinkActivity");
        assertTrue(activity.contains("android:exported=\"true\""));
        assertTrue(activity.contains("android:noHistory=\"true\""));
        assertTrue(activity.contains("android:excludeFromRecents=\"true\""));
        assertTrue("not in the task with the stream", activity.contains("android:taskAffinity=\"\""));
        assertTrue(activity.contains("android:launchMode=\"singleTask\""));
        assertTrue(activity.contains("android.intent.action.VIEW"));
        assertTrue(activity.contains("android.intent.category.DEFAULT"));
        assertTrue(activity.contains("android.intent.category.BROWSABLE"));
        assertTrue(activity.contains("<data android:scheme=\"rubylight\" android:host=\"pair\" />"));

        String addPc = activityIn(manifest, ".preferences.AddComputerManually");
        assertTrue(addPc.contains("android:exported=\"false\""));
        assertFalse(addPc.contains("launchMode"));
        assertFalse(addPc.contains("intent-filter"));
    }

    @Test
    public void theStreamScreenHoldsLinksForExactlyAsLongAsItRuns() throws Exception {
        String game = source("src/main/java/com/limelight/Game.java");
        int start = game.indexOf("protected void onStart()");
        int stop = game.indexOf("protected void onStop()");
        assertTrue(game.indexOf("LinkGate.hold(this)", start) < game.indexOf("}", game.indexOf("super.onStart()", start)));
        assertTrue(game.indexOf("LinkGate.release(this, this)", stop) < game.indexOf("}", game.indexOf("super.onStop()", stop)));
    }
}
