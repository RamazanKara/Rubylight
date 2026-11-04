package com.limelight.computers;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.os.Looper;
import android.widget.TextView;

import androidx.appcompat.app.AlertDialog;

import com.limelight.R;
import com.limelight.nativeapi.HostEndpoint;
import com.limelight.nativeapi.KnownUpdates;
import com.limelight.nativeapi.NativeApiClient;
import com.limelight.nativeapi.NativeApiException;
import com.limelight.nativeapi.NativePcPower;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.ui.ToolbarIconTintTest;

import org.junit.Before;
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

/**
 * Restart PC, Shut down PC and Update the PC's host against scripted native_api.rs envelopes, as the pc_power and
 * update ops produce them.
 */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class PcPowerFlowTest {
    private static final String TOKEN_A = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    private static final String TOKEN_B = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";
    private static final String STREAMING = "Living room TV is streaming from this PC.";

    private final Map<String, ArrayDeque<String>> replies = new HashMap<>();
    private final List<String> calls = new ArrayList<>();
    private final AtomicLong clock = new AtomicLong(1_000_000);

    private static String offer(String token, String reason, boolean wakeFromOff) {
        return offer(token, reason, wakeFromOff, null);
    }

    private static String offer(String token, String reason, boolean wakeFromOff, String lastRefusal) {
        String availability = "{\"available\":" + (token != null) + ",\"reason\":"
                + (reason == null ? "null" : "\"" + reason + "\"") + "}";
        return "{\"ok\":{\"restart\":" + availability + ",\"shutdown\":" + availability + ",\"token\":"
                + (token == null ? "null" : "\"" + token + "\"") + ",\"token_lifetime_s\":30,\"wake_from_off\":"
                + wakeFromOff + ",\"last_refusal\":" + (lastRefusal == null ? "null" : "\"" + lastRefusal + "\"")
                + "}}";
    }

    /** GET /power after the PC took the request, with Windows' late refusal. */
    private static String refusedAfter(String refusal) {
        return offer(null, STREAMING, true, refusal);
    }

    private static final String WINDOWS_REFUSED =
            "Windows refused to restart the PC: Access is denied. (exit code 5)";

    private static final String ACCEPTED = "{\"ok\":{}}";

    private static String api(int status, String code, String message) {
        return "{\"error\":{\"kind\":\"api\",\"message\":\"" + message + "\",\"http_status\":" + status
                + ",\"code\":\"" + code + "\",\"setting\":null,\"retry_after_ms\":null}}";
    }

    private static final String UNREACHABLE = "{\"error\":{\"kind\":\"network\",\"message\":\"connection refused\"}}";

    private static String update(String phase) {
        return update(phase, "3.4.0", null, null, false);
    }

    /**
     * An update op's envelope, with the verdicts native_api.rs's update_json adds from pc_power.rs: {@code failure}
     * set means phase failed with those words ("" for none).
     */
    private static String update(String phase, String offered, String waitingFor, String failure, boolean gaveUp) {
        boolean failed = failure != null;
        String words = failure == null || failure.isEmpty() ? null : failure;
        return "{\"ok\":{\"current_version\":\"3.3.0\",\"latest_version\":\"v3.4.0\",\"phase\":\""
                + (failed ? "failed" : phase) + "\",\"install_supported\":true,\"error\":" + quoted(words)
                + ",\"waiting_for\":" + quoted(waitingFor) + ",\"install_now\":true,\"offered\":" + quoted(offered)
                + ",\"installing_now\":" + !phase.equals("idle") + ",\"in_progress\":" + !phase.equals("idle")
                + ",\"failed\":" + failed + ",\"failure\":" + quoted(words) + ",\"gave_up\":" + gaveUp + "}}";
    }

    private static String quoted(String text) {
        return text == null ? "null" : "\"" + text + "\"";
    }

    private void script(String op, String... envelopes) {
        replies.computeIfAbsent(op, k -> new ArrayDeque<>()).addAll(Arrays.asList(envelopes));
    }

    private NativeApiClient client() {
        return new NativeApiClient((host, op, argument) -> {
            calls.add(argument.isEmpty() ? op : op + " " + argument);
            ArrayDeque<String> queue = replies.get(op);
            if (queue == null || queue.isEmpty()) {
                throw new AssertionError("Unexpected " + op);
            }
            return queue.poll();
        }, new HostEndpoint("pc", "10.0.0.2", 47989, 47984, "pem", "unique", new byte[1], new byte[1]));
    }

    private static final class Recorder implements PcPowerFlow.Ui {
        final List<String> events = new ArrayList<>();
        Runnable go;
        Boolean wakeFromOff;
        String said;
        NativeApiException failure;

        @Override public void confirm(boolean wakeFromOff, Runnable go) {
            events.add("confirm");
            this.wakeFromOff = wakeFromOff;
            this.go = go;
        }

        @Override public void accepted() {
            events.add("accepted");
        }

        @Override public void unavailable(String reason) {
            events.add("unavailable");
            said = reason;
        }

        @Override public void refused(String message) {
            events.add("refused");
            said = message;
        }

        @Override public void failed(NativeApiException error) {
            events.add("failed");
            failure = error;
        }

        @Override public void refusedLater(String refusal) {
            events.add("refused later");
            said = refusal;
        }
    }

    private final List<Long> slept = new ArrayList<>();

    private PcPowerFlow flow(NativePcPower.Action action, Recorder ui) {
        NativeApiClient api = client();
        return new PcPowerFlow(action, () -> api, ui, Runnable::run, Runnable::run, clock::get, slept::add);
    }

    @Before
    public void forget() {
        KnownUpdates.clear();
    }

    @Test
    public void eachActionAsksForATokenAndSpendsItOnce() {
        // After each 202 the PC goes down: the refusal check's read fails.
        script("pc_power", offer(TOKEN_A, null, true), UNREACHABLE, offer(TOKEN_B, null, true), UNREACHABLE);
        script("pc_power.restart", ACCEPTED);
        script("pc_power.shutdown", ACCEPTED);
        Recorder restart = new Recorder();
        flow(NativePcPower.Action.RESTART, restart).start();
        assertEquals(List.of("confirm"), restart.events);
        restart.go.run();
        restart.go.run();
        assertEquals(List.of("confirm", "accepted"), restart.events);

        // A new action asks again: the earlier token is never reused.
        Recorder shutdown = new Recorder();
        flow(NativePcPower.Action.SHUT_DOWN, shutdown).start();
        shutdown.go.run();
        assertEquals(List.of("pc_power", "pc_power.restart " + TOKEN_A, "pc_power", "pc_power", "pc_power.shutdown "
                + TOKEN_B, "pc_power"), calls);
        assertEquals("Going down: nothing more to say", List.of("confirm", "accepted"), shutdown.events);
    }

    @Test
    public void aRefusalAfterThePcTookTheRequestIsShownAndAFailedReadStaysQuiet() {
        // Windows refused once the answer had left: the second read says so.
        script("pc_power", offer(TOKEN_A, null, true), refusedAfter(null), refusedAfter(WINDOWS_REFUSED));
        script("pc_power.restart", ACCEPTED);
        Recorder refused = new Recorder();
        flow(NativePcPower.Action.RESTART, refused).start();
        refused.go.run();
        assertEquals(List.of("confirm", "accepted", "refused later"), refused.events);
        assertEquals(WINDOWS_REFUSED, refused.said);
        assertEquals(List.of(PcPowerFlow.REFUSAL_POLL_MS, PcPowerFlow.REFUSAL_POLL_MS), slept);

        // The PC never says within the reads: about 20 s, then nothing.
        calls.clear();
        script("pc_power", offer(TOKEN_B, null, true));
        for (int i = 0; i < PcPowerFlow.REFUSAL_READS; i++) {
            script("pc_power", refusedAfter(null));
        }
        script("pc_power.shutdown", ACCEPTED);
        Recorder quiet = new Recorder();
        flow(NativePcPower.Action.SHUT_DOWN, quiet).start();
        quiet.go.run();
        assertEquals(List.of("confirm", "accepted"), quiet.events);
        assertEquals(2 + PcPowerFlow.REFUSAL_READS, calls.size());
        assertEquals(20_000, PcPowerFlow.REFUSAL_POLL_MS * PcPowerFlow.REFUSAL_READS);
    }

    @Test
    public void aTokenOf25SecondsOrOlderIsFetchedAgainBeforeItIsSpent() {
        script("pc_power", offer(TOKEN_A, null, true), offer(TOKEN_B, null, true), UNREACHABLE);
        script("pc_power.shutdown", ACCEPTED);
        Recorder ui = new Recorder();
        flow(NativePcPower.Action.SHUT_DOWN, ui).start();
        clock.addAndGet(SleepFlow.TOKEN_REFRESH_MS);
        ui.go.run();
        assertEquals(List.of("pc_power", "pc_power", "pc_power.shutdown " + TOKEN_B, "pc_power"), calls);
        assertEquals(List.of("confirm", "accepted"), ui.events);

        // A refresh that finds the PC busy now stops before the request.
        script("pc_power", offer(TOKEN_A, null, true), offer(null, STREAMING, true));
        Recorder busy = new Recorder();
        flow(NativePcPower.Action.RESTART, busy).start();
        clock.addAndGet(SleepFlow.TOKEN_REFRESH_MS);
        busy.go.run();
        assertEquals(List.of("confirm", "unavailable"), busy.events);
        assertEquals(STREAMING, busy.said);
        assertFalse(calls.contains("pc_power.restart " + TOKEN_A));
    }

    @Test
    public void theHostsReasonAndRefusalReachThePersonAsSent() {
        script("pc_power", offer(null, STREAMING, true));
        Recorder busy = new Recorder();
        flow(NativePcPower.Action.RESTART, busy).start();
        assertEquals(List.of("unavailable"), busy.events);
        assertEquals(STREAMING, busy.said);

        script("pc_power", offer(TOKEN_A, null, true));
        script("pc_power.shutdown", api(409, "host_in_use", STREAMING));
        Recorder refused = new Recorder();
        flow(NativePcPower.Action.SHUT_DOWN, refused).start();
        refused.go.run();
        assertEquals(List.of("confirm", "refused"), refused.events);
        assertEquals(STREAMING, refused.said);

        script("pc_power", UNREACHABLE);
        Recorder unreachable = new Recorder();
        flow(NativePcPower.Action.RESTART, unreachable).start();
        assertEquals(List.of("failed"), unreachable.events);
        assertEquals(NativeApiException.Kind.NETWORK, unreachable.failure.getKind());
    }

    // The screen

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

    private static void assertCancelFocused(AlertDialog dialog) {
        assertTrue("Cancel holds the focus", dialog.getButton(AlertDialog.BUTTON_NEGATIVE).hasFocus());
        assertFalse(dialog.getButton(AlertDialog.BUTTON_POSITIVE).hasFocus());
    }

    @Test
    public void restartConfirmsWithCancelFocusedThenSaysThePcTookIt() {
        try (ActivityController<ToolbarIconTintTest.Screen> controller = screen()) {
            ToolbarIconTintTest.Screen screen = controller.get();
            HostActions actions = new HostActions(screen, CALLBACKS);
            NativeApiClient api = client();
            script("pc_power", offer(TOKEN_A, null, true), UNREACHABLE);
            script("pc_power.restart", ACCEPTED);
            ArrayDeque<Runnable> worker = new ArrayDeque<>();
            actions.startPcPower(office(), NativePcPower.Action.RESTART, () -> api, worker::add, slept::add);
            AlertDialog asking = latestDialog();
            assertEquals(screen.getString(R.string.host_sleep_asking), message(asking));
            worker.poll().run();
            AlertDialog confirm = latestDialog();
            assertFalse(asking.isShowing());
            assertEquals(screen.getString(R.string.host_power_message), message(confirm));
            assertCancelFocused(confirm);
            assertEquals(screen.getString(R.string.host_restart_action),
                    confirm.getButton(AlertDialog.BUTTON_POSITIVE).getText().toString());
            confirm.getButton(AlertDialog.BUTTON_POSITIVE).performClick();
            assertEquals(screen.getString(R.string.host_sleep_asking), message(latestDialog()));
            worker.poll().run();
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            assertEquals(List.of("pc_power", "pc_power.restart " + TOKEN_A, "pc_power"), calls);
            assertEquals(screen.getString(R.string.host_restart_accepted, "Office"), ShadowToast.getTextOfLatestToast());
        }
    }

    @Test
    public void theScreenShowsWindowsRefusalThatCameAfterTheAnswer() {
        try (ActivityController<ToolbarIconTintTest.Screen> controller = screen()) {
            ToolbarIconTintTest.Screen screen = controller.get();
            HostActions actions = new HostActions(screen, CALLBACKS);
            NativeApiClient api = client();
            String refusal = "Windows refused to shut down the PC: Access is denied. (exit code 5)";
            script("pc_power", offer(TOKEN_A, null, true), refusedAfter(refusal));
            script("pc_power.shutdown", ACCEPTED);
            actions.startPcPower(office(), NativePcPower.Action.SHUT_DOWN, () -> api, Runnable::run, slept::add);
            latestDialog().getButton(AlertDialog.BUTTON_POSITIVE).performClick();
            AlertDialog shown = latestDialog();
            assertEquals(refusal, message(shown));
            assertEquals(screen.getString(R.string.host_shutdown_refused_title, "Office"),
                    ((TextView) shown.findViewById(androidx.appcompat.R.id.alertTitle)).getText().toString());
        }
    }

    @Test
    public void aShutdownWarnsWhenThePcMayNotWakeFromOff() {
        try (ActivityController<ToolbarIconTintTest.Screen> controller = screen()) {
            ToolbarIconTintTest.Screen screen = controller.get();
            HostActions actions = new HostActions(screen, CALLBACKS);
            NativeApiClient api = client();
            String warning = screen.getString(R.string.host_shutdown_no_wake);

            script("pc_power", offer(TOKEN_A, null, false), offer(TOKEN_B, null, true));
            actions.startPcPower(office(), NativePcPower.Action.SHUT_DOWN, () -> api, Runnable::run);
            AlertDialog confirm = latestDialog();
            assertEquals(screen.getString(R.string.host_power_message) + "\n\n" + warning, message(confirm));
            assertCancelFocused(confirm);
            confirm.getButton(AlertDialog.BUTTON_NEGATIVE).performClick();

            actions.startPcPower(office(), NativePcPower.Action.SHUT_DOWN, () -> api, Runnable::run);
            assertFalse(message(latestDialog()).contains(warning));
            // Restart brings the PC back by itself: no warning.
            assertEquals(screen.getString(R.string.host_power_message),
                    HostActions.powerConfirmText(screen, NativePcPower.Action.RESTART, false));
            assertEquals(List.of("pc_power", "pc_power"), calls);
        }
    }

    @Test
    public void theScreenShowsTheHostsRefusalAndReasonAsTheySaidIt() {
        try (ActivityController<ToolbarIconTintTest.Screen> controller = screen()) {
            ToolbarIconTintTest.Screen screen = controller.get();
            HostActions actions = new HostActions(screen, CALLBACKS);
            NativeApiClient api = client();

            script("pc_power", offer(TOKEN_A, null, true));
            script("pc_power.shutdown", api(409, "host_in_use", STREAMING));
            actions.startPcPower(office(), NativePcPower.Action.SHUT_DOWN, () -> api, Runnable::run);
            latestDialog().getButton(AlertDialog.BUTTON_POSITIVE).performClick();
            assertEquals(STREAMING, message(latestDialog()));

            script("pc_power", offer(null, "Turn on Restart and shut down PC for this device in the Rubylight console.", true));
            actions.startPcPower(office(), NativePcPower.Action.RESTART, () -> api, Runnable::run);
            assertEquals("Turn on Restart and shut down PC for this device in the Rubylight console.",
                    message(latestDialog()));

            script("pc_power", offer(null, null, true));
            actions.startPcPower(office(), NativePcPower.Action.RESTART, () -> api, Runnable::run);
            assertEquals(screen.getString(R.string.host_power_busy), message(latestDialog()));
        }
    }


    // Update the PC's host

    private static final class UpdateRecorder implements HostUpdateFlow.Ui {
        final List<String> events = new ArrayList<>();

        @Override public void started(String version) { events.add("started " + version); }
        @Override public void waiting(String reason) { events.add("waiting " + reason); }
        @Override public void back(String version) { events.add("back " + version); }
        @Override public void timedOut(String version) { events.add("timeout " + version); }
        @Override public void nothingToInstall() { events.add("nothing to install"); }
        @Override public void updateFailed(String error) { events.add("failed on the PC " + error); }
        @Override public void cameBackOn(String version, String error) { events.add("back on " + version + " " + error); }
        @Override public void refused(String message) { events.add("refused " + message); }
        @Override public void failed(NativeApiException error) { events.add("failed " + error.getKind()); }
    }

    private static final HostUpdateFlow.Probe NO_ANSWER = () -> null;
    private static final long POLLS = HostUpdateFlow.WAIT_MS / HostUpdateFlow.POLL_MS;

    private UpdateRecorder follow(HostUpdateFlow.Probe probe) {
        NativeApiClient api = client();
        UpdateRecorder ui = new UpdateRecorder();
        new HostUpdateFlow(() -> api, probe, ui, Runnable::run, Runnable::run, slept::add).start();
        return ui;
    }

    @Test
    public void anUpdateWaitsUntilServerinfoAnswersWithTheNewVersion() {
        script("update.install", update("downloading"));
        script("update", update("downloading"), UNREACHABLE, UNREACHABLE);
        java.util.LinkedList<String> versions = new java.util.LinkedList<>(Arrays.asList("3.3.0", null, null, "3.4.0"));
        UpdateRecorder ui = follow(versions::poll);
        assertEquals(List.of("started 3.4.0", "back 3.4.0"), ui.events);
        assertEquals(List.of("update.install", "update 3.4.0", "update 3.4.0", "update 3.4.0"), calls);
        assertEquals(4, slept.size());
        assertEquals(HostUpdateFlow.POLL_MS, (long) slept.get(0));
    }

    @Test
    public void anUpdateThatNeverArrivesTimesOutAndARefusalShowsTheHostsWords() {
        script("update.install", update("downloading"), api(409, "host_in_use", STREAMING));
        for (long i = 0; i < POLLS; i++) {
            script("update", update("installing"));
        }
        UpdateRecorder ui = follow(() -> "3.3.0");
        assertEquals(List.of("started 3.4.0", "timeout 3.4.0"), ui.events);
        assertEquals(POLLS, slept.size());

        UpdateRecorder refused = follow(() -> {
            throw new AssertionError("No wait after a refusal");
        });
        assertEquals(List.of("refused " + STREAMING), refused.events);
    }

    @Test
    public void anInstallThatOffersNoVersionEndsAtOnce() {
        script("update.install", update("idle", null, null, null, false));
        UpdateRecorder ui = follow(() -> {
            throw new AssertionError("No wait for a version that isn't coming");
        });
        assertEquals(List.of("nothing to install"), ui.events);
        assertTrue(slept.isEmpty());
    }

    @Test
    public void theUpdatersFailureEndsTheWaitInItsWords() {
        script("update.install", update("downloading"), update("downloading"));
        script("update", update("downloading"), update("downloading", "3.4.0", null, "Installer verification failed", false),
                update("downloading", "3.4.0", null, "", false));
        UpdateRecorder ui = follow(NO_ANSWER);
        assertEquals(List.of("started 3.4.0", "failed on the PC Installer verification failed"), ui.events);
        UpdateRecorder unsaid = follow(NO_ANSWER);
        assertEquals(List.of("started 3.4.0", "failed on the PC null"), unsaid.events);
    }

    @Test
    public void aPcBackOnTheOldVersionWithNothingUnderWayEndsTheWait() {
        script("update.install", update("downloading"));
        // Only after it went away: before that, idle and on 3.3.0 is how the install starts.
        script("update", update("installing", "3.4.0", null, null, false), UNREACHABLE,
                update("idle", "3.4.0", null, null, true));
        UpdateRecorder ui = follow(NO_ANSWER);
        assertEquals(List.of("started 3.4.0", "back on 3.3.0 null"), ui.events);
        assertEquals(3, slept.size());
    }

    @Test
    public void aWaitForAnotherDevicesStreamIsSaidOnceAndDoesNotCountAgainstTheTimeout() {
        script("update.install", update("ready"));
        long waits = POLLS + 20;
        for (long i = 0; i < waits; i++) {
            script("update", update("ready", "3.4.0", STREAMING, null, false));
        }
        AtomicLong asked = new AtomicLong();
        UpdateRecorder ui = follow(() -> asked.incrementAndGet() > waits ? "3.4.0" : "3.3.0");
        assertEquals(List.of("started 3.4.0", "waiting " + STREAMING, "back 3.4.0"), ui.events);
    }

    @Test
    public void theUpdateConfirmsWithCancelFocusedShowsProgressThenThePcIsBack() throws Exception {
        try (ActivityController<ToolbarIconTintTest.Screen> controller = screen()) {
            ToolbarIconTintTest.Screen screen = controller.get();
            HostActions actions = new HostActions(screen, CALLBACKS);
            script("update", update("idle"), UNREACHABLE);
            script("update.install", update("downloading"));
            NativeApiClient api = client();
            api.hostUpdate();
            assertEquals("3.4.0", KnownUpdates.offered("pc"));

            List<String> installed = new ArrayList<>();
            actions.confirmUpdate(office(), "3.4.0", () -> installed.add("install"));
            AlertDialog confirm = latestDialog();
            assertEquals(screen.getString(R.string.host_update_message), message(confirm));
            assertCancelFocused(confirm);
            confirm.getButton(AlertDialog.BUTTON_NEGATIVE).performClick();
            assertTrue("Cancel installs nothing", installed.isEmpty());

            ArrayDeque<Runnable> worker = new ArrayDeque<>();
            java.util.LinkedList<String> versions = new java.util.LinkedList<>(Arrays.asList(null, "3.4.0"));
            actions.startUpdate(office(), () -> api, versions::poll, worker::add, clock::addAndGet);
            assertEquals(screen.getString(R.string.host_sleep_asking), message(latestDialog()));
            worker.poll().run();
            AlertDialog back = latestDialog();
            assertEquals(screen.getString(R.string.host_update_back, "3.4.0"), message(back));
            assertNull("The host's earlier answer is out of date", KnownUpdates.offered("pc"));

            // The progress in between, on its own.
            HostUpdateFlow.Ui ui = actions.updateUi(office(), new HostActions.Route(screen, "test"));
            ui.started("3.4.0");
            assertEquals(screen.getString(R.string.host_update_started, "3.4.0"), message(latestDialog()));
            ui.waiting(STREAMING);
            assertEquals(STREAMING + " The update starts once that stream ends.", message(latestDialog()));
        }
    }

    @Test
    public void theScreenSaysWhyTheUpdateEnded() {
        try (ActivityController<ToolbarIconTintTest.Screen> controller = screen()) {
            ToolbarIconTintTest.Screen screen = controller.get();
            HostActions actions = new HostActions(screen, CALLBACKS);
            HostUpdateFlow.Ui ui = actions.updateUi(office(), new HostActions.Route(screen, "test"));
            ui.nothingToInstall();
            assertEquals(screen.getString(R.string.host_update_none), message(latestDialog()));
            ui.updateFailed("Installer verification failed");
            assertEquals("Installer verification failed", message(latestDialog()));
            ui.updateFailed(null);
            assertEquals(screen.getString(R.string.host_update_failed), message(latestDialog()));
            ui.cameBackOn("3.3.0", null);
            assertEquals(screen.getString(R.string.host_update_old_version, "3.3.0"), message(latestDialog()));
            ui.cameBackOn("3.3.0", "Setup exited with code 1603.");
            assertEquals(screen.getString(R.string.host_update_old_version, "3.3.0") + " Setup exited with code 1603.",
                    message(latestDialog()));
        }
    }
}
