package com.limelight.computers;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.os.Looper;
import android.view.View;
import android.view.ViewGroup;
import android.widget.TextView;

import androidx.appcompat.app.AlertDialog;

import com.limelight.R;
import com.limelight.nativeapi.HostEndpoint;
import com.limelight.nativeapi.NativeApiClient;
import com.limelight.nativeapi.NativeCoopTest;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;
import com.limelight.ui.ToolbarIconTintTest;

import com.google.android.material.button.MaterialButton;

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

import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

/** Join co-op's sheet against scripted native_api.rs envelopes of GET /coop and POST /coop/join. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class CoopSheetTest {
    private static final String OPEN = NativeCoopTest.LISTING.replace("\"you\":true,\"away\":true",
            "\"you\":false,\"away\":false");
    private static final String JOINED = "{\"ok\":{\"session\":3000000000,\"slot\":2,\"app_id\":2147483647}}";

    private ActivityController<ToolbarIconTintTest.Screen> controller;
    private ToolbarIconTintTest.Screen screen;
    private final Map<String, ArrayDeque<String>> replies = new HashMap<>();
    private final List<String> calls = new ArrayList<>();
    /** Host calls wait here until the test runs them, so it can close the sheet or tap again meanwhile. */
    private final ArrayDeque<Runnable> pending = new ArrayDeque<>();
    private boolean inline = true;
    private final Recorder ui = new Recorder();

    private static final class Recorder implements CoopSheet.Ui {
        final List<String> events = new ArrayList<>();
        CharSequence title;
        CharSequence message;
        Runnable retry;
        NvApp started;

        @Override public void problem(CharSequence title, CharSequence message, Runnable retry) {
            events.add("problem");
            this.title = title;
            this.message = message;
            this.retry = retry;
        }

        @Override public void start(NvApp app) {
            events.add("start");
            started = app;
        }

        @Override public void reopen() {
            events.add("reopen");
        }
    }

    @Before
    public void setUp() {
        controller = Robolectric.buildActivity(ToolbarIconTintTest.Screen.class).setup().visible();
        screen = controller.get();
    }

    @After
    public void tearDown() {
        controller.pause().stop().destroy();
    }

    private static String api(int status, String code, String message) {
        return "{\"error\":{\"kind\":\"api\",\"message\":\"" + message + "\",\"http_status\":" + status
                + ",\"code\":\"" + code + "\"}}";
    }

    private static final String UNREACHABLE = "{\"error\":{\"kind\":\"network\",\"message\":\"connection refused\"}}";

    private void script(String op, String... envelopes) {
        replies.computeIfAbsent(op, k -> new ArrayDeque<>()).addAll(Arrays.asList(envelopes));
    }

    private static ComputerDetails pc() {
        ComputerDetails pc = new ComputerDetails();
        pc.uuid = "pc";
        pc.name = "Office";
        pc.state = ComputerDetails.State.ONLINE;
        return pc;
    }

    private CoopSheet sheet() {
        NativeApiClient api = new NativeApiClient((host, op, argument) -> {
            calls.add(argument.isEmpty() ? op : op + " " + argument);
            ArrayDeque<String> queue = replies.get(op);
            if (queue == null || queue.isEmpty()) {
                throw new AssertionError("Unexpected " + op);
            }
            return queue.poll();
        }, new HostEndpoint("pc", "10.0.0.2", 47989, 47984, "pem", "unique", new byte[1], new byte[1]));
        CoopSheet sheet = new CoopSheet(screen, pc(), () -> api, task -> {
            if (inline) task.run();
            else pending.add(task);
        }, Runnable::run, ui);
        sheet.show();
        return sheet;
    }

    private static void texts(View view, StringBuilder into) {
        if (view instanceof TextView && ((TextView) view).getText().length() > 0) {
            into.append(((TextView) view).getText()).append('|');
        }
        if (view instanceof ViewGroup) {
            for (int i = 0; i < ((ViewGroup) view).getChildCount(); i++) {
                texts(((ViewGroup) view).getChildAt(i), into);
            }
        }
    }

    private static String texts(CoopSheet sheet) {
        StringBuilder text = new StringBuilder();
        texts(sheet.sheet().findViewById(android.R.id.content), text);
        return text.toString();
    }

    private static MaterialButton row(View view, String label) {
        if (view instanceof MaterialButton && ((MaterialButton) view).getText().toString().startsWith(label)) {
            return (MaterialButton) view;
        }
        if (view instanceof ViewGroup) {
            for (int i = 0; i < ((ViewGroup) view).getChildCount(); i++) {
                MaterialButton found = row(((ViewGroup) view).getChildAt(i), label);
                if (found != null) return found;
            }
        }
        return null;
    }

    private MaterialButton row(CoopSheet sheet, int label) {
        return row(sheet.sheet().findViewById(android.R.id.content), screen.getString(label));
    }

    @Test
    public void theListShowsEachSessionsPlayersFreeSlotsAndJoin() {
        script("coop", "{\"ok\":" + OPEN + "}");
        CoopSheet sheet = sheet();
        String text = texts(sheet);
        assertTrue(text, text.contains("Portal 2|"));
        assertTrue(text, text.contains("Running|"));
        assertTrue(text, text.contains("Player 1|Laptop|"));
        assertTrue(text, text.contains("Player 2|Retroid|"));
        assertTrue(text, text.contains("Player 3|Free|"));
        MaterialButton join = row(sheet, R.string.coop_join);
        assertEquals("Join\n1 free slot", join.getText().toString());
        assertEquals(1f, join.getAlpha(), 0f);
        assertNull(row(sheet, R.string.coop_return));
        assertNotNull(row(sheet, R.string.coop_refresh));
        assertEquals(Arrays.asList("coop"), calls);
        assertTrue(sheet.sheet().isShowing());
    }

    @Test
    public void thisDevicesOwnSeatOffersItsWayBack() {
        script("coop", "{\"ok\":" + NativeCoopTest.LISTING + "}");
        CoopSheet sheet = sheet();
        String text = texts(sheet);
        assertTrue(text, text.contains("Player 2|Retroid (this device), away|"));
        assertEquals("Return to your seat\nPlayer 2", row(sheet, R.string.coop_return).getText().toString());
        assertNull(row(sheet, R.string.coop_join));
    }

    @Test
    public void noSessionSaysHowToStartOne() {
        script("coop", "{\"ok\":{\"available\":true,\"games\":[],\"sessions\":[]}}");
        CoopSheet sheet = sheet();
        assertTrue(texts(sheet).contains(screen.getString(R.string.coop_none)));
        assertNotNull(row(sheet, R.string.coop_refresh));
    }

    @Test
    public void aSessionThisDeviceCannotJoinIsDimmedWithTheHostsReasonAndStillAsks() {
        script("coop", "{\"ok\":{\"available\":true,\"games\":[],\"sessions\":[{\"id\":4,\"app_id\":9,"
                + "\"title\":\"Racer\",\"phase\":\"ended\",\"end\":{\"reason\":\"crashed\",\"detail\":"
                + "\"Instance 2 exited\"},\"players\":[],\"free_slots\":0,\"joinable\":false,\"refusal\":"
                + "\"The session has ended.\"}]}}");
        script("coop.join", api(409, "unknown", "The session has ended."));
        script("coop", "{\"ok\":{\"available\":true,\"games\":[],\"sessions\":[]}}");
        CoopSheet sheet = sheet();
        String text = texts(sheet);
        assertTrue(text, text.contains("Ended|"));
        assertTrue(text, text.contains("A game instance crashed.\nInstance 2 exited|"));
        MaterialButton join = row(sheet, R.string.coop_join);
        assertEquals("Join\nThe session has ended.", join.getText().toString());
        assertEquals(HostMenu.DISABLED_ALPHA, join.getAlpha(), 0f);
        assertTrue("D-pad and TalkBack reach it", join.isEnabled() && join.isFocusable());
        join.performClick();
        assertEquals(Arrays.asList("coop", "coop.join 4", "coop"), calls);
        assertEquals("Office refused the join", ui.title.toString());
        assertEquals("The session has ended.", ui.message.toString());
    }

    @Test
    public void aJoinSeatsThisDeviceAndStartsTheCoopEntry() {
        script("coop", "{\"ok\":" + OPEN + "}");
        script("coop.join", JOINED);
        CoopSheet sheet = sheet();
        row(sheet, R.string.coop_join).performClick();
        assertEquals(Arrays.asList("coop", "coop.join 3000000000"), calls);
        assertEquals(Arrays.asList("start"), ui.events);
        assertEquals(Integer.MAX_VALUE, ui.started.getAppId());
        assertEquals("Portal 2 (Co-op)", ui.started.getAppName());
        assertFalse(sheet.sheet().isShowing());
    }

    @Test
    public void aDoubleTapJoinsOnce() {
        script("coop", "{\"ok\":" + OPEN + "}");
        script("coop.join", JOINED);
        CoopSheet sheet = sheet();
        inline = false;
        MaterialButton join = row(sheet, R.string.coop_join);
        join.performClick();
        join.performClick();
        assertEquals(1, pending.size());
        assertTrue(texts(sheet).contains("Joining Portal 2…"));
        pending.poll().run();
        assertEquals(Arrays.asList("start"), ui.events);
    }

    @Test
    public void anAnswerAfterTheSheetClosedStartsNothing() {
        script("coop", "{\"ok\":" + OPEN + "}");
        script("coop.join", JOINED);
        CoopSheet sheet = sheet();
        inline = false;
        row(sheet, R.string.coop_join).performClick();
        sheet.close();
        pending.poll().run();
        assertTrue(ui.events.isEmpty());
    }

    @Test
    public void aListAfterTheSheetClosedShowsNothing() {
        inline = false;
        script("coop", UNREACHABLE);
        CoopSheet sheet = sheet();
        assertTrue(texts(sheet).contains("Asking Office for its co-op sessions…"));
        sheet.sheet().dismiss();
        Shadows.shadowOf(Looper.getMainLooper()).idle();
        pending.poll().run();
        assertTrue(ui.events.isEmpty());
    }

    @Test
    public void anOlderListAnswerIsDropped() {
        // The first refresh's answer comes last, and it would show the session again.
        script("coop", "{\"ok\":" + OPEN + "}", "{\"ok\":{\"available\":true,\"games\":[],\"sessions\":[]}}",
                "{\"ok\":" + OPEN + "}");
        CoopSheet sheet = sheet();
        inline = false;
        MaterialButton refresh = row(sheet, R.string.coop_refresh);
        refresh.performClick();
        refresh.performClick();
        Runnable older = pending.poll();
        Runnable newer = pending.poll();
        newer.run();
        assertTrue(texts(sheet).contains(screen.getString(R.string.coop_none)));
        older.run();
        assertTrue(texts(sheet).contains(screen.getString(R.string.coop_none)));
        assertNull(row(sheet, R.string.coop_join));
        assertTrue(ui.events.isEmpty());
    }

    @Test
    public void permissionKeepsTheListAndSaysWhoTurnsItOn() {
        script("coop", "{\"ok\":" + OPEN + "}");
        script("coop.join", api(403, "permission_required", "Turn on Join co-op for this device on the host."));
        CoopSheet sheet = sheet();
        row(sheet, R.string.coop_join).performClick();
        assertEquals(Arrays.asList("problem"), ui.events);
        assertEquals("Join co-op is off for this device", ui.title.toString());
        assertEquals("Turn on Join co-op for this device on the host.", ui.message.toString());
        assertNull(ui.retry);
        assertEquals(Arrays.asList("coop", "coop.join 3000000000"), calls);
        assertTrue(sheet.sheet().isShowing());
        assertNotNull(row(sheet, R.string.coop_join));
    }

    @Test
    public void refusedGoneAndBusyShowTheHostsWordsAndReadTheListAgain() {
        String[][] cases = {
                {"409", "unknown", "The session is full.", "Office refused the join"},
                {"409", "no_session", "That co-op session no longer exists.", "The co-op session is gone"},
                {"409", "session_busy", "Another co-op session is starting.", "Co-op is busy on Office"},
        };
        for (String[] c : cases) {
            replies.clear();
            calls.clear();
            ui.events.clear();
            script("coop", "{\"ok\":" + OPEN + "}", "{\"ok\":" + OPEN + "}");
            script("coop.join", api(Integer.parseInt(c[0]), c[1], c[2]));
            CoopSheet sheet = sheet();
            row(sheet, R.string.coop_join).performClick();
            assertEquals(c[1], Arrays.asList("problem"), ui.events);
            assertEquals(c[1], c[3], ui.title.toString());
            assertEquals(c[1], c[2], ui.message.toString());
            assertNull(c[1], ui.retry);
            assertEquals(c[1], Arrays.asList("coop", "coop.join 3000000000", "coop"), calls);
            assertNotNull(c[1], row(sheet, R.string.coop_join));
            sheet.close();
        }
    }

    @Test
    public void anUnavailableJoinClosesTheSheetWithTheHostsWords() {
        script("coop", "{\"ok\":" + OPEN + "}");
        script("coop.join", api(503, "temporarily_unavailable", "Nucleus Co-op is not installed."));
        CoopSheet sheet = sheet();
        row(sheet, R.string.coop_join).performClick();
        assertEquals("Co-op is not available on Office", ui.title.toString());
        assertEquals("Nucleus Co-op is not installed.", ui.message.toString());
        assertNull(ui.retry);
        assertFalse(sheet.sheet().isShowing());
    }

    @Test
    public void anUnreachableJoinOffersTryAgain() {
        script("coop", "{\"ok\":" + OPEN + "}");
        script("coop.join", UNREACHABLE, JOINED);
        CoopSheet sheet = sheet();
        row(sheet, R.string.coop_join).performClick();
        assertEquals("Couldn’t reach Office", ui.title.toString());
        assertEquals("connection refused", ui.message.toString());
        assertNotNull(ui.retry);
        ui.retry.run();
        assertEquals(Arrays.asList("coop", "coop.join 3000000000", "coop.join 3000000000"), calls);
        assertEquals(Arrays.asList("problem", "start"), ui.events);
    }

    @Test
    public void aHostWithoutCoopSaysWhyAndCloses() {
        script("coop", "{\"ok\":{\"available\":false,\"reason\":\"Nucleus Co-op is not installed.\","
                + "\"games\":[],\"sessions\":[]}}");
        CoopSheet sheet = sheet();
        assertEquals("Co-op is not available on Office", ui.title.toString());
        assertEquals("Nucleus Co-op is not installed.", ui.message.toString());
        assertFalse(sheet.sheet().isShowing());
    }

    @Test
    public void anUnreachableListOffersToOpenAgainAndAnErrorWithoutWordsGivesTheStatus() {
        script("coop", UNREACHABLE);
        sheet();
        assertEquals("Couldn’t reach Office", ui.title.toString());
        assertNotNull(ui.retry);
        ui.retry.run();
        assertEquals(Arrays.asList("problem", "reopen"), ui.events);

        ui.events.clear();
        script("coop", api(500, "unknown", ""));
        CoopSheet other = sheet();
        assertEquals("Co-op failed", ui.title.toString());
        assertEquals("The PC gave no reason (HTTP 500).", ui.message.toString());
        assertNull(ui.retry);
        assertFalse(other.sheet().isShowing());
    }

    @Test
    public void hostActionsShowAProblemAsADialogWithTheHostsWords() {
        HostActions actions = new HostActions(screen, new HostActions.Callbacks() {
            @Override public ComputerManagerService.ComputerManagerBinder binder() { return null; }
            @Override public boolean isForeground() { return true; }
            @Override public void onPaired(ComputerDetails computer) { }
            @Override public void onRemoved(ComputerDetails computer) { }
            @Override public void onRenamed(ComputerDetails computer) { }
            @Override public void onWakeSent(ComputerDetails computer) { }
            @Override public void onWakeEnded(ComputerDetails computer) { }
            @Override public void pausePolling() { }
            @Override public void resumePolling() { }
        });
        try {
            script("coop", "{\"ok\":" + OPEN + "}");
            script("coop.join", api(403, "permission_required", "Turn on Join co-op for this device on the host."));
            NativeApiClient api = new NativeApiClient((host, op, argument) -> replies.get(op).poll(),
                    new HostEndpoint("pc", "10.0.0.2", 47989, 47984, "pem", "unique", new byte[1], new byte[1]));
            CoopSheet sheet = actions.openCoop(pc(), () -> api, Runnable::run);
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            row(sheet, R.string.coop_join).performClick();
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            AlertDialog dialog = (AlertDialog) ShadowDialog.getLatestDialog();
            assertTrue(dialog.isShowing());
            assertEquals("Turn on Join co-op for this device on the host.",
                    ((TextView) dialog.findViewById(android.R.id.message)).getText().toString());
            // A second sheet closes the first, so only the newest one speaks.
            script("coop", "{\"ok\":" + OPEN + "}");
            CoopSheet newer = actions.openCoop(pc(), () -> api, Runnable::run);
            Shadows.shadowOf(Looper.getMainLooper()).idle();
            assertFalse(sheet.sheet().isShowing());
            assertTrue(newer.sheet().isShowing());
        } finally {
            actions.destroy();
        }
        Shadows.shadowOf(Looper.getMainLooper()).idle();
    }
}
