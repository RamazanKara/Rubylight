package com.limelight.computers;

import static org.junit.Assert.assertArrayEquals;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.os.Looper;
import android.widget.TextView;

import androidx.appcompat.app.AlertDialog;

import com.limelight.R;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.jni.NightjarJni;
import com.limelight.nvstream.wol.WakeRelayKeys;
import com.limelight.ui.ToolbarIconTintTest;

import org.junit.After;
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

import java.io.IOException;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.List;
import java.util.concurrent.TimeUnit;

/** Wake from away: when an offline PC's wake relay is asked, and what the user reads at each step and outcome. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class AwayWakeTest {
    private static final String BUNDLE = "{\"target_id\":\"PC\",\"epoch\":1,\"device\":\"ab\",\"relays\":["
            + "{\"id\":\"r1\",\"name\":\"NUC\",\"secret\":\"00\"},{\"id\":\"r2\",\"name\":\"Server\",\"secret\":\"11\"}]}";
    // Today's wait for the PC (WakeLaunch).
    private static final long TIMEOUT_MS = 90_000;

    private ActivityController<ToolbarIconTintTest.Screen> controller;
    private HostActions actions;
    private final List<String> events = new ArrayList<>();
    private final List<String> sent = new ArrayList<>();
    private final List<Runnable> queued = new ArrayList<>();
    private final FakeRelay relay = new FakeRelay();
    private ComputerDetails pc;
    private long now;
    private IOException sendError;

    private static final class FakeRelay implements AwayWakeFlow.Relay {
        final List<Long> cancelled = new ArrayList<>();
        String bundle;
        String outcome = "{\"outcome\":\"awake\",\"relays\":[\"NUC\"]}";
        List<String> steps = Collections.emptyList();

        @Override public long create() { return 7; }
        @Override public void cancel(long id) { cancelled.add(id); }
        @Override public String request(long id, String bundle, NightjarJni.WakeRelayListener listener) {
            this.bundle = bundle;
            for (String step : steps) listener.progress(step);
            return outcome;
        }
    }

    @Before
    public void setUp() {
        controller = Robolectric.buildActivity(ToolbarIconTintTest.Screen.class).setup().visible();
        actions = new HostActions(controller.get(), new HostActions.Callbacks() {
            @Override public ComputerManagerService.ComputerManagerBinder binder() { return null; }
            @Override public boolean isForeground() { return true; }
            @Override public void onPaired(ComputerDetails computer) { }
            @Override public void onRemoved(ComputerDetails computer) { }
            @Override public void onRenamed(ComputerDetails computer) { }
            @Override public void onWakeSent(ComputerDetails computer) { events.add("sent " + computer.uuid); }
            @Override public void onWakeEnded(ComputerDetails computer) { events.add("ended " + computer.uuid); }
            @Override public void pausePolling() { }
            @Override public void resumePolling() { }
        });
        pc = new ComputerDetails();
        pc.uuid = "pc";
        pc.name = "Office";
        pc.state = ComputerDetails.State.OFFLINE;
        pc.macAddress = "02:7c:16:b4:13:4f";
    }

    @After
    public void tearDown() {
        actions.destroy();
        controller.pause().stop().destroy();
    }

    private String text(AwayWake.Words words) {
        return words.in(controller.get());
    }

    private String string(int id, Object... args) {
        return controller.get().getString(id, args);
    }

    private void wake(WakeRelayKeys.Kept kept, boolean onPcNetwork) {
        actions.wake(pc, kept, onPcNetwork, host -> {
            sent.add(host.uuid);
            if (sendError != null) throw sendError;
        }, Runnable::run, relay, queued::add, () -> now);
    }

    /** Lets time pass a second at a time, the LAN wait's tick. */
    private void pass(long ms) {
        for (long step = 0; step < ms; step += 1_000) {
            now += 1_000;
            Shadows.shadowOf(Looper.getMainLooper()).idleFor(1_000, TimeUnit.MILLISECONDS);
        }
    }

    private static AlertDialog latestDialog() {
        Shadows.shadowOf(Looper.getMainLooper()).idle();
        AlertDialog dialog = (AlertDialog) ShadowDialog.getLatestDialog();
        assertNotNull(dialog);
        return dialog;
    }

    private static String message(AlertDialog dialog) {
        return ((TextView) dialog.findViewById(android.R.id.message)).getText().toString();
    }

    private void runWorker() {
        for (Runnable task : new ArrayList<>(queued)) task.run();
        queued.clear();
        Shadows.shadowOf(Looper.getMainLooper()).idle();
    }

    // Off the main thread

    @Test
    public void anOfflinePcsKeysAndThisDevicesNetworkAreReadOnTheWorkerBeforeAnythingStarts() {
        List<String> looked = new ArrayList<>();
        List<Runnable> posted = new ArrayList<>();
        actions.wake(pc, computer -> {
            looked.add(computer.uuid);
            return new AwayWake.Seen(WakeRelayKeys.Kept.parse(BUNDLE), false);
        }, host -> sent.add(host.uuid), Runnable::run, relay, queued::add, posted::add, () -> now);
        assertTrue("nothing read on the caller's thread", looked.isEmpty());
        assertFalse(actions.isWaking());
        assertEquals(1, queued.size());
        // The worker reads, then hands the decision back to the main thread.
        queued.remove(0).run();
        assertEquals(Collections.singletonList("pc"), looked);
        assertFalse("chosen only back on the main thread", actions.isWaking());
        assertEquals(1, posted.size());
        posted.remove(0).run();
        // Away with a relay: the packets go and the relay is asked.
        assertTrue(actions.isWaking());
        assertEquals(Collections.singletonList("pc"), sent);
    }

    @Test
    public void aPcThatIsNotOfflineReadsNothingAndALookThatFailsLeavesTodaysWake() {
        List<Runnable> posted = new ArrayList<>();
        pc.state = ComputerDetails.State.ONLINE;
        actions.wake(pc, computer -> {
            throw new AssertionError("read for a PC that is online");
        }, host -> sent.add(host.uuid), Runnable::run, relay, queued::add, posted::add, () -> now);
        assertTrue(queued.isEmpty());
        assertTrue(posted.isEmpty());
        assertEquals(string(R.string.wol_pc_online), ShadowToast.getTextOfLatestToast());

        pc.state = ComputerDetails.State.OFFLINE;
        actions.wake(pc, computer -> {
            throw new IllegalStateException("unreadable keys");
        }, host -> sent.add(host.uuid), Runnable::run, relay, queued::add, posted::add, () -> now);
        queued.remove(0).run();
        posted.remove(0).run();
        assertTrue("today's wake", actions.isWaking());
        assertEquals(Collections.singletonList("pc"), sent);
        assertEquals(Collections.singletonList("sent pc"), events);
        assertNull("no relay asked", relay.bundle);
    }

    // The decision

    @Test
    public void onlyAnOfflinePcThatOffersItSeenFromAwayAsksARelay() {
        WakeRelayKeys.Kept keys = WakeRelayKeys.Kept.parse(BUNDLE);
        assertEquals(AwayWake.Path.RELAY, AwayWake.path(true, false, keys));
        assertEquals("on the PC's network", AwayWake.Path.LAN, AwayWake.path(true, true, keys));
        assertEquals("a PC that is not offline", AwayWake.Path.LAN, AwayWake.path(false, false, keys));
        assertEquals("a host without the feature", AwayWake.Path.LAN, AwayWake.path(true, false, null));
        assertEquals("offered, no keys", AwayWake.Path.NO_RELAY,
                AwayWake.path(true, false, WakeRelayKeys.Kept.parse("")));
        assertEquals("keys, no relays", AwayWake.Path.NO_RELAY,
                AwayWake.path(true, false, WakeRelayKeys.Kept.parse("{\"relays\":[]}")));
        assertEquals("unreadable keys", AwayWake.Path.NO_RELAY,
                AwayWake.path(true, false, WakeRelayKeys.Kept.parse("{not json")));
        assertEquals(Arrays.asList("NUC", "Server"), keys.relays);
        assertEquals(BUNDLE, keys.bundle);
    }

    @Test
    public void thisDeviceIsHomeWhenOneOfItsSubnetsHoldsThePcOrItsWakeBroadcast() {
        AwayWake.Subnet home = new AwayWake.Subnet(AwayWake.ipv4("192.168.4.37"), 24);
        AwayWake.Subnet cellular = new AwayWake.Subnet(AwayWake.ipv4("100.71.3.9"), 32);
        List<String> pc = Collections.singletonList("192.168.4.20");
        List<String> none = Collections.emptyList();
        assertTrue(AwayWake.onPcNetwork(Arrays.asList(cellular, home), pc, none));
        assertFalse(AwayWake.onPcNetwork(Collections.singletonList(cellular), pc, none));
        assertFalse("another network with another subnet",
                AwayWake.onPcNetwork(Collections.singletonList(home), Collections.singletonList("192.168.5.20"), none));
        assertTrue("a subnet the PC listed for waking", AwayWake.onPcNetwork(
                Collections.singletonList(new AwayWake.Subnet(AwayWake.ipv4("10.0.0.5"), 24)),
                Collections.singletonList("pc.example"), Collections.singletonList("10.0.0.255")));
        assertFalse("no network at all", AwayWake.onPcNetwork(Collections.emptyList(), pc, none));
        assertNull(AwayWake.ipv4("pc.example"));
        assertNull(AwayWake.ipv4("192.168.4.256"));
        assertNull(AwayWake.ipv4("192.168.4"));
        assertEquals(Integer.valueOf(0xC0A80414), AwayWake.ipv4("192.168.4.20"));
    }

    // The words

    @Test
    public void eachStepSaysWhatTheRelayIsDoing() {
        assertEquals(string(R.string.wake_relay_contacting, "NUC"),
                text(AwayWake.step("{\"stage\":\"looking_up\",\"relay\":\"NUC\"}", "Office")));
        assertEquals(string(R.string.wake_relay_contacting, "NUC"),
                text(AwayWake.step("{\"stage\":\"punching\",\"relay\":\"NUC\"}", "Office")));
        assertEquals(string(R.string.wake_relay_asking, "NUC", "Office"),
                text(AwayWake.step("{\"stage\":\"requested\",\"relay\":\"NUC\"}", "Office")));
        assertEquals(string(R.string.wake_relay_sent, "Office"),
                text(AwayWake.step("{\"stage\":\"status\",\"relay\":\"NUC\",\"status\":\"packet_sent\"}", "Office")));
        assertNull("a final status leaves the words to the outcome",
                AwayWake.step("{\"stage\":\"status\",\"status\":\"host_awake\"}", "Office"));
        assertNull(AwayWake.step("garbage", "Office"));
    }

    @Test
    public void eachOutcomeIsSaidInPlainWords() {
        assertOutcome("{\"outcome\":\"awake\",\"relays\":[\"NUC\"]}", R.string.wake_relay_awake, "Office");
        assertOutcome("{\"outcome\":\"sent_no_answer\"}", R.string.wake_relay_sent_no_answer, "Office");
        assertOutcome("{\"outcome\":\"timed_out\"}", R.string.wake_relay_timed_out, "Office");
        assertOutcome("{\"outcome\":\"rate_limited\",\"retry_after\":42}", R.string.wake_relay_rate_limited, "Office",
                42);
        assertOutcome("{\"outcome\":\"refused\"}", R.string.wake_relay_refused);
        assertOutcome("{\"outcome\":\"invalid\"}", R.string.wake_relay_refused);
        assertOutcome("{\"outcome\":\"relay_offline\",\"relays\":[\"NUC\",\"Server\"]}", R.string.wake_relay_offline,
                "NUC, Server", "Office");
        assertOutcome("{\"outcome\":\"relay_unreachable\",\"relays\":[\"NUC\"]}", R.string.wake_relay_unreachable,
                "NUC");
        assertOutcome("{\"outcome\":\"not_on_pc_network\",\"relays\":[\"NUC\"]}", R.string.wake_relay_elsewhere,
                "NUC", "Office");
        assertOutcome("{\"outcome\":\"network\",\"detail\":\"no route\"}", R.string.wake_relay_network);
        assertOutcome("{\"outcome\":\"no_relay\"}", R.string.wake_relay_none, "Office");
        assertOutcome("{\"outcome\":\"from_a_later_version\",\"relays\":[\"NUC\"]}",
                R.string.wake_relay_unreachable, "NUC");
        assertNull("a cancelled wake says nothing", AwayWake.outcome("{\"outcome\":\"cancelled\"}", "Office"));

        for (String retry : new String[] {"sent_no_answer", "timed_out", "relay_offline", "relay_unreachable",
                "network"}) {
            assertTrue(retry, AwayWake.offersRetry(retry));
        }
        for (String final_ : new String[] {"awake", "refused", "rate_limited", "no_relay", "not_on_pc_network",
                "cancelled"}) {
            assertFalse(final_, AwayWake.offersRetry(final_));
        }
    }

    @Test
    public void noWordsNameTheHostOrSayExperimental() {
        int[] all = {R.string.wake_relay_contacting, R.string.wake_relay_asking, R.string.wake_relay_sent,
                R.string.wake_relay_awake, R.string.wake_relay_sent_no_answer, R.string.wake_relay_timed_out,
                R.string.wake_relay_rate_limited, R.string.wake_relay_refused, R.string.wake_relay_offline,
                R.string.wake_relay_unreachable, R.string.wake_relay_elsewhere, R.string.wake_relay_network,
                R.string.wake_relay_none};
        for (int id : all) {
            String words = controller.get().getString(id).toLowerCase(java.util.Locale.ROOT);
            assertFalse(words, words.contains("rubylight") || words.contains("butterpollo")
                    || words.contains("experimental"));
        }
    }

    private void assertOutcome(String json, int text, Object... args) {
        AwayWake.Words words = AwayWake.outcome(json, "Office");
        assertNotNull(json, words);
        assertEquals(json, text, words.text);
        assertArrayEquals(json, args, words.args);
    }

    // The flow on screen

    @Test
    public void aRelayIsAskedWithTheKeptBundleAndItsOutcomeIsShown() {
        relay.outcome = "{\"outcome\":\"relay_offline\",\"relays\":[\"NUC\",\"Server\"]}";
        wake(WakeRelayKeys.Kept.parse(BUNDLE), false);
        assertEquals("the magic packets go out too", Collections.singletonList("pc"), sent);
        assertEquals(Collections.singletonList("sent pc"), events);
        assertTrue(actions.isWaking());
        AlertDialog progress = latestDialog();
        assertEquals(string(R.string.wake_relay_contacting, "NUC"), message(progress));

        runWorker();
        assertEquals(BUNDLE, relay.bundle);
        assertEquals(Arrays.asList("sent pc", "ended pc"), events);
        assertFalse(actions.isWaking());
        assertFalse(progress.isShowing());
        AlertDialog outcome = latestDialog();
        assertEquals(string(R.string.wake_relay_offline, "NUC, Server", "Office"), message(outcome));
        assertEquals(string(R.string.pair_try_again), outcome.getButton(AlertDialog.BUTTON_POSITIVE).getText().toString());

        outcome.getButton(AlertDialog.BUTTON_POSITIVE).performClick();
        Shadows.shadowOf(Looper.getMainLooper()).idle();
        assertEquals("Try again asks again", 1, queued.size());
        assertEquals(Arrays.asList("pc", "pc"), sent);
    }

    @Test
    public void anAwakePcIsToldInAToast() {
        relay.steps = Arrays.asList("{\"stage\":\"requested\",\"relay\":\"NUC\"}",
                "{\"stage\":\"status\",\"relay\":\"NUC\",\"status\":\"packet_sent\"}");
        wake(WakeRelayKeys.Kept.parse(BUNDLE), false);
        AlertDialog progress = latestDialog();
        runWorker();
        assertFalse(progress.isShowing());
        assertEquals(string(R.string.wake_relay_awake, "Office"), ShadowToast.getTextOfLatestToast());
    }

    @Test
    public void cancelEndsTheRequestAndItsOutcomeShowsNothing() {
        wake(WakeRelayKeys.Kept.parse(BUNDLE), false);
        AlertDialog progress = latestDialog();
        progress.getButton(AlertDialog.BUTTON_NEGATIVE).performClick();
        Shadows.shadowOf(Looper.getMainLooper()).idle();
        assertEquals(Collections.singletonList(7L), relay.cancelled);
        assertEquals(Arrays.asList("sent pc", "ended pc"), events);
        assertFalse(actions.isWaking());

        relay.outcome = "{\"outcome\":\"cancelled\"}";
        runWorker();
        assertEquals("ended once", Arrays.asList("sent pc", "ended pc"), events);
        assertTrue(ShadowDialog.getLatestDialog() == progress);
    }

    @Test
    public void onANetworkNumberedLikeHomeARelayIsAskedOnceTodaysWaitEndsWithThePcAway() {
        // Away on a network that reuses the home numbering: a subnet holds the PC's address, so it looks like home.
        boolean home = AwayWake.onPcNetwork(
                Collections.singletonList(new AwayWake.Subnet(AwayWake.ipv4("192.168.1.37"), 24)),
                Collections.singletonList("192.168.1.20"), Collections.emptyList());
        assertTrue(home);
        relay.outcome = "{\"outcome\":\"timed_out\"}";
        wake(WakeRelayKeys.Kept.parse(BUNDLE), home);
        assertEquals("today's wake first", Collections.singletonList("pc"), sent);
        assertEquals(Collections.singletonList("sent pc"), events);
        assertEquals(string(R.string.wol_waking_msg), ShadowToast.getTextOfLatestToast());
        pass(TIMEOUT_MS - 1_000);
        assertTrue("no relay while the wait lasts", queued.isEmpty());

        pass(2_000);
        assertEquals("the relay next", 1, queued.size());
        assertEquals(Arrays.asList("sent pc", "ended pc", "sent pc"), events);
        assertTrue(actions.isWaking());
        AlertDialog progress = latestDialog();
        assertEquals(string(R.string.wake_relay_contacting, "NUC"), message(progress));
        assertNotNull("Cancel ends it", progress.getButton(AlertDialog.BUTTON_NEGATIVE));

        runWorker();
        assertEquals(BUNDLE, relay.bundle);
        assertFalse(progress.isShowing());
        assertEquals(Arrays.asList("sent pc", "ended pc", "sent pc", "ended pc"), events);
        AlertDialog outcome = latestDialog();
        assertEquals(string(R.string.wake_relay_timed_out, "Office"), message(outcome));

        int before = sent.size();
        outcome.getButton(AlertDialog.BUTTON_POSITIVE).performClick();
        Shadows.shadowOf(Looper.getMainLooper()).idle();
        assertEquals("Try again asks the relay at once", 1, queued.size());
        assertEquals(before + 1, sent.size());
    }

    @Test
    public void aPcThatAnswersDuringTodaysWaitLeavesTheRelayUnasked() {
        wake(WakeRelayKeys.Kept.parse(BUNDLE), true);
        pass(8_000);
        pc.state = ComputerDetails.State.ONLINE;
        pass(1_000);
        assertEquals(string(R.string.wol_pc_online), ShadowToast.getTextOfLatestToast());
        assertEquals(Arrays.asList("sent pc", "ended pc"), events);
        pass(TIMEOUT_MS);
        assertTrue(queued.isEmpty());
        assertNull(relay.bundle);
        assertFalse(actions.isWaking());
        assertFalse(actions.isShowingError());
    }

    @Test
    public void aWakeThatCannotSendOrHasNoMacAsksAKeptRelay() {
        sendError = new IOException("no route");
        wake(WakeRelayKeys.Kept.parse(BUNDLE), true);
        Shadows.shadowOf(Looper.getMainLooper()).idle();
        assertEquals(1, queued.size());
        assertEquals(string(R.string.wake_relay_contacting, "NUC"), message(latestDialog()));

        actions.destroy();
        queued.clear();
        sent.clear();
        pc.macAddress = null;
        wake(WakeRelayKeys.Kept.parse(BUNDLE), true);
        assertEquals("the relay knows the PC's MACs", 1, queued.size());
        assertTrue(sent.isEmpty());
        // Without a relay, away: the hint follows.
        actions.destroy();
        wake(WakeRelayKeys.Kept.parse(""), false);
        assertEquals(string(R.string.wol_no_mac) + "\n\n" + string(R.string.wake_relay_none, "Office"),
                message(latestDialog()));
    }

    @Test
    public void awayWithoutARelayTodaysWakeRunsAndOnlyItsFailureSaysHowToGetOne() {
        wake(WakeRelayKeys.Kept.parse(""), false);
        assertEquals(Collections.singletonList("pc"), sent);
        assertEquals(Collections.singletonList("sent pc"), events);
        assertTrue(actions.isWaking());
        assertEquals(string(R.string.wol_waking_msg), ShadowToast.getTextOfLatestToast());
        assertFalse("nothing said while the wait lasts", actions.isShowingError());

        pass(TIMEOUT_MS + 1_000);
        assertEquals(Arrays.asList("sent pc", "ended pc"), events);
        assertEquals(string(R.string.console_wake_timeout, "Office") + "\n\n"
                + string(R.string.wake_relay_none, "Office"), message(latestDialog()));
        assertTrue(queued.isEmpty());

        // A PC that answers says so, without the hint.
        actions.destroy();
        events.clear();
        wake(WakeRelayKeys.Kept.parse(""), false);
        pc.state = ComputerDetails.State.ONLINE;
        pass(1_000);
        assertEquals(string(R.string.wol_pc_online), ShadowToast.getTextOfLatestToast());
        assertFalse(actions.isShowingError());

        // A wake that cannot send says so, with the hint.
        pc.state = ComputerDetails.State.OFFLINE;
        sendError = new IOException("no route");
        wake(WakeRelayKeys.Kept.parse(""), false);
        Shadows.shadowOf(Looper.getMainLooper()).idle();
        assertEquals(string(R.string.wol_fail) + "\n\n" + string(R.string.wake_relay_none, "Office"),
                message(latestDialog()));
    }

    @Test
    public void onThePcsNetworkWithoutARelayOrWithoutTheFeatureTodaysWakeRunsAsBefore() {
        for (WakeRelayKeys.Kept kept : new WakeRelayKeys.Kept[] {WakeRelayKeys.Kept.parse(""), null}) {
            sent.clear();
            events.clear();
            wake(kept, kept != null);
            assertEquals(Collections.singletonList("pc"), sent);
            assertEquals(string(R.string.wol_waking_msg), ShadowToast.getTextOfLatestToast());
            pass(TIMEOUT_MS + 1_000);
            assertEquals(Arrays.asList("sent pc", "ended pc"), events);
            assertEquals("no hint", string(R.string.console_wake_timeout, "Office"), message(latestDialog()));
        }
        assertTrue(queued.isEmpty());

        events.clear();
        sent.clear();
        pc.state = ComputerDetails.State.ONLINE;
        wake(WakeRelayKeys.Kept.parse(BUNDLE), false);
        assertEquals(string(R.string.wol_pc_online), ShadowToast.getTextOfLatestToast());
        assertTrue(sent.isEmpty());
        assertTrue(queued.isEmpty());
        assertTrue(events.isEmpty());
    }

    // The kept keys

    @Test
    public void keysAreKeptPerPcUntilThePcStopsOfferingThemOrForgetsThisDevice() {
        WakeRelayKeys keys = new WakeRelayKeys(controller.get());
        assertNull(keys.load("pc"));
        keys.store("pc", BUNDLE);
        assertEquals(BUNDLE, keys.load("pc").bundle);
        keys.store("pc", null);
        assertFalse("offered without keys", keys.load("pc").hasRelay());
        keys.store("pc", BUNDLE);
        keys.forget("pc");
        assertNull(keys.load("pc"));
        keys.store("pc", BUNDLE);
        InternetPlay.forget(controller.get(), "pc");
        assertNull("unpaired or removed", keys.load("pc"));
    }
}
