package com.limelight.computers;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.os.Looper;
import android.text.Spanned;
import android.text.style.StyleSpan;
import android.view.KeyEvent;
import android.view.View;
import android.view.ViewGroup;
import android.widget.TextView;

import androidx.appcompat.app.AlertDialog;

import com.google.android.material.button.MaterialButton;
import com.limelight.R;
import com.limelight.grid.PcGridAdapter;
import com.limelight.nativeapi.HostEndpoint;
import com.limelight.nativeapi.KnownCapabilities;
import com.limelight.nativeapi.NativeApiClient;
import com.limelight.nativeapi.NativeApiException;
import com.limelight.nativeapi.NativePairing;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.TestHosts;
import com.limelight.ui.ActionSheet;
import com.limelight.ui.ToolbarIconTintTest;

import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.Shadows;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;

import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.concurrent.atomic.AtomicLong;

/**
 * Approving a device from this one: who gets the row and the banner, how often Home asks, and the PIN dialog a gamepad
 * can fill in. Refusals stay in the host's words.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class PairingApprovalTest {
    private static final NativePairing.Request TV = new NativePairing.Request("a1", "TV", "192.168.4.116", 3);

    private static ComputerDetails pc(String uuid) {
        ComputerDetails details = new ComputerDetails();
        details.uuid = uuid;
        details.name = "PC";
        details.state = ComputerDetails.State.ONLINE;
        details.activeAddress = new ComputerDetails.AddressTuple("10.0.0.2", 47989);
        details.caps = TestHosts.caps("<root status_code=\"200\"><RustHostVersion>3.6.0</RustHostVersion>"
                + "<PairStatus>1</PairStatus><NativeApiVersion>1</NativeApiVersion></root>");
        return details;
    }

    private static String capabilities(String features, boolean hostControl) {
        return capabilities(features, hostControl, false);
    }

    private static String capabilities(String features, boolean hostControl, boolean pairingApprove) {
        return "{\"ok\":{\"api_version\":1,\"host\":{\"name\":\"PC\",\"version\":\"3.6.0\",\"unique_id\":\"U\"},"
                + "\"features\":[" + features + "],\"permissions\":{\"list\":true,\"view\":true,\"launch\":true,"
                + "\"host_control\":" + hostControl + ",\"restart_shutdown\":false,\"pairing_approve\":" + pairingApprove + "}}}";
    }

    private static NativeApiClient api(NativeApiTransportReply reply) {
        return new NativeApiClient((host, op, argument) -> reply.answer(op, argument),
                new HostEndpoint("pc", "10.0.0.2", 47989, 47984, "pem", "unique", new byte[1], new byte[1]));
    }

    private interface NativeApiTransportReply {
        String answer(String op, String argument);
    }

    private static void read(String envelope) throws Exception {
        api((op, argument) -> envelope).capabilities();
    }

    @Before
    public void forget() {
        KnownCapabilities.clear();
    }

    @Test
    public void onlyAPcOfferingItToADeviceWithAGrantIsAsked() throws Exception {
        assertFalse(HostMenu.mayApprovePairing(pc("pc")));
        read(capabilities("\"update\"", true));
        assertFalse("an older host", HostMenu.mayApprovePairing(pc("pc")));
        read(capabilities("\"pairing.approve\"", false));
        assertFalse("no grant", HostMenu.mayApprovePairing(pc("pc")));
        read(capabilities("\"pairing.approve\"", false, true));
        assertTrue("Approve new devices", HostMenu.mayApprovePairing(pc("pc")));
        read(capabilities("\"pairing.approve\"", true));
        assertTrue(HostMenu.mayApprovePairing(pc("pc")));
        ComputerDetails asleep = pc("pc");
        asleep.state = ComputerDetails.State.OFFLINE;
        assertFalse(HostMenu.mayApprovePairing(asleep));
    }

    @Test
    public void theMenuRowFollowsTheSameRule() throws Exception {
        try (ActivityController<ToolbarIconTintTest.Screen> controller =
                     Robolectric.buildActivity(ToolbarIconTintTest.Screen.class).setup().visible()) {
            ToolbarIconTintTest.Screen screen = controller.get();
            String label = screen.getString(R.string.host_menu_pair_another);
            List<Integer> chosen = new ArrayList<>();
            read(capabilities("\"pairing.approve\"", false));
            String reason = screen.getString(R.string.host_feature_reason_pairing_approve_permission);
            ActionSheet without = HostMenu.build(screen, pc("pc"), PcGridAdapter.RowState.READY, null, chosen::add);
            without.show();
            View content = without.findViewById(android.R.id.content);
            MaterialButton dimmed = row(content, label);
            assertNotNull("Findable without the grant", dimmed);
            assertEquals(HostMenu.DISABLED_ALPHA, dimmed.getAlpha(), 0f);
            assertTrue("It names the permission", reason.contains("Approve new devices"));
            assertTrue("It points to the Devices page", reason.contains("Devices"));
            assertTrue(containsText(content, reason));
            dimmed.performClick();
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            assertEquals(List.of(HostMenu.PAIR_ANOTHER), chosen);
            chosen.clear();

            read(capabilities("\"update\"", true));
            ActionSheet older = HostMenu.build(screen, pc("pc"), PcGridAdapter.RowState.READY, null, chosen::add);
            older.show();
            assertNull("A host that offers nothing lists nothing", row(older.findViewById(android.R.id.content), label));
            older.dismiss();

            for (String granted : List.of(capabilities("\"pairing.approve\"", true),
                    capabilities("\"pairing.approve\"", false, true))) {
                read(granted);
                ActionSheet with = HostMenu.build(screen, pc("pc"), PcGridAdapter.RowState.READY, null, chosen::add);
                with.show();
                MaterialButton entry = row(with.findViewById(android.R.id.content), label);
                assertNotNull(entry);
                assertEquals(1f, entry.getAlpha(), 0f);
                assertFalse(containsText(with.findViewById(android.R.id.content), reason));
                entry.performClick();
                Shadows.shadowOf(Looper.getMainLooper()).idle();
                assertEquals(List.of(HostMenu.PAIR_ANOTHER), chosen);
                chosen.clear();
                with.dismiss();
            }
        }
    }

    private static boolean containsText(View view, String text) {
        if (view instanceof android.widget.TextView
                && ((android.widget.TextView) view).getText().toString().contains(text)) {
            return true;
        }
        if (view instanceof ViewGroup) {
            ViewGroup group = (ViewGroup) view;
            for (int i = 0; i < group.getChildCount(); i++) {
                if (containsText(group.getChildAt(i), text)) {
                    return true;
                }
            }
        }
        return false;
    }

    private static MaterialButton row(View view, String text) {
        if (view instanceof MaterialButton && ((MaterialButton) view).getText().toString().startsWith(text)) {
            return (MaterialButton) view;
        }
        if (view instanceof ViewGroup) {
            ViewGroup group = (ViewGroup) view;
            for (int i = 0; i < group.getChildCount(); i++) {
                MaterialButton found = row(group.getChildAt(i), text);
                if (found != null) {
                    return found;
                }
            }
        }
        return null;
    }

    // The watch

    private final AtomicLong now = new AtomicLong(100_000);
    private final List<String> reads = new ArrayList<>();
    private final List<List<PairingWatch.Waiting>> published = new ArrayList<>();

    private PairingWatch watch(PairingWatch.Reader reader, boolean asks) {
        return new PairingWatch(host -> {
            reads.add(host.uuid);
            return reader.read(host);
        }, host -> asks, Runnable::run, Runnable::run, now::get, published::add);
    }

    @Test
    public void aPcNotAskedIsNeverRead() {
        PairingWatch watch = watch(host -> List.of(TV), false);
        watch.poll(List.of(pc("pc")));
        assertTrue(reads.isEmpty());
        assertTrue(published.isEmpty());
    }

    @Test
    public void readsAtMostEveryFewSecondsAndTellsOnlyChanges() {
        PairingWatch watch = watch(host -> List.of(TV), true);
        watch.poll(List.of(pc("pc")));
        watch.poll(List.of(pc("pc")));
        assertEquals(1, reads.size());
        assertEquals(1, published.size());
        assertEquals("TV", published.get(0).get(0).request.name);
        now.addAndGet(4000);
        watch.poll(List.of(pc("pc")));
        assertEquals(2, reads.size());
        assertEquals("the same request again", 1, published.size());
    }

    @Test
    public void aSettledRequestReadsAtOnceAndTheBannerGoes() {
        List<NativePairing.Request> answer = new ArrayList<>(List.of(TV));
        PairingWatch watch = watch(host -> new ArrayList<>(answer), true);
        watch.poll(List.of(pc("pc")));
        answer.clear();
        watch.refreshSoon();
        watch.poll(List.of(pc("pc")));
        assertEquals(2, reads.size());
        assertTrue(published.get(1).isEmpty());
    }

    @Test
    public void aPcThatStopsAnsweringDropsOut() {
        boolean[] down = {false};
        PairingWatch watch = watch(host -> {
            if (down[0]) {
                throw new NativeApiException(NativeApiException.Kind.NETWORK, "timed out");
            }
            return List.of(TV);
        }, true);
        watch.poll(List.of(pc("pc")));
        down[0] = true;
        now.addAndGet(4000);
        watch.poll(List.of(pc("pc")));
        assertEquals(2, published.size());
        assertTrue(published.get(1).isEmpty());
    }

    @Test
    public void anAnswerAfterTheScreenLeftIsDropped() {
        List<Runnable> answers = new ArrayList<>();
        PairingWatch watch = new PairingWatch(host -> List.of(TV), host -> true, Runnable::run, answers::add, now::get,
                published::add);
        watch.poll(List.of(pc("pc")));
        watch.pause();
        answers.get(0).run();
        assertTrue(published.isEmpty());
        // Coming back reads at once, and that answer counts.
        watch.poll(List.of(pc("pc")));
        answers.get(1).run();
        assertEquals(1, published.size());
    }

    @Test
    public void losingTheCapabilityTakesTheBannerAwayWithoutAsking() {
        boolean[] asks = {true};
        PairingWatch watch = new PairingWatch(host -> {
            reads.add(host.uuid);
            return List.of(TV);
        }, host -> asks[0], Runnable::run, Runnable::run, now::get, published::add);
        watch.poll(List.of(pc("pc")));
        asks[0] = false;
        now.addAndGet(4000);
        watch.poll(List.of(pc("pc")));
        assertEquals(1, reads.size());
        assertEquals(Collections.emptyList(), published.get(1));
    }

    // The dialog

    private static final class Transcript {
        final List<String> calls = new ArrayList<>();
        final List<String> errors = new ArrayList<>();
        int changed;
    }

    private PairingDialogs dialogs(ToolbarIconTintTest.Screen screen, Transcript transcript) {
        return new PairingDialogs(screen, (title, message) -> transcript.errors.add(title + ": " + message),
                Runnable::run, Runnable::run, () -> transcript.changed++);
    }

    private static SleepFlow.Client client(Transcript transcript, String approveReply) {
        return () -> api((op, argument) -> {
            transcript.calls.add(op + " " + argument);
            return op.equals("pairing.approve") ? approveReply : "{\"ok\":{\"requests\":[]}}";
        });
    }

    private static PairingWatch.Waiting tv() {
        return new PairingWatch.Waiting(pc("pc"), TV);
    }

    private static View digit(AlertDialog dialog, int digit) {
        ViewGroup pad = dialog.findViewById(R.id.pairing_pin_pad);
        for (int i = 0; i < pad.getChildCount(); i++) {
            View child = pad.getChildAt(i);
            if (String.valueOf(digit).contentEquals(((MaterialButton) child).getText())) {
                return child;
            }
        }
        throw new AssertionError("no digit " + digit);
    }

    private static void idle() {
        Shadows.shadowOf(Looper.getMainLooper()).idle();
    }

    @Test
    public void theAddressStandsOutInThePinDialog() {
        try (ActivityController<ToolbarIconTintTest.Screen> controller =
                     Robolectric.buildActivity(ToolbarIconTintTest.Screen.class).setup().visible()) {
            Transcript transcript = new Transcript();
            AlertDialog dialog = dialogs(controller.get(), transcript).showPin(tv(), client(transcript, ""));
            idle();
            CharSequence from = ((TextView) dialog.findViewById(R.id.pairing_pin_from)).getText();
            Spanned spanned = (Spanned) from;
            StyleSpan[] bold = spanned.getSpans(0, from.length(), StyleSpan.class);
            assertEquals(1, bold.length);
            assertEquals("192.168.4.116",
                    from.subSequence(spanned.getSpanStart(bold[0]), spanned.getSpanEnd(bold[0])).toString());
            dialog.dismiss();
        }
    }

    @Test
    public void everyPadKeyTakesTheFocusSoAGamepadCanReachIt() {
        try (ActivityController<ToolbarIconTintTest.Screen> controller =
                     Robolectric.buildActivity(ToolbarIconTintTest.Screen.class).setup().visible()) {
            Transcript transcript = new Transcript();
            AlertDialog dialog = dialogs(controller.get(), transcript).showPin(tv(), client(transcript, ""));
            idle();
            ViewGroup pad = dialog.findViewById(R.id.pairing_pin_pad);
            assertEquals(11, pad.getChildCount());
            for (int i = 0; i < pad.getChildCount(); i++) {
                assertTrue(pad.getChildAt(i).isFocusable());
            }
            assertTrue("the pad opens focused", pad.getChildAt(0).hasFocus());
            assertFalse(dialog.getButton(AlertDialog.BUTTON_POSITIVE).isEnabled());
            dialog.dismiss();
        }
    }

    @Test
    public void theRightPinPairsAndApproveWaitsForAllFourDigits() {
        try (ActivityController<ToolbarIconTintTest.Screen> controller =
                     Robolectric.buildActivity(ToolbarIconTintTest.Screen.class).setup().visible()) {
            Transcript transcript = new Transcript();
            AlertDialog dialog = dialogs(controller.get(), transcript)
                    .showPin(tv(), client(transcript, "{\"ok\":{\"name\":\"TV\"}}"));
            idle();
            digit(dialog, 1).performClick();
            digit(dialog, 2).performClick();
            digit(dialog, 3).performClick();
            assertFalse(dialog.getButton(AlertDialog.BUTTON_POSITIVE).isEnabled());
            digit(dialog, 4).performClick();
            digit(dialog, 5).performClick();
            assertEquals("1234", ((TextView) dialog.findViewById(R.id.pairing_pin_value)).getText().toString());
            assertTrue(dialog.getButton(AlertDialog.BUTTON_POSITIVE).isEnabled());
            dialog.getButton(AlertDialog.BUTTON_POSITIVE).performClick();
            idle();
            assertEquals(List.of("pairing.approve {\"id\":\"a1\",\"pin\":\"1234\"}"), transcript.calls);
            assertTrue(transcript.errors.isEmpty());
            assertEquals(1, transcript.changed);
            assertFalse(dialog.isShowing());
        }
    }

    @Test
    public void digitKeysTypeAndDeleteErases() {
        try (ActivityController<ToolbarIconTintTest.Screen> controller =
                     Robolectric.buildActivity(ToolbarIconTintTest.Screen.class).setup().visible()) {
            Transcript transcript = new Transcript();
            AlertDialog dialog = dialogs(controller.get(), transcript).showPin(tv(), client(transcript, ""));
            idle();
            key(dialog, KeyEvent.KEYCODE_7);
            key(dialog, KeyEvent.KEYCODE_NUMPAD_8);
            key(dialog, KeyEvent.KEYCODE_DEL);
            assertEquals("7", ((TextView) dialog.findViewById(R.id.pairing_pin_value)).getText().toString());
            dialog.dismiss();
        }
    }

    private static void key(AlertDialog dialog, int code) {
        dialog.dispatchKeyEvent(new KeyEvent(KeyEvent.ACTION_DOWN, code));
        dialog.dispatchKeyEvent(new KeyEvent(KeyEvent.ACTION_UP, code));
    }

    @Test
    public void aWrongPinShowsTheHostsWordsAndClosesTheDialog() {
        try (ActivityController<ToolbarIconTintTest.Screen> controller =
                     Robolectric.buildActivity(ToolbarIconTintTest.Screen.class).setup().visible()) {
            Transcript transcript = new Transcript();
            String refused = "{\"error\":{\"kind\":\"api\",\"message\":\"That is not the PIN TV shows.\","
                    + "\"http_status\":422,\"code\":\"wrong_pin\"}}";
            AlertDialog dialog = dialogs(controller.get(), transcript).showPin(tv(), client(transcript, refused));
            idle();
            for (int digit : new int[] {9, 9, 9, 9}) {
                digit(dialog, digit).performClick();
            }
            dialog.getButton(AlertDialog.BUTTON_POSITIVE).performClick();
            idle();
            assertEquals(1, transcript.errors.size());
            assertTrue(transcript.errors.get(0), transcript.errors.get(0).endsWith("That is not the PIN TV shows."));
            assertEquals("the list reads again", 1, transcript.changed);
            assertFalse(dialog.isShowing());
        }
    }

    @Test
    public void denyEndsTheRequestWithoutAPin() {
        try (ActivityController<ToolbarIconTintTest.Screen> controller =
                     Robolectric.buildActivity(ToolbarIconTintTest.Screen.class).setup().visible()) {
            Transcript transcript = new Transcript();
            AlertDialog dialog = dialogs(controller.get(), transcript).showPin(tv(), client(transcript, ""));
            idle();
            dialog.getButton(AlertDialog.BUTTON_NEGATIVE).performClick();
            idle();
            assertEquals(List.of("pairing.deny a1"), transcript.calls);
            assertEquals(1, transcript.changed);
            assertFalse(dialog.isShowing());
        }
    }

    @Test
    public void anEmptyListSaysWhatToDoOnTheNewDevice() {
        try (ActivityController<ToolbarIconTintTest.Screen> controller =
                     Robolectric.buildActivity(ToolbarIconTintTest.Screen.class).setup().visible()) {
            Transcript transcript = new Transcript();
            dialogs(controller.get(), transcript).showRequests(pc("pc"), client(transcript, ""));
            idle();
            assertEquals(List.of("pairing.requests "), transcript.calls);
            AlertDialog shown = (AlertDialog) org.robolectric.shadows.ShadowDialog.getLatestDialog();
            TextView message = shown.findViewById(android.R.id.message);
            assertTrue(message.getText().toString(), message.getText().toString().contains("PC"));
            assertTrue(message.getText().toString(), message.getText().toString().contains("Nightjar"));
            shown.dismiss();
        }
    }
}
