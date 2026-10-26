package com.limelight.computers;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertSame;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.os.Looper;
import android.widget.TextView;

import androidx.appcompat.app.AlertDialog;

import com.limelight.R;
import com.limelight.nativeapi.HostEndpoint;
import com.limelight.nativeapi.NativeApiClient;
import com.limelight.nativeapi.NativeApiException;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.ui.ToolbarIconTintTest;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.Shadows;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowDialog;
import org.robolectric.shadows.ShadowToast;

import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.atomic.AtomicLong;

/** Sleep PC's handshake against scripted native_api.rs envelopes, as power.rs and the JNI op produce them. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class SleepFlowTest {
    private static final String TOKEN_A = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    private static final String TOKEN_B = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";
    private static final String BUSY = "End all applications and sessions before requesting sleep";
    private static final String SPENT = "Sleep token expired or already used; confirm the action again";

    private final Map<String, ArrayDeque<String>> replies = new HashMap<>();
    private final List<String> calls = new ArrayList<>();
    private final AtomicLong clock = new AtomicLong(1_000_000);
    private final Recorder ui = new Recorder();

    private static String reply(int status, String message, boolean supported, boolean allowed, String token,
                                boolean accepted) {
        return "{\"status\":" + status + ",\"message\":\"" + message + "\",\"version\":1,\"sleep_supported\":" +
                supported + ",\"sleep_allowed\":" + allowed + ",\"sleep_available\":" + (token != null) +
                ",\"active_session_policy\":\"idle-only\",\"token\":" + (token == null ? "null" : "\"" + token + "\"") +
                ",\"token_lifetime_seconds\":30,\"accepted\":" + accepted + "}";
    }

    private static String offer(String token) {
        return "{\"ok\":" + reply(200, "", true, true, token, false) + "}";
    }

    private static final String ACCEPTED =
            "{\"ok\":" + reply(202, "Sleep request accepted; native completion is not confirmed", true, true, null, true) + "}";

    private static String refused(int status, String refusal, String message) {
        return "{\"error\":{\"kind\":\"power_refused\",\"message\":\"" + message + "\",\"http_status\":" + status +
                ",\"refusal\":\"" + refusal + "\",\"power\":" + reply(status, message, true, true, null, false) + "}}";
    }

    private static final String UNREACHABLE = "{\"error\":{\"kind\":\"network\",\"message\":\"connection refused\"}}";

    private void script(String op, String... envelopes) {
        replies.computeIfAbsent(op, k -> new ArrayDeque<>()).addAll(Arrays.asList(envelopes));
    }

    private NativeApiClient client() {
        return new NativeApiClient((host, op, argument) -> {
            calls.add(op.equals("power.sleep") ? op + " " + argument : op);
            ArrayDeque<String> queue = replies.get(op);
            if (queue == null || queue.isEmpty()) {
                throw new AssertionError("Unexpected " + op);
            }
            return queue.poll();
        }, new HostEndpoint("pc", "10.0.0.2", 47989, 47984, "pem", "unique", new byte[1], new byte[1]));
    }

    /** Runs host calls and UI callbacks inline. */
    private SleepFlow flow() {
        NativeApiClient api = client();
        return new SleepFlow(() -> api, ui, Runnable::run, Runnable::run, clock::get);
    }

    private static final class Recorder implements SleepFlow.Ui {
        final List<String> events = new ArrayList<>();
        Runnable confirm;
        NativeApiClient.PowerReply unavailable;
        String refusedMessage;
        int refusedStatus;
        NativeApiException failure;

        @Override public void confirm(Runnable sleep) {
            events.add("confirm");
            confirm = sleep;
        }

        @Override public void accepted() {
            events.add("accepted");
        }

        @Override public void unavailable(NativeApiClient.PowerReply offer) {
            events.add("unavailable");
            unavailable = offer;
        }

        @Override public void refused(String message, int httpStatus) {
            events.add("refused");
            refusedMessage = message;
            refusedStatus = httpStatus;
        }

        @Override public void failed(NativeApiException error) {
            events.add("failed");
            failure = error;
        }
    }

    @Test
    public void aFreshTokenIsSpentAsOfferedAndA202IsAccepted() {
        script("power.capabilities", offer(TOKEN_A));
        script("power.sleep", ACCEPTED);
        flow().start();
        assertEquals(Arrays.asList("confirm"), ui.events);
        assertEquals(Arrays.asList("power.capabilities"), calls);

        clock.addAndGet(SleepFlow.TOKEN_REFRESH_MS - 1);
        ui.confirm.run();
        assertEquals(Arrays.asList("power.capabilities", "power.sleep " + TOKEN_A), calls);
        assertEquals(Arrays.asList("confirm", "accepted"), ui.events);
    }

    @Test
    public void aSecondConfirmationSpendsNothing() {
        script("power.capabilities", offer(TOKEN_A));
        script("power.sleep", ACCEPTED);
        flow().start();
        ui.confirm.run();
        ui.confirm.run();
        assertEquals(Arrays.asList("power.capabilities", "power.sleep " + TOKEN_A), calls);
        assertEquals(Arrays.asList("confirm", "accepted"), ui.events);
    }

    @Test
    public void aTokenOf25SecondsOrOlderIsFetchedAgainBeforeItIsSpent() {
        script("power.capabilities", offer(TOKEN_A), offer(TOKEN_B));
        script("power.sleep", ACCEPTED);
        flow().start();
        clock.addAndGet(SleepFlow.TOKEN_REFRESH_MS);
        ui.confirm.run();
        assertEquals(25_000, SleepFlow.TOKEN_REFRESH_MS);
        assertEquals(Arrays.asList("power.capabilities", "power.capabilities", "power.sleep " + TOKEN_B), calls);
        assertEquals(Arrays.asList("confirm", "accepted"), ui.events);
    }

    @Test
    public void aRefreshWithoutATokenStopsBeforeThePost() {
        script("power.capabilities", offer(TOKEN_A), "{\"ok\":" + reply(200, "", true, true, null, false) + "}");
        flow().start();
        clock.addAndGet(60_000);
        ui.confirm.run();
        assertEquals(Arrays.asList("power.capabilities", "power.capabilities"), calls);
        assertEquals(Arrays.asList("confirm", "unavailable"), ui.events);
        assertTrue(ui.unavailable.sleepSupported && ui.unavailable.sleepAllowed);
        assertFalse(ui.unavailable.sleepAvailable);
    }

    @Test
    public void anOfferWithoutATokenAsksNothingAndSaysWhy() {
        script("power.capabilities", "{\"ok\":" + reply(200, "", true, false, null, false) + "}");
        flow().start();
        assertEquals(Arrays.asList("unavailable"), ui.events);
        assertNull(ui.confirm);
        assertFalse(ui.unavailable.sleepAllowed);
        assertEquals(Arrays.asList("power.capabilities"), calls);
    }

    @Test
    public void theHostsRefusalReachesThePersonVerbatim() {
        script("power.capabilities", offer(TOKEN_A));
        script("power.sleep", refused(409, "conflict", BUSY));
        flow().start();
        ui.confirm.run();
        assertEquals(Arrays.asList("confirm", "refused"), ui.events);
        assertEquals(BUSY, ui.refusedMessage);
        assertEquals(409, ui.refusedStatus);

        Recorder spent = new Recorder();
        script("power.capabilities", offer(TOKEN_A));
        script("power.sleep", refused(409, "conflict", SPENT));
        NativeApiClient api = client();
        new SleepFlow(() -> api, spent, Runnable::run, Runnable::run, clock::get).start();
        spent.confirm.run();
        assertEquals(SPENT, spent.refusedMessage);
    }

    @Test
    public void aRefusedOfferAndAnEmptyMessageStillReachThePerson() {
        script("power.capabilities", refused(501, "not_implemented", ""));
        flow().start();
        assertEquals(Arrays.asList("refused"), ui.events);
        assertEquals("", ui.refusedMessage);
        assertEquals(501, ui.refusedStatus);
        assertNull(ui.confirm);
    }

    @Test
    public void transportErrorsFailLoudlyBeforeAndAfterTheConfirmation() {
        script("power.capabilities", UNREACHABLE);
        flow().start();
        assertEquals(Arrays.asList("failed"), ui.events);
        assertEquals(NativeApiException.Kind.NETWORK, ui.failure.getKind());
        assertNull(ui.confirm);

        Recorder late = new Recorder();
        script("power.capabilities", offer(TOKEN_A));
        script("power.sleep", UNREACHABLE);
        NativeApiClient api = client();
        new SleepFlow(() -> api, late, Runnable::run, Runnable::run, clock::get).start();
        late.confirm.run();
        assertEquals(Arrays.asList("confirm", "failed"), late.events);
        assertEquals(NativeApiException.Kind.NETWORK, late.failure.getKind());

        Recorder noEndpoint = new Recorder();
        new SleepFlow(() -> {
            throw new NativeApiException(NativeApiException.Kind.NOT_PAIRED, "no endpoint");
        }, noEndpoint, Runnable::run, Runnable::run, clock::get).start();
        assertEquals(Arrays.asList("failed"), noEndpoint.events);
        assertEquals(NativeApiException.Kind.NOT_PAIRED, noEndpoint.failure.getKind());
    }

    private static AlertDialog latestDialog() {
        Shadows.shadowOf(Looper.getMainLooper()).idle();
        return (AlertDialog) ShadowDialog.getLatestDialog();
    }

    private static String message(AlertDialog dialog) {
        return ((TextView) dialog.findViewById(android.R.id.message)).getText().toString();
    }

    private static final HostActions.Callbacks CALLBACKS = new HostActions.Callbacks() {
        @Override public ComputerManagerService.ComputerManagerBinder binder() { return null; }
        @Override public boolean isForeground() { return true; }
        @Override public void onPaired(ComputerDetails computer) { }
        @Override public void onRemoved(ComputerDetails computer) { }
        @Override public void onRenamed(ComputerDetails computer) { }
        @Override public void onWakeSent(ComputerDetails computer) { }
        @Override public void pausePolling() { }
        @Override public void resumePolling() { }
    };

    private static ComputerDetails office() {
        ComputerDetails pc = new ComputerDetails();
        pc.uuid = "pc";
        pc.name = "Office";
        return pc;
    }

    private static ActivityController<ToolbarIconTintTest.Screen> screen() {
        return Robolectric.buildActivity(ToolbarIconTintTest.Screen.class).setup().visible();
    }

    @Test
    public void theScreenConfirmsShowsTheHostsWordsAndOursOnlyWhenItSentNone() {
        try (ActivityController<ToolbarIconTintTest.Screen> controller = screen()) {
            ToolbarIconTintTest.Screen screen = controller.get();
            HostActions actions = new HostActions(screen, CALLBACKS);
            NativeApiClient api = client();

            script("power.capabilities", offer(TOKEN_A));
            script("power.sleep", ACCEPTED);
            ArrayDeque<Runnable> worker = new ArrayDeque<>();
            actions.startSleep(office(), () -> api, worker::add);
            AlertDialog asking = latestDialog();
            assertEquals(screen.getString(R.string.host_sleep_asking), message(asking));
            worker.poll().run();
            AlertDialog confirm = latestDialog();
            assertFalse("The progress gives way to the question", asking.isShowing());
            assertEquals(screen.getString(R.string.host_sleep_message), message(confirm));
            assertEquals(Arrays.asList("power.capabilities"), calls);
            confirm.getButton(AlertDialog.BUTTON_POSITIVE).performClick();
            asking = latestDialog();
            assertEquals(screen.getString(R.string.host_sleep_asking), message(asking));
            worker.poll().run();
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            assertFalse(asking.isShowing());
            assertEquals(Arrays.asList("power.capabilities", "power.sleep " + TOKEN_A), calls);
            assertEquals(screen.getString(R.string.host_sleep_accepted), ShadowToast.getTextOfLatestToast());

            SleepFlow.Ui shown = actions.sleepUi(office());
            shown.refused(BUSY, 409);
            assertEquals(BUSY, message(latestDialog()));
            shown.refused("", 501);
            assertEquals(screen.getString(R.string.host_sleep_refused, 501), message(latestDialog()));
            shown.failed(new NativeApiException(NativeApiException.Kind.NETWORK, "connection refused"));
            assertEquals(screen.getString(R.string.host_sleep_failed), message(latestDialog()));

            script("power.capabilities", "{\"ok\":" + reply(200, "", true, false, null, false) + "}");
            actions.startSleep(office(), () -> api, Runnable::run);
            assertEquals(screen.getString(R.string.host_feature_reason_sleep_permission), message(latestDialog()));
            script("power.capabilities", "{\"ok\":" + reply(200, "", true, true, null, false) + "}");
            actions.startSleep(office(), () -> api, Runnable::run);
            assertEquals(screen.getString(R.string.host_sleep_busy), message(latestDialog()));
        }
    }

    @Test
    public void aSecondSleepReplacesTheFirstWhoseAnswersAreDropped() {
        try (ActivityController<ToolbarIconTintTest.Screen> controller = screen()) {
            ToolbarIconTintTest.Screen screen = controller.get();
            HostActions actions = new HostActions(screen, CALLBACKS);
            NativeApiClient api = client();
            script("power.capabilities", offer(TOKEN_B), refused(409, "conflict", BUSY));
            ArrayDeque<Runnable> worker = new ArrayDeque<>();
            actions.startSleep(office(), () -> api, worker::add);
            AlertDialog firstAsking = latestDialog();
            actions.startSleep(office(), () -> api, worker::add);
            AlertDialog secondAsking = latestDialog();
            assertFalse("One progress at a time", firstAsking.isShowing());
            assertTrue(secondAsking.isShowing());

            // The second flow's GET answers first, with the token the host now holds for this client.
            Runnable first = worker.poll();
            worker.poll().run();
            AlertDialog confirm = latestDialog();
            assertEquals(screen.getString(R.string.host_sleep_message), message(confirm));
            first.run();
            assertSame("The replaced flow's refusal is not shown", confirm, latestDialog());
            assertTrue(confirm.isShowing());
            assertEquals(Arrays.asList("power.capabilities", "power.capabilities"), calls);
        }
    }

    @Test
    public void cancellingTheProgressDropsWhatTheHostSaysLater() {
        try (ActivityController<ToolbarIconTintTest.Screen> controller = screen()) {
            HostActions actions = new HostActions(controller.get(), CALLBACKS);
            NativeApiClient api = client();
            script("power.capabilities", offer(TOKEN_A));
            ArrayDeque<Runnable> worker = new ArrayDeque<>();
            actions.startSleep(office(), () -> api, worker::add);
            AlertDialog asking = latestDialog();
            asking.getButton(AlertDialog.BUTTON_NEGATIVE).performClick();
            worker.poll().run();
            assertSame("No confirmation after Cancel", asking, latestDialog());
            assertFalse(asking.isShowing());
        }
    }

    @Test
    public void aResultAfterTheScreenClosedIsAToastNotLost() {
        script("power.capabilities", offer(TOKEN_A));
        script("power.sleep", refused(409, "conflict", BUSY));
        NativeApiClient api = client();
        ArrayDeque<Runnable> worker = new ArrayDeque<>();
        HostActions actions;
        try (ActivityController<ToolbarIconTintTest.Screen> controller = screen()) {
            actions = new HostActions(controller.get(), CALLBACKS);
            actions.startSleep(office(), () -> api, worker::add);
            worker.poll().run();
            latestDialog().getButton(AlertDialog.BUTTON_POSITIVE).performClick();
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            actions.destroy();
        }
        ShadowDialog.reset();
        worker.poll().run();
        Shadows.shadowOf(Looper.getMainLooper()).idle();
        assertEquals(BUSY, ShadowToast.getTextOfLatestToast());
        assertNull("No dialog on a closed screen", ShadowDialog.getLatestDialog());

        // An offer that arrives after the screen closed asks nothing and sends nothing.
        script("power.capabilities", offer(TOKEN_B));
        try (ActivityController<ToolbarIconTintTest.Screen> controller = screen()) {
            actions = new HostActions(controller.get(), CALLBACKS);
            actions.startSleep(office(), () -> api, worker::add);
            actions.destroy();
        }
        ShadowDialog.reset();
        worker.poll().run();
        Shadows.shadowOf(Looper.getMainLooper()).idle();
        assertNull(ShadowDialog.getLatestDialog());
        assertEquals(Arrays.asList("power.capabilities", "power.sleep " + TOKEN_A, "power.capabilities"), calls);
    }

    @Test
    public void hostCallsRunOnTheWorkerAndResultsOnTheMainThread() {
        script("power.capabilities", offer(TOKEN_A));
        script("power.sleep", ACCEPTED);
        ArrayDeque<Runnable> worker = new ArrayDeque<>();
        ArrayDeque<Runnable> main = new ArrayDeque<>();
        NativeApiClient api = client();
        new SleepFlow(() -> api, ui, worker::add, main::add, clock::get).start();
        assertTrue(calls.isEmpty());
        worker.poll().run();
        assertEquals(Arrays.asList("power.capabilities"), calls);
        assertTrue(ui.events.isEmpty());
        main.poll().run();
        assertNotNull(ui.confirm);

        ui.confirm.run();
        assertEquals(1, calls.size());
        worker.poll().run();
        assertEquals(2, calls.size());
        assertEquals(Arrays.asList("confirm"), ui.events);
        main.poll().run();
        assertEquals(Arrays.asList("confirm", "accepted"), ui.events);
        assertTrue(worker.isEmpty() && main.isEmpty());
    }
}
