package com.limelight.console;

import static com.limelight.console.HomeLibraryRowsTest.found;
import static com.limelight.console.HomeLibraryRowsTest.game;
import static com.limelight.nativeapi.NativeApiFixtures.library;
import static com.limelight.nativeapi.NativeApiFixtures.nativeHost;
import static com.limelight.nativeapi.NativeApiFixtures.ok;
import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;
import static org.robolectric.Shadows.shadowOf;

import android.app.Activity;
import android.app.Application;
import android.content.Intent;

import com.limelight.nativeapi.ConsoleRepository;
import com.limelight.nativeapi.NativeApiFixtures;
import com.limelight.nvstream.http.ComputerDetails;

import org.junit.After;
import org.junit.Before;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;
import org.junit.runner.RunWith;
import org.robolectric.Robolectric;
import org.robolectric.RobolectricTestRunner;
import org.robolectric.android.controller.ActivityController;
import org.robolectric.annotation.Config;
import org.robolectric.shadows.ShadowToast;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.HashSet;
import java.util.List;

/** Opening a card: a game in the library opens its page, one the host found is added first or sent to the PC. */
@RunWith(RobolectricTestRunner.class)
@Config(sdk = 29, application = Application.class)
public class GameOpenerTest {
    @Rule public TemporaryFolder temporary = new TemporaryFolder();

    private ConsoleRig rig;
    private ActivityController<Activity> controller;
    private Activity activity;
    private final ComputerDetails tower = nativeHost("tower", "Tower", true);
    /** The worker: runs nothing until the test says, so a press can come while an add is in flight. */
    private final List<Runnable> queued = new ArrayList<>();
    private int added;

    private final GameOpener.Screen screen = new GameOpener.Screen() {
        @Override public Activity activity() { return activity; }
        @Override public ConsoleRepository repository() { return rig.repository; }
        @Override public ComputerDetails host(String uuid) { return "tower".equals(uuid) ? tower : null; }
        @Override public Class<? extends GameDetailActivity> detailActivity() { return GameDetailActivity.class; }
        @Override public void onAdded() { added++; }
    };

    @Before
    public void setUp() throws Exception {
        rig = new ConsoleRig(temporary.newFolder());
        controller = Robolectric.buildActivity(Activity.class).setup();
        activity = controller.get();
    }

    @After
    public void tearDown() {
        controller.destroy();
        rig.close();
    }

    private HomeModel model(List<String> features) throws Exception {
        StringBuilder names = new StringBuilder("[\"library\"");
        for (String feature : features) names.append(",\"").append(feature).append('"');
        rig.transport.reply("tower", "capabilities", ok("{\"api_version\":1,\"features\":" + names + "],"
                + "\"permissions\":{\"list\":true,\"view\":true,\"launch\":true}}"));
        rig.transport.reply("tower", "library", ok(library("t", game(1, "Portal", "steam", true, null),
                found("Hades", "epic"))));
        rig.transport.reply("tower", "session", ok(NativeApiFixtures.IDLE));
        return HomeModel.of(rig.repository.refresh(Collections.singletonList(tower)),
                new HashSet<>(Collections.singletonList("tower")));
    }

    private static HomeModel.GameCard card(HomeModel model, String title) {
        for (HomeModel.GameCard card : model.allGames) {
            if (card.title.equals(title)) return card;
        }
        throw new AssertionError(title + " is not listed");
    }

    @Test
    public void aGameInTheLibraryOpensItsPage() throws Exception {
        GameOpener.open(screen, queued::add, card(model(Collections.emptyList()), "Portal"));
        Intent page = shadowOf(activity).getNextStartedActivity();
        assertEquals(GameDetailActivity.class.getName(), page.getComponent().getClassName());
        assertEquals("tower", page.getStringExtra(GameDetailActivity.EXTRA_HOST_UUID));
        assertEquals(1, page.getIntExtra(GameDetailActivity.EXTRA_APP_ID, -1));
        assertEquals("Portal", page.getStringExtra(GameDetailActivity.EXTRA_TITLE));
        assertTrue(queued.isEmpty());
    }

    @Test
    public void aGameTheHostCannotAddOnRequestSaysToAddItOnThePc() throws Exception {
        GameOpener.open(screen, queued::add, card(model(Collections.singletonList("library.unadded")), "Hades"));
        assertEquals(activity.getString(com.limelight.R.string.console_add_on_pc), ShadowToast.getTextOfLatestToast());
        assertNull(shadowOf(activity).getNextStartedActivity());
        assertTrue(queued.isEmpty());
    }

    @Test
    public void aGameOnAHostThatDeniesLaunchSaysToAddItOnThePc() throws Exception {
        HomeModel.GameCard hades = card(model(Arrays.asList("library.unadded", "library.add")), "Hades");
        tower.permission = ComputerDetails.PERMISSION_LIST;
        GameOpener.open(screen, queued::add, hades);
        assertEquals(activity.getString(com.limelight.R.string.console_add_on_pc), ShadowToast.getTextOfLatestToast());
        assertTrue(queued.isEmpty());
    }

    @Test
    public void anAddableGameIsAddedOnceThenOpensWithTheAppIdTheHostGaveIt() throws Exception {
        HomeModel.GameCard hades = card(model(Arrays.asList("library.unadded", "library.add")), "Hades");
        rig.transport.reply("tower", "library_add", ok("{\"install_id\":\"i9\",\"app_id\":9,\"title\":\"Hades\","
                + "\"game_id\":\"found:Hades\"}"));
        GameOpener.open(screen, queued::add, hades);
        assertEquals(activity.getString(com.limelight.R.string.console_adding, "Hades"),
                ShadowToast.getTextOfLatestToast());
        assertTrue(GameOpener.adding("tower", "found:Hades"));

        // A second press while the host is still adding it does nothing.
        GameOpener.open(screen, queued::add, hades);
        assertEquals(1, queued.size());

        queued.remove(0).run();
        assertFalse(GameOpener.adding("tower", "found:Hades"));
        assertEquals(1, added);
        assertEquals(1, java.util.Collections.frequency(rig.transport.calls, "tower|library_add"));
        Intent page = shadowOf(activity).getNextStartedActivity();
        assertEquals(9, page.getIntExtra(GameDetailActivity.EXTRA_APP_ID, -1));
        assertEquals("Hades", page.getStringExtra(GameDetailActivity.EXTRA_TITLE));
    }

    @Test
    public void aFailedAddSaysSoAndAnotherPressTriesAgain() throws Exception {
        HomeModel.GameCard hades = card(model(Arrays.asList("library.unadded", "library.add")), "Hades");
        GameOpener.open(screen, queued::add, hades);
        queued.remove(0).run();
        assertEquals(activity.getString(com.limelight.R.string.console_add_failed, "Hades"),
                ShadowToast.getTextOfLatestToast());
        assertNull(shadowOf(activity).getNextStartedActivity());

        GameOpener.open(screen, queued::add, hades);
        assertEquals(1, queued.size());
        queued.remove(0).run();
    }
}
