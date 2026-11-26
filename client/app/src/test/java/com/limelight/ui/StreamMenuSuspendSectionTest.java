package com.limelight.ui;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import android.app.Application;
import android.content.Context;

import com.limelight.R;
import com.limelight.nativeapi.HostEndpoint;
import com.limelight.nativeapi.NativeApiClient;
import com.limelight.nativeapi.NativeApiFixtures;
import com.limelight.nvstream.http.ComputerDetails;
import com.limelight.nvstream.http.NvApp;

import org.junit.Test;
import org.junit.runner.RunWith;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.RuntimeEnvironment;
import org.robolectric.annotation.Config;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;

/** Quick Suspend's tile, its requests through the native API and the lines the stream shows for the host's answers. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class StreamMenuSuspendSectionTest {
    private static final String READY = status("ready", "none", "null");
    private static final String SUSPENDED = status("suspended", "none", "null");
    private static final String REFUSED = status("refused", "anti_cheat_detected",
            "\"This game uses anti-cheat protection and cannot be paused safely.\"");

    private static String status(String state, String reason, String message) {
        return NativeApiFixtures.ok("{\"state\":\"" + state + "\",\"reason\":\"" + reason + "\",\"message\":" + message
                + ",\"app_title\":\"Hollow Knight\",\"suspended_count\":0,\"suspended_since_unix_ms\":null,"
                + "\"suspended_by\":null}");
    }

    private final Context context = RuntimeEnvironment.getApplication();
    private final NativeApiFixtures.FakeTransport transport = new NativeApiFixtures.FakeTransport();

    /** Runs everything at once on the test's thread and keeps what the stream would show. */
    private final class FakeHost implements StreamMenuSuspendSection.Host {
        boolean offered = true;
        final List<String> said = new ArrayList<>();
        int refreshed;

        @Override public boolean offered() { return offered; }

        @Override public NativeApiClient client() {
            ComputerDetails host = NativeApiFixtures.nativeHost("pc", "PC", true);
            return new NativeApiClient(transport, HostEndpoint.of(host, "unique", NativeApiFixtures.crypto()));
        }

        @Override public boolean background(Runnable task) {
            task.run();
            return true;
        }

        @Override public void ui(Runnable task) { task.run(); }

        @Override public void say(CharSequence text) { said.add(text.toString()); }

        @Override public void refreshMenu() { refreshed++; }
    }

    private static List<StreamMenu.Item> tiles(StreamMenuSuspendSection section, NvApp.Role role) {
        StreamMenu.Options options = StreamMenuTestSupport.options(StreamMenuTestSupport.nativeApiHost(), role);
        options.quickSuspendOffered = section.offered();
        options.quickSuspendPaused = section.paused();
        List<StreamMenu.Entry> entries = new ArrayList<>();
        StreamMenuSuspendSection.addTo(entries, options);
        List<StreamMenu.Item> items = new ArrayList<>();
        for (StreamMenu.Entry entry : entries) items.add(entry.item);
        return items;
    }

    @Test
    public void theTileFollowsTheHostAndARefusalSaysWhyInThePlayersWords() {
        FakeHost host = new FakeHost();
        StreamMenuSuspendSection section = new StreamMenuSuspendSection(context, host);
        assertEquals("nothing before the host answers", Arrays.asList(), tiles(section, NvApp.Role.STREAM));

        transport.reply("pc", "quick_suspend", READY);
        section.refresh();
        assertEquals(Arrays.asList(StreamMenu.Item.SUSPEND_GAME), tiles(section, NvApp.Role.STREAM));
        assertEquals("only the device playing its own game", Arrays.asList(), tiles(section, NvApp.Role.REMOTE_MONITOR));
        assertEquals(1, host.refreshed);

        transport.reply("pc", "quick_suspend.suspend", REFUSED);
        section.choose(StreamMenu.Item.SUSPEND_GAME);
        assertEquals(Arrays.asList(context.getString(R.string.quick_suspend_refused_anti_cheat)), host.said);
        assertEquals("a refusal leaves the game as it was", Arrays.asList(StreamMenu.Item.SUSPEND_GAME),
                tiles(section, NvApp.Role.STREAM));

        transport.reply("pc", "quick_suspend.suspend", SUSPENDED);
        section.choose(StreamMenu.Item.SUSPEND_GAME);
        assertTrue(section.paused());
        assertEquals(context.getString(R.string.quick_suspend_paused), host.said.get(1));
        assertEquals(Arrays.asList(StreamMenu.Item.RESUME_GAME), tiles(section, NvApp.Role.STREAM));

        transport.reply("pc", "quick_suspend.resume", READY);
        section.choose(StreamMenu.Item.RESUME_GAME);
        assertFalse(section.paused());
        assertEquals(context.getString(R.string.quick_suspend_resumed), host.said.get(2));

        // No answer is said too, never swallowed.
        transport.reply("pc", "quick_suspend.resume", NativeApiFixtures.error("network", "connection refused"));
        section.choose(StreamMenu.Item.RESUME_GAME);
        assertTrue(host.said.get(3), host.said.get(3).contains("connection refused"));
        assertEquals(Arrays.asList("pc|quick_suspend", "pc|quick_suspend.suspend", "pc|quick_suspend.suspend",
                "pc|quick_suspend.resume", "pc|quick_suspend.resume"), transport.calls);
    }

    @Test
    public void aHostThatDoesNotOfferItIsNotAsked() {
        FakeHost host = new FakeHost();
        host.offered = false;
        StreamMenuSuspendSection section = new StreamMenuSuspendSection(context, host);
        section.refresh();
        assertTrue(transport.calls.isEmpty());
        assertFalse(section.offered());
    }

    @Test
    public void everyReasonTheHostNamesHasWordsAndANewOneReadsAsAFailure() {
        String[][] reasons = {
                {"anti_cheat_detected", "refused_anti_cheat"}, {"protected_process", "refused_protected"},
                {"denylisted_process", "refused_protected"}, {"guests_active", "refused_others"},
                {"coop_session", "refused_others"}, {"not_owner", "refused_not_owner"}, {"session_busy", "refused_busy"},
                {"no_session", "refused_nothing"}, {"no_owned_process", "refused_nothing"},
                {"disabled_for_app", "refused_nothing"}, {"io_or_os_error", "refused_failed"},
                {"hibernate_failed", "refused_failed"}, {null, "refused_failed"},
        };
        for (String[] reason : reasons) {
            int words = StreamMenuSuspendSection.refusal(reason[0]);
            assertEquals(String.valueOf(reason[0]), "quick_suspend_" + reason[1], context.getResources().getResourceEntryName(words));
        }
    }
}
