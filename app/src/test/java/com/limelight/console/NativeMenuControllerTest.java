package com.limelight.console;

import static com.limelight.nativeapi.NativeApiFixtures.error;
import static com.limelight.nativeapi.NativeApiFixtures.ok;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertSame;
import static org.junit.Assert.assertTrue;

import android.app.Application;

import com.limelight.R;
import com.limelight.nativeapi.HostEndpoint;
import com.limelight.nativeapi.NativeApiClient;
import com.limelight.nativeapi.NativeApiFixtures;

import org.json.JSONObject;
import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.annotation.Config;

import java.util.ArrayList;
import java.util.Collections;
import java.util.Comparator;
import java.util.List;

@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class NativeMenuControllerTest {
    /** Virtual time: tasks run in due order, as the one background thread would. */
    private static final class Clock implements NativeMenuController.Scheduler {
        private final class Task {
            final long due;
            final int seq;
            final Runnable run;

            Task(long due, int seq, Runnable run) {
                this.due = due;
                this.seq = seq;
                this.run = run;
            }
        }

        long now;
        int seq;
        final List<Task> queue = new ArrayList<>();

        @Override
        public void schedule(Runnable task, long delayMs) {
            queue.add(new Task(now + delayMs, seq++, task));
        }

        /** Runs everything due up to {@code until}. */
        void advanceTo(long until) {
            while (true) {
                Task next = null;
                for (Task task : queue) {
                    if (task.due <= until && (next == null
                            || Comparator.<Task>comparingLong(t -> t.due).thenComparingInt(t -> t.seq).compare(task, next) < 0)) {
                        next = task;
                    }
                }
                if (next == null) {
                    now = until;
                    return;
                }
                queue.remove(next);
                now = Math.max(now, next.due);
                next.run.run();
            }
        }
    }

    private final NativeApiFixtures.FakeTransport transport = new NativeApiFixtures.FakeTransport();
    private final Clock clock = new Clock();
    private final List<NativeMenuController.State> states = new ArrayList<>();
    private boolean noClient;
    private final NativeMenuController controller = new NativeMenuController(this::client, clock, Runnable::run);

    private NativeApiClient client() {
        if (noClient) {
            return null;
        }
        return new NativeApiClient(transport,
                HostEndpoint.of(NativeApiFixtures.nativeHost("pc", "PC", true), "unique", NativeApiFixtures.crypto()));
    }

    private int calls(String op) {
        return Collections.frequency(transport.calls, "pc|" + op);
    }

    private NativeMenuController.State last() {
        return states.get(states.size() - 1);
    }

    private void open() {
        controller.start(states::add);
    }

    @Test
    public void openingReadsOnceAndThenOncePerSecondOnlyWhileShown() {
        transport.reply("pc", "radeon", ok(RadeonFixtures.configurable()));
        open();
        assertFalse(last().loaded());
        clock.advanceTo(0);
        assertEquals(1, calls("radeon"));
        assertTrue(last().loaded());
        assertNotNull(last().model);
        clock.advanceTo(999);
        assertEquals("not before a second has passed", 1, calls("radeon"));
        clock.advanceTo(1000);
        assertEquals(2, calls("radeon"));
        clock.advanceTo(3500);
        assertEquals(4, calls("radeon"));
        controller.stop();
        clock.advanceTo(20000);
        assertEquals("a closed panel makes no calls", 4, calls("radeon"));
    }

    @Test
    public void aSlowReadDelaysTheNextOneInsteadOfPilingUp() {
        transport.reply("pc", "radeon", ok(RadeonFixtures.configurable()));
        NativeMenuController slow = new NativeMenuController(() -> new NativeApiClient((host, op, argument) -> {
            clock.now += 700;
            return transport.request(host, op, argument);
        }, HostEndpoint.of(NativeApiFixtures.nativeHost("pc", "PC", true), "unique", NativeApiFixtures.crypto())),
                clock, Runnable::run);
        slow.start(states::add);
        clock.advanceTo(0);
        assertEquals("the read ended at 700", 1, calls("radeon"));
        clock.advanceTo(1699);
        assertEquals(1, calls("radeon"));
        clock.advanceTo(1700);
        assertEquals("a second after the read ended", 2, calls("radeon"));
    }

    @Test
    public void closingBeforeAQueuedReadRunsDropsIt() {
        transport.reply("pc", "radeon", ok(RadeonFixtures.configurable()));
        open();
        controller.stop();
        clock.advanceTo(5000);
        assertEquals(0, calls("radeon"));
        assertEquals(1, states.size());
    }

    @Test
    public void aMissingEndpointHidesThePanelAndStopsPolling() {
        transport.reply("pc", "radeon", error("not_found", "no such endpoint"));
        open();
        clock.advanceTo(0);
        assertTrue(last().hidden);
        assertNull(last().model);
        clock.advanceTo(10000);
        assertEquals(1, calls("radeon"));
        // Opening the menu again within the same stream does not ask again.
        controller.stop();
        open();
        clock.advanceTo(20000);
        assertEquals(1, calls("radeon"));
        assertTrue(last().hidden);
    }

    @Test
    public void aHostWithoutANativeClientHidesThePanel() {
        noClient = true;
        open();
        clock.advanceTo(0);
        assertTrue(last().hidden);
        assertEquals(0, transport.calls.size());
    }

    @Test
    public void anUnreachableHostShowsAFailureUntilThereIsAPanelAndThenKeepsThePanel() {
        transport.reply("pc", "radeon", error("network", "refused"));
        open();
        clock.advanceTo(0);
        assertNull(last().model);
        assertNotNull(last().loadFailure);
        assertEquals(R.string.native_error_unreachable, last().loadFailure.messageRes);
        transport.reply("pc", "radeon", ok(RadeonFixtures.configurable()));
        clock.advanceTo(1000);
        assertNotNull(last().model);
        assertNull(last().loadFailure);
        transport.reply("pc", "radeon", error("network", "refused"));
        clock.advanceTo(2000);
        assertNotNull("a failed read does not blank a panel on screen", last().model);
    }

    @Test
    public void anUnchangedReadKeepsTheSameModelObjectSoTheScreenIsNotRebuilt() {
        transport.reply("pc", "radeon", ok(RadeonFixtures.configurable()));
        open();
        clock.advanceTo(0);
        RadeonPanelModel first = last().model;
        transport.reply("pc", "radeon", ok(RadeonFixtures.configurable().replace("\"revision\":3", "\"revision\":4")));
        clock.advanceTo(1000);
        assertSame(first, last().model);
        transport.reply("pc", "radeon",
                ok(RadeonFixtures.configurable().replace("\"value\":144", "\"value\":90")));
        clock.advanceTo(2000);
        assertEquals(90L, last().model.find("chill.max_fps").value);
    }

    @Test
    public void aWriteSendsOneTypedChangeAndTheAnswerReplacesThePanel() throws Exception {
        transport.reply("pc", "radeon", ok(RadeonFixtures.configurable()));
        open();
        clock.advanceTo(0);
        RadeonPanelModel.Row max = last().model.find("chill.max_fps");
        transport.reply("pc", "radeon_write", ok(RadeonFixtures.configurable()
                .replace("\"value\":144", "\"value\":150").replace("\"revision\":3", "\"revision\":4")));
        controller.write(max, 148L);
        assertEquals("chill.max_fps", last().pendingId);
        clock.advanceTo(0);
        assertEquals(1, calls("radeon_write"));
        JSONObject sent = new JSONObject(transport.arguments.get(transport.calls.indexOf("pc|radeon_write")));
        assertEquals(9, sent.getLong("gpu"));
        assertEquals("snapped to the step", 150, sent.getJSONArray("changes").getJSONObject(0).getLong("value"));
        assertNull(last().pendingId);
        assertNull(last().writeFailure);
        assertEquals(150L, last().model.find("chill.max_fps").value);
        assertEquals("the answer is the panel, no extra read", 1, calls("radeon"));
    }

    @Test
    public void aRejectedWriteShowsTheErrorAtItsSettingAndReadsTheRealValueBack() {
        transport.reply("pc", "radeon", ok(RadeonFixtures.configurable()));
        open();
        clock.advanceTo(0);
        RadeonPanelModel.Row chill = last().model.find("chill.enabled");
        transport.reply("pc", "radeon_write", "{\"error\":{\"kind\":\"api\",\"message\":\"The driver said no.\","
                + "\"http_status\":409,\"code\":\"driver_rejected\",\"setting\":\"chill.enabled\"}}");
        controller.write(chill, true);
        clock.advanceTo(0);
        assertNull(last().pendingId);
        assertNotNull(last().writeFailure);
        assertEquals(R.string.native_error_driver_rejected, last().writeFailure.messageRes);
        assertEquals("chill.enabled", last().writeFailure.settingId);
        assertEquals(2, calls("radeon"));
        assertNotNull("the panel stays", last().model);
        // A later successful write clears the error.
        transport.reply("pc", "radeon_write", ok(RadeonFixtures.configurable()));
        controller.write(chill, true);
        assertNull("starting a write clears the old error", last().writeFailure);
    }

    @Test
    public void writeErrorsAreKeptPerTheirCode() {
        transport.reply("pc", "radeon", ok(RadeonFixtures.configurable()));
        open();
        clock.advanceTo(0);
        RadeonPanelModel.Row chill = last().model.find("chill.enabled");
        String[][] cases = {
                {"422", "invalid_value", String.valueOf(R.string.native_error_invalid_value)},
                {"422", "not_writable", String.valueOf(R.string.native_error_not_writable)},
                {"403", "host_control_required", String.valueOf(R.string.native_error_host_control)},
                {"409", "no_session", String.valueOf(R.string.native_error_no_session)},
                {"503", "temporarily_unavailable", String.valueOf(R.string.native_error_busy)},
        };
        for (String[] c : cases) {
            transport.reply("pc", "radeon_write", "{\"error\":{\"kind\":\"api\",\"message\":\"m\",\"http_status\":"
                    + c[0] + ",\"code\":\"" + c[1] + "\"}}");
            controller.write(chill, true);
            clock.advanceTo(clock.now);
            assertEquals(c[1], Integer.parseInt(c[2]), last().writeFailure.messageRes);
            assertEquals("chill.enabled", last().writeFailure.settingId);
        }
    }

    @Test
    public void aWriteToAHostThatLostItsEndpointsHidesThePanel() {
        transport.reply("pc", "radeon", ok(RadeonFixtures.configurable()));
        open();
        clock.advanceTo(0);
        transport.reply("pc", "radeon_write", error("not_found", "gone"));
        controller.write(last().model.find("chill.enabled"), true);
        clock.advanceTo(0);
        assertTrue(last().hidden);
    }

    @Test
    public void nothingIsDeliveredAfterTheMenuClosed() {
        transport.reply("pc", "radeon", ok(RadeonFixtures.configurable()));
        open();
        clock.advanceTo(0);
        int delivered = states.size();
        controller.stop();
        clock.advanceTo(5000);
        assertEquals(delivered, states.size());
    }
}
